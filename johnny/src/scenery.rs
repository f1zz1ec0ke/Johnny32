//! The island backdrop: the sea, the island, and the surf breaking round it.
//!
//! This is `SCRANTIC.EXE` segment 3. The screensaver's screen is 640x480
//! (`seg2:0x1483` writes 640 and 480 into `ds:0x2562`/`ds:0x2564`, and
//! `seg3:0x01D1` clears a rectangle that size), and the picture is built in
//! three passes:
//!
//! 1. **The sea.** `seg3:0x01E7` picks it: before phase 12 one of
//!    `OCEAN00/01/02.SCR` at random, from the pointer table at `ds:0x0262`;
//!    from phase 12 on, `NIGHT.SCR` at `ds:0x02D3`. All four are 640x480.
//! 2. **The island**, out of `BACKGRND.BMP`. Six pieces at fixed positions,
//!    two of them on a coin flip that is remembered for the session
//!    (`ds:0x30C2`, rolled once at `seg3:0x0226`).
//! 3. **The surf.** Seven patches of breaking water round the shore, three
//!    frames each, driven by three counters that chase each other so the
//!    whole shoreline does not move in lockstep.

use crate::ads::Rng;
use crate::dgds::{self, Archive, Frame};
use crate::render::{Mirror, Surface, SCREEN_H, SCREEN_W};

/// Ticks between surf steps. `seg3:0x0E96` writes 4 into the delay slot at
/// `ds:0x0288` and `seg3:0x0E8F` will not step again until it reaches zero,
/// so one step is 64ms.
pub const SURF_TICKS: u32 = 4;

/// The static island, as `(frame, x, y)`. Every one of these is a blit
/// through the handle table at `ds:0x30AC`, and the addresses are where each
/// is set up in segment 3.
///
/// `seg3:0x0226` flips a coin into `ds:0x30C2` once a session, and that picks
/// between **two islands**, not between an island and some trimmings. With
/// the flip up, the sandbar carries the beach forward and the rock sits
/// offshore, and four more patches of surf come with them; the placement
/// rectangle is different (`seg3:0x1B56`) and so is where the raft ties up
/// (`seg3:0x1DCB`). `ISLETEMP.SCR` is the other layout, and has none of it.
const ISLAND: [(usize, i32, i32, bool); 6] = [
    // frame,  x,   y,   always
    (0, 0x121, 0x117, true),  // seg3:0x0270 -- the island itself, 280x52
    (1, 0x0FA, 0x12F, false), // seg3:0x0293 -- 384x69
    (2, 0x097, 0x148, false), // seg3:0x02B0 -- 64x30
    (14, 0x18D, 0x117, true), // seg3:0x02EA -- a starfish and two shells
    (13, 0x1BB, 0x094, true), // seg3:0x0307 -- the palm trunk, 24x145
    (12, 0x16E, 0x07A, true), // seg3:0x0324 -- the fronds, 152x69
];

/// The surf, as `(x, y, first frame, counter, sandbar only)`. Each patch has
/// three frames; which one is showing comes from one of the three counters at
/// `ds:0x0282`, `ds:0x0284` and `ds:0x0286`.
///
/// The last four belong to the sandbar, not to the island. They have beach
/// baked into them, they sit exactly along the sandbar's edge and around the
/// rock, and `ISLETEMP.SCR` -- which is the layout without the sandbar --
/// contains no trace of any of them, while it matches the first three at
/// 54-75%. Drawn without the sandbar they are strips of sand floating in open
/// water, which is what they looked like.
const SURF: [(i32, i32, usize, usize, bool); 7] = [
    (0x10F, 0x132, 3, 0, false),  // seg3:0x13A0
    (0x16D, 0x13F, 6, 1, false),  // seg3:0x13C9
    (0x207, 0x12F, 9, 2, false),  // seg3:0x13F2
    (0x0EA, 0x143, 30, 0, true),  // seg3:0x141B -- the sandbar's left end
    (0x170, 0x164, 33, 1, true),  // seg3:0x1444 -- along its front
    (0x22F, 0x143, 36, 2, true),  // seg3:0x146D -- its right end
    (0x082, 0x154, 39, 0, true),  // seg3:0x1496 -- around the rock
];

