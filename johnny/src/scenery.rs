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
/// The two optional pieces are the rock and the sandbar; `seg3:0x027E` draws
/// them only when the coin flip came up 1.
const ISLAND: [(usize, i32, i32, bool); 6] = [
    // frame,  x,   y,   always
    (0, 0x121, 0x117, true),  // seg3:0x0270 -- the island itself, 280x52
    (1, 0x0FA, 0x12F, false), // seg3:0x0293 -- 384x69
    (2, 0x097, 0x148, false), // seg3:0x02B0 -- 64x30
    (14, 0x18D, 0x117, true), // seg3:0x02EA -- the scrub, 104x28
    (13, 0x1BB, 0x094, true), // seg3:0x0307 -- the palm trunk, 24x145
    (12, 0x16E, 0x07A, true), // seg3:0x0324 -- the fronds, 152x69
];

/// The surf, as `(x, y, first frame, counter)`. Each patch has three frames;
/// which one is showing comes from one of the three counters at `ds:0x0282`,
/// `ds:0x0284` and `ds:0x0286`.
const SURF: [(i32, i32, usize, usize); 7] = [
    (0x10F, 0x132, 3, 0),  // seg3:0x13A0
    (0x16D, 0x13F, 6, 1),  // seg3:0x13C9
    (0x207, 0x12F, 9, 2),  // seg3:0x13F2
    (0x0EA, 0x143, 30, 0), // seg3:0x141B
    (0x170, 0x164, 33, 1), // seg3:0x1444
    (0x22F, 0x143, 36, 2), // seg3:0x146D
    (0x082, 0x154, 39, 0), // seg3:0x1496
];

/// The three sea frames, in the order the pointer table at `ds:0x0262` has
/// them. `seg3:0x01F5` draws `rand() % 3`.
pub const SEAS: [&str; 3] = ["OCEAN00.SCR", "OCEAN01.SCR", "OCEAN02.SCR"];
pub const NIGHT_SEA: &str = "NIGHT.SCR";

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
    pub sea: &'static str,
    pub extras: bool,
}

impl Scenery {
    /// Compose the backdrop. `night` picks `NIGHT.SCR` over the oceans,
    /// `roll` supplies the two random choices: which sea, and whether the
    /// optional pieces are there.
    pub fn build(archive: &Archive, night: bool, roll: u32, now: u32) -> Option<Scenery> {
        let sea = if night {
            NIGHT_SEA
        } else {
            SEAS[(roll % 3) as usize]
        };
        let mut base = dgds::read_screen(archive.get(sea)?).ok()?;
        // `NIGHT.SCR` and the oceans are 640x480, the size of the screen. If
        // one ever is not, pad rather than leave the surface ragged.
        if base.width != SCREEN_W || base.height != SCREEN_H {
            base = fit_to_screen(&base);
        }
        for p in base.pixels.iter_mut() {
            if *p == 0 {
                *p = SEA_KEY_AS;
            }
        }

        let pieces = archive
            .get("BACKGRND.BMP")
            .and_then(|b| dgds::read_bitmap(b).ok())
            .unwrap_or_default();

        let extras = roll / 3 % 2 == 1;
        let mut surface = Surface::new(base.width, base.height);
        surface.draw_background(&base);
        for &(frame, x, y, always) in ISLAND.iter() {
            if !always && !extras {
                continue;
            }
            if let Some(f) = pieces.get(frame) {
                surface.draw_sprite(f, x, y, Mirror::default());
            }
        }
        base.pixels = surface.pixels;

        Some(Scenery {
            base,
            pieces,
            counters: [2, 2, 2],
            next: now,
            sea,
            extras,
        })
    }

    /// Step the surf if its 64ms is up. Returns whether anything moved.
    ///
    /// `seg3:0x0E9C`: only one counter moves per step, and which one depends
    /// on how they currently sit relative to each other, so the three patches
    /// they drive stay out of phase. From `[2, 2, 2]` it runs a nine-step
    /// cycle -- 576ms for the shoreline to come back round.
    pub fn tick(&mut self, now: u32) -> bool {
        if now < self.next {
            return false;
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

    /// The backdrop as it stands: island, then this step's surf.
    pub fn compose(&self) -> Frame {
        let mut surface = Surface::new(self.base.width, self.base.height);
        surface.draw_background(&self.base);
        for &(x, y, first, counter) in SURF.iter() {
            let frame = first + self.counters[counter] as usize;
            if let Some(f) = self.pieces.get(frame) {
                surface.draw_sprite(f, x, y, Mirror::default());
            }
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