/// Ticks between wave steps. `seg3:0x0A19` writes 10 into the delay slot at
/// `ds:0x028A`, so one step is 160ms.
pub const WAVE_TICKS: u32 = 10;

/// The waves out on the open sea, in the order their spawn handlers sit in
/// the table at `seg3:0x0A30`.
///
/// Each is a four-frame animation of `BACKGRND.BMP` played as a ping-pong,
/// from the sequences at `ds:0x0268`, `ds:0x0270` and `ds:0x0278`, and each
/// appears at a random spot inside a band of sea. The three bands are three
/// distances: the small one sits high and far off, the big one low and close.
struct Kind {
    /// Frame indices, terminated in the data by `0xFF`.
    sequence: [usize; 7],
    /// How many of this size may be on the water at once (`seg3:0x046E`).
    budget: u8,
    /// Top-left of the band, and how far the spawn may wander inside it.
    base: (i32, i32),
    span: (i32, i32),
}

const KINDS: [Kind; 3] = [
    // seg3:0x05CA, sequence at ds:0x0278 -- 32x6, furthest out
    Kind {
        sequence: [18, 19, 20, 21, 20, 19, 18],
        budget: 4,
        base: (-10, 249),
        span: (650, 40),
    },
    // seg3:0x0627, sequence at ds:0x0270 -- 64x12
    Kind {
        sequence: [22, 23, 24, 25, 24, 23, 22],
        budget: 2,
        base: (-40, 289),
        span: (680, 93),
    },
    // seg3:0x0682, sequence at ds:0x0268 -- 104x16, nearest
    Kind {
        sequence: [26, 27, 28, 29, 28, 27, 26],
        budget: 1,
        base: (-70, 382),
        span: (710, 142),
    },
];

/// One wave, mid-life. The engine keeps these in twelve-byte records in the
/// array at `ds:0x2F42` and walks them at `seg3:0x04BD`.
struct Wave {
    kind: usize,
    x: i32,
    y: i32,
    /// How far into the sequence; reaching the end retires the wave.
    cursor: usize,
    /// Steps to wait between frames, and the countdown to the next one.
    hold: u16,
    left: u16,
}

/// The clouds, from `seg3:0x0A38`. `BACKGRND.BMP` frames 15, 16 and 17 --
/// three sizes of cloud bank -- with a cap on how many of each may be in the
/// sky (`ds:0x2E7C`, `ds:0x2E7E`, `ds:0x2E80`, set to 8, 4 and 2) and the
/// weight each gets in the spawn roll (`seg3:0x0AB1`).
const CLOUDS: [(usize, u8, u32); 3] = [
    // frame, cap, weight
    (15, 8, 15),
    (16, 4, 10),
    (17, 2, 5),
];

/// Fifteen cloud slots (`seg3:0x0A7A`), placed in the sky: x anywhere across
/// the screen, y in the top 139 rows (`seg3:0x0B0C`).
const CLOUD_SLOTS: usize = 15;
const CLOUD_BAND: (i32, i32, i32, i32) = (0, 1, 640, 139);

/// One cloud drifting. The engine keeps these in the same twelve-byte records
/// as everything else, in the array at `ds:0x2E82`.
struct Cloud {
    kind: usize,
    x: i32,
    y: i32,
    /// Passes between one-pixel moves: `rand() % 5 + 6`, so every cloud
    /// travels at its own speed (`seg3:0x0B2B`).
    speed: u16,
    left: u16,
}

/// Sunlight on the water. `seg3:0x07DE` sets the drawing colour to 15 and
/// draws a line from x to x+1: a glint is two white pixels, nothing more.
///
/// Thirty at a time (`ds:0x2F38`), each lasting between ten and thirty-nine
/// steps of 160ms, scattered across a band of open water below the horizon
/// (`seg3:0x053C` -- x anywhere, y in 214..249).
const GLINTS: usize = 30;
const GLINT_BAND: (i32, i32, i32, i32) = (0, 214, 640, 35);

/// A glint that lands on the palm is thrown away rather than drawn -- these
/// are reflections, and the palm is not water. `seg3:0x055E` tests the trunk,
/// `seg3:0x058A` the fronds.
const NOT_WATER: [(i32, i32, i32, i32); 2] = [
    (0x1BB, 0x094, 0x16, 0x91), // the trunk
    (0x16E, 0x07A, 0x94, 0x45), // the fronds
];

struct Glint {
    x: i32,
    y: i32,
    left: u16,
}

/// Where the raft sits, without and with the rock (`seg3:0x1DCB`).
const RAFT_AT: [(i32, i32); 2] = [(0x201, 0x10A), (0x212, 0x119)];

/// What a scene asks of the backdrop, out of its 32-bit mask.
///
/// `seg3:0x1C41` takes the mask the chooser landed on and turns bits of it
/// into the backdrop's state. Passing all ones resets everything, which is how
/// the island gets back to normal between scenes.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct Backdrop {
    /// Bit 0x2000 (`seg3:0x1CDC`): not the island at all. The screen is
    /// cleared and the scene's own `.SCR` is the whole picture -- this is how
    /// the office and Suzy's flat get shown.
    pub cutaway: bool,
    /// Bit 0x0200 (`seg3:0x1C87`): no rock and no sandbar this time.
    pub no_rock: bool,
    /// Bit 0x4000 (`seg3:0x1C98`): no raft. The scene is about the raft and
    /// draws its own.
    pub no_raft: bool,
}

impl Backdrop {
    pub fn from_mask(mask: u32) -> Backdrop {
        Backdrop {
            cutaway: mask & 0x2000 != 0,
            no_rock: mask & 0x0200 != 0,
            no_raft: mask & 0x4000 != 0,
        }
    }
}

/// Which of `MRAFT.BMP`'s five frames the raft is at, on this story day.
///
/// `seg3:0x1D7E` reads the day from `ds:0x2D9C`, takes one off it and clamps
/// to 1..5. So there is a raft from the first day, it grows on days three
/// through six, and from day six it is finished.
pub fn raft_frame(day: u8) -> usize {
    (day.saturating_sub(1).clamp(1, 5) - 1) as usize
}

/// The three sea frames, in the order the pointer table at `ds:0x0262` has
/// them. `seg3:0x01F5` draws `rand() % 3`.
pub const SEAS: [&str; 3] = ["OCEAN00.SCR", "OCEAN01.SCR", "OCEAN02.SCR"];
pub const NIGHT_SEA: &str = "NIGHT.SCR";

/// Where the island's origin sits on screen.
///
/// Everything segment 3 draws is placed relative to `ds:0x30C6`/`ds:0x30C4`,
/// and the offsets are all written against a nominal (0x121, 0x117). This is
/// the correction from that nominal anchor to where the island actually goes.
///
/// `ISLETEMP.SCR` settles it: it is a render of this very composition, and
/// matching each piece against it finds the island at 96%, the palm trunk at
/// 95% and the fronds at 90% -- each one pixel to the left of the nominal
/// anchor, all three agreeing.
const ORIGIN: (i32, i32) = (-1, 0);

/// Whether the shells go on the beach.
///
/// `seg3:0x02EA` draws `BACKGRND.BMP` frame 14 -- a starfish and two shells --
/// at (397, 279), unconditionally, right on the island's white patch. But
/// `ISLETEMP.SCR`, which is a 96% render of everything else in this
/// composition, does not have them, and searching the whole image finds them
/// nowhere in it. Compositing with them scores 76.6% against that reference
/// and without them 81.1%.
///
/// So the code says draw them and the game's own picture of its island says
/// otherwise. The port sides with the picture, and `JC_FAITHFUL=1` sides with
/// the code, so the disagreement stays visible instead of being buried.
fn draw_shells() -> bool {
    std::env::var_os("JC_FAITHFUL").is_some()
}

/// The colour the sea's unused index is shown as.
///
/// The three ocean screens are the only artwork in the game that draws with
/// index 0, the engine's transparency key: about 10,000 pixels each, in
/// wave-crest shapes. Every one of them sits inside index 9 water -- for
/// `OCEAN00` all 9,917 are index 9 in `OCEAN01`, and every neighbouring pixel
/// in its own image is index 9 too. Whatever the engine puts there, it is a
/// second shade of open water, and it is not a palette effect: the binary
/// imports no `AnimatePalette` or `SetPaletteEntries`. Until that is settled,
/// the water they sit in is the one choice that cannot be wrong by much.
const SEA_KEY_AS: u8 = 9;

/// White. `seg3:0x07DE` is the only place the backdrop sets a draw colour.
const GLINT_COLOUR: u8 = 15;

/// The backdrop, and enough state to keep the surf moving.
pub struct Scenery {
    /// Sea and island: everything that does not move, composed once.
    base: Frame,
    /// `BACKGRND.BMP`, for the surf frames.
    pieces: Vec<Frame>,
    /// `ds:0x0282`, `ds:0x0284`, `ds:0x0286`. All three start at 2
    /// (`seg3:0x02C5`).
    counters: [u8; 3],
    /// Tick at which the counters step again.
    next: u32,
    /// The waves out on the water, and the tick they next move on.
    waves: Vec<Wave>,
    wave_next: u32,
    /// The clouds, and the tick they next move on.
    clouds: Vec<Cloud>,
    cloud_next: u32,
    /// The glints on the water. They share the object array and the 160ms
    /// step with the waves.
    glints: Vec<Glint>,
    /// The wind: `ds:0x0280`, +1 or -1, chosen once when the sky is set up
    /// (`seg3:0x0A59`) and shared by the clouds and the waves.
    drift: i32,
    pub sea: &'static str,
    pub extras: bool,
    /// The raft is part of the island, so the base has to be rebuilt when the
    /// story day moves it on or a scene asks for it to be left out.
    raft: Option<Frame>,
    day: u8,
    backdrop: Backdrop,
    /// The sea and nothing else, so the island can be laid down again when the
    /// story changes without decoding the screen a second time.
    sea_only: Frame,
}

impl Scenery {
    /// Compose the backdrop. `night` picks `NIGHT.SCR` over the oceans,
    /// `roll` supplies the two random choices: which sea, and whether the
    /// optional pieces are there.
    pub fn build(
        archive: &Archive,
        night: bool,
        roll: u32,
        now: u32,
        day: u8,
    ) -> Option<Scenery> {
        let sea = if night {
            NIGHT_SEA
        } else {
            SEAS[(roll % 3) as usize]
        };
        let mut sea_only = dgds::read_screen(archive.get(sea)?).ok()?;
        // `NIGHT.SCR` and the oceans are 640x480, the size of the screen. If
        // one ever is not, pad rather than leave the surface ragged.
        if sea_only.width != SCREEN_W || sea_only.height != SCREEN_H {
            sea_only = fit_to_screen(&sea_only);
        }
        for p in sea_only.pixels.iter_mut() {
            if *p == 0 {
                *p = SEA_KEY_AS;
            }
        }

        let pieces = archive
            .get("BACKGRND.BMP")
            .and_then(|b| dgds::read_bitmap(b).ok())
            .unwrap_or_default();
        // The raft is its own file, loaded next to the island at
        // `seg3:0x0179` and kept in `ds:0x30AE`.
        let raft = archive
            .get("MRAFT.BMP")
            .and_then(|b| dgds::read_bitmap(b).ok())
            .and_then(|frames| frames.into_iter().nth(raft_frame(day)));

        let mut scenery = Scenery {
            base: sea_only.clone(),
            sea_only,
            pieces,
            raft,
            day,
            backdrop: Backdrop::default(),
            counters: [2, 2, 2],
            next: now,
            waves: Vec::new(),
            wave_next: now,
            clouds: Vec::new(),
            cloud_next: now,
            glints: Vec::new(),
            // `seg3:0x0A59` rolls the wind once, here, and everything that
            // drifts drifts with it.
            drift: 1,
            sea,
            extras: roll / 3 % 2 == 1,
        };
        scenery.rebuild();
        Some(scenery)
    }

    /// Pick the wind and put some clouds up. `seg3:0x0A38` does both, once,
    /// when the backdrop is set up -- which is why the waves and the clouds
    /// always drift the same way as each other.
    pub fn start_sky(&mut self, rng: &mut Rng) {
        self.drift = if weighted(rng, &[50]) == 1 { -1 } else { 1 };
        self.clouds.clear();
        self.spawn_clouds(rng);
    }

    /// Walk the empty slots and roll for each. Sparse by design: fifteen slots
    /// at thirty percent between them is four or five clouds, which is what
    /// the painted sky in `ISLETEMP.SCR` has.
    fn spawn_clouds(&mut self, rng: &mut Rng) {
        for _ in self.clouds.len()..CLOUD_SLOTS {
            let pick = weighted(rng, &[CLOUDS[2].2, CLOUDS[1].2, CLOUDS[0].2]);
            if pick == 0 {
                continue;
            }
            // The roll runs biggest first and falls through to the next size
            // down when that one is at its cap (`seg3:0x0AC9`).
            for kind in (0..=3 - pick).rev() {
                if self.count_clouds(kind) >= CLOUDS[kind].1 as usize {
                    continue;
                }
                if let Some(c) = self.spawn_cloud(kind, rng) {
                    self.clouds.push(c);
                }
                break;
            }
        }
    }

    fn count_clouds(&self, kind: usize) -> usize {
        self.clouds.iter().filter(|c| c.kind == kind).count()
    }

    fn spawn_cloud(&self, kind: usize, rng: &mut Rng) -> Option<Cloud> {
        let sprite = self.pieces.get(CLOUDS[kind].0)?;
        let (bx, by, sx, sy) = CLOUD_BAND;
        let sx = sx - sprite.width as i32;
        let sy = sy - sprite.height as i32;
        if sx < 0 || sy < 0 {
            return None;
        }
        let speed = (rng.next_u32() % 5) as u16 + 6;
        Some(Cloud {
            kind,
            x: bx + span_roll(rng, sx),
            y: by + span_roll(rng, sy),
            speed,
            left: (rng.next_u32() % speed as u32) as u16,
        })
    }

    /// Drift the sky along. A cloud moves one pixel every `speed` passes and
    /// is gone once it has left the screen (`seg3:0x0CF7`); the slot it frees
    /// is filled again by the next spawn pass.
    fn step_clouds(&mut self, now: u32, rng: &mut Rng) -> bool {
        if now < self.cloud_next {
            return false;
        }
        self.cloud_next = now + 1;
        let drift = self.drift;
        let mut moved = false;
        for c in self.clouds.iter_mut() {
            let due = c.left == 0;
            c.left = if due { c.speed } else { c.left - 1 };
            if due {
                c.x += drift;
                moved = true;
            }
        }
        let widths: Vec<i32> = CLOUDS
            .iter()
            .map(|(f, _, _)| self.pieces.get(*f).map_or(0, |p| p.width as i32))
            .collect();
        let before = self.clouds.len();
        self.clouds
            .retain(|c| c.x + widths[c.kind] >= 0 && c.x <= SCREEN_W as i32);
        if self.clouds.len() != before {
            self.spawn_clouds(rng);
            moved = true;
        }
        moved
    }

    /// Lay the island down over the sea again: the pieces, then the raft.
    ///
    /// `seg3:0x0262` draws the island, the two optional pieces, the scrub and
    /// the palm; `seg3:0x1D69` puts the raft on afterwards.
    fn rebuild(&mut self) {
        let mut surface = Surface::new(self.sea_only.width, self.sea_only.height);
        surface.draw_background(&self.sea_only);
        if !self.backdrop.cutaway {
            let rock = self.extras && !self.backdrop.no_rock;
            for &(frame, x, y, always) in ISLAND.iter() {
                if !always && !rock {
                    continue;
                }
                if frame == 14 && !draw_shells() {
                    continue;
                }
                if let Some(f) = self.pieces.get(frame) {
                    surface.draw_sprite(f, x + ORIGIN.0, y + ORIGIN.1, Mirror::default());
                }
            }
            if !self.backdrop.no_raft {
                if let Some(f) = self.raft.as_ref() {
                    let (x, y) = RAFT_AT[rock as usize];
                    surface.draw_sprite(f, x + ORIGIN.0, y + ORIGIN.1, Mirror::default());
                }
            }
        } else {
            // `seg3:0x01C4` clears the whole 640x480 to colour 5 rather than
            // drawing a sea at all.
            surface.fill(crate::render::BLACK);
        }
        self.base.pixels = surface.pixels;
    }

    /// The story moved on, or a scene wants the island a particular way.
    /// Returns whether anything actually changed.
    pub fn set_story(&mut self, archive: &Archive, day: u8, backdrop: Backdrop) -> bool {
        if self.day == day && self.backdrop == backdrop {
            return false;
        }
        if raft_frame(day) != raft_frame(self.day) {
            self.raft = archive
                .get("MRAFT.BMP")
                .and_then(|b| dgds::read_bitmap(b).ok())
                .and_then(|frames| frames.into_iter().nth(raft_frame(day)));
        }
        self.day = day;
        self.backdrop = backdrop;
        self.rebuild();
        true
    }

    /// Step the surf if its 64ms is up. Returns whether anything moved.
    ///
    /// `seg3:0x0E9C`: only one counter moves per step, and which one depends
    /// on how they currently sit relative to each other, so the three patches
    /// they drive stay out of phase. From `[2, 2, 2]` it runs a nine-step
    /// cycle -- 576ms for the shoreline to come back round.
    pub fn tick(&mut self, now: u32, rng: &mut Rng) -> bool {
        let moved = self.step_waves(now, rng) | self.step_clouds(now, rng);
        if now < self.next {
            return moved;
        }
        self.next = now + SURF_TICKS;
        let [c0, c1, c2] = self.counters;
        if c0 == c2 {
            self.counters[0] += 1;
        } else if c1 != c0 {
            self.counters[1] += 1;
        } else {
            self.counters[2] += 1;
        }
        for c in self.counters.iter_mut() {
            if *c > 2 {
                *c = 0;
            }
        }
        true
    }

    /// How many waves are on the water, for the tools that report it.
    pub fn wave_count(&self) -> usize {
        self.waves.len()
    }

    /// How many clouds are in the sky.
    pub fn cloud_count(&self) -> usize {
        self.clouds.len()
    }

    /// How many glints are on the water.
    pub fn glint_count(&self) -> usize {
        self.glints.len()
    }

    /// Try to put another wave on the water, then move the ones already there.
    ///
    /// `seg3:0x04BD` walks the object array once per pass. An empty slot gets
    /// a 35% roll to be filled at all (`seg3:0x04CE`), and if it wins, a
    /// second weighted roll picks what goes in it: 35% a glint, 5% each for
    /// the three wave sizes, 50% nothing after all. A live slot moves on when
    /// its 160ms is up.
    fn step_waves(&mut self, now: u32, rng: &mut Rng) -> bool {
        let mut moved = false;

        // `seg3:0x04E4` will not spawn anything unless `ds:0x30BE` is over
        // 100. That is a speed index -- segment 2 times a calibration loop
        // and divides -- so on a 386 the extra objects were simply skipped.
        // Any machine running this port is over the line.
        if weighted(rng, &[35]) == 1 {
            // Weights from `seg3:0x04EE`: a glint, then the three wave sizes.
            match weighted(rng, &[35, 5, 5, 5]) {
                1 => {
                    if self.glints.len() < GLINTS {
                        if let Some(g) = self.spawn_glint(rng) {
                            self.glints.push(g);
                            moved = true;
                        }
                    }
                }
                pick @ 2..=4 => {
                    let kind = pick - 2;
                    let live = self.waves.iter().filter(|w| w.kind == kind).count();
                    if live < KINDS[kind].budget as usize {
                        if let Some(w) = self.spawn(kind, rng) {
                            self.waves.push(w);
                            moved = true;
                        }
                    }
                }
                _ => {}
            }
        }

        if now < self.wave_next {
            return moved;
        }
        self.wave_next = now + WAVE_TICKS;
        let drift = self.drift;
        for w in self.waves.iter_mut() {
            w.x += drift;
            let due = w.left == 0;
            w.left = w.left.wrapping_sub(1);
            if due {
                w.left = w.hold;
                w.cursor += 1;
            }
        }
        let before = self.waves.len();
        self.waves
            .retain(|w| w.cursor < KINDS[w.kind].sequence.len());
        let glints_before = self.glints.len();
        for g in self.glints.iter_mut() {
            g.left = g.left.saturating_sub(1);
        }
        self.glints.retain(|g| g.left > 0);
        moved || self.waves.len() != before || self.glints.len() != glints_before
    }

    /// Put a glint somewhere on the water, unless it lands on the palm.
    fn spawn_glint(&self, rng: &mut Rng) -> Option<Glint> {
        let (bx, by, sx, sy) = GLINT_BAND;
        let x = bx + span_roll(rng, sx);
        let y = by + span_roll(rng, sy);
        // Two pixels wide, so the test is the point against each rectangle.
        if NOT_WATER
            .iter()
            .any(|&(rx, ry, rw, rh)| x + 1 >= rx && x < rx + rw && y >= ry && y < ry + rh)
        {
            return None;
        }
        Some(Glint {
            x,
            y,
            // `seg3:0x0524`: ten to thirty-nine steps.
            left: (rng.next_u32() % 30) as u16 + 10,
        })
    }

    /// Place a new wave inside its band. `seg3:0x14BF` picks the spot, and
    /// takes the sprite's own size off the span so it lands whole.
    fn spawn(&self, kind: usize, rng: &mut Rng) -> Option<Wave> {
        let k = &KINDS[kind];
        let sprite = self.pieces.get(k.sequence[0])?;
        let sx = k.span.0 - sprite.width as i32;
        let sy = k.span.1 - sprite.height as i32;
        if sx < 0 || sy < 0 {
            return None;
        }
        // `seg4:0x1F1B(30, 50, 0)`: 30% one step of hold, 50% two, 20% none.
        let hold = match weighted(rng, &[30, 50]) {
            0 => 0,
            n => n as u16,
        };
        Some(Wave {
            kind,
            x: k.base.0 + span_roll(rng, sx),
            y: k.base.1 + span_roll(rng, sy),
            cursor: 0,
            hold,
            left: hold,
        })
    }

    /// The backdrop as it stands: island, then this step's surf.
    pub fn compose(&self) -> Frame {
        let mut surface = Surface::new(self.base.width, self.base.height);
        surface.draw_background(&self.base);
        // Clouds first: they are up in the sky, behind everything.
        for c in self.clouds.iter() {
            if let Some(f) = self.pieces.get(CLOUDS[c.kind].0) {
                surface.draw_sprite(f, c.x, c.y, Mirror::default());
            }
        }
        let sandbar = self.extras && !self.backdrop.no_rock;
        for &(x, y, first, counter, sandbar_only) in SURF.iter() {
            if sandbar_only && !sandbar {
                continue;
            }
            let frame = first + self.counters[counter] as usize;
            if let Some(f) = self.pieces.get(frame) {
                surface.draw_sprite(f, x + ORIGIN.0, y + ORIGIN.1, Mirror::default());
            }
        }
        for w in self.waves.iter() {
            let frame = KINDS[w.kind].sequence[w.cursor.min(6)];
            if let Some(f) = self.pieces.get(frame) {
                surface.draw_sprite(f, w.x, w.y, Mirror::default());
            }
        }
        for g in self.glints.iter() {
            surface.draw_line(g.x, g.y, g.x + 1, g.y, GLINT_COLOUR);
        }
        Frame {
            width: self.base.width,
            height: self.base.height,
            pixels: surface.pixels,
        }
    }
}

/// Centre a background that is not screen-sized, rather than letting
/// `draw_background`'s clamp decide.
fn fit_to_screen(src: &Frame) -> Frame {
    let mut surface = Surface::new(SCREEN_W, SCREEN_H);
    surface.draw_background(src);
    Frame {
        width: SCREEN_W,
        height: SCREEN_H,
        pixels: surface.pixels,
    }
}

/// The engine's weighted choice, from `seg4:0x1F1B`.
///
/// The arguments are percentages. It draws a 16-bit random number and walks
/// the list scaling each weight by 655 -- a hundredth of 65536 -- returning
/// the 1-based position of the one it lands on, or 0 if it runs off the end.
fn weighted(rng: &mut Rng, weights: &[u32]) -> usize {
    let mut roll = rng.next_u32() & 0xFFFF;
    for (i, &w) in weights.iter().enumerate() {
        if w == 0 {
            break;
        }
        let scaled = w * 655;
        if scaled > roll {
            return i + 1;
        }
        roll -= scaled;
    }
    0
}

/// `rand() % span`, and zero when there is no room to move.
fn span_roll(rng: &mut Rng, span: i32) -> i32 {
    if span <= 0 {
        0
    } else {
        (rng.next_u32() % span as u32) as i32
    }
}
