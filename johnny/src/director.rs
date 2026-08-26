//! The scheduler: what Johnny does next, and getting him there.
//!
//! This is the layer above the ADS sequencer. It picks a scene out of the
//! catalogue by weight, walks Johnny to wherever that scene needs him to be
//! standing, runs it, and keeps the clock and the story moving.
//!
//! All of it is read out of `SCRANTIC.EXE` segment 4; see `docs/SCHEDULER.md`
//! for the write-up and `tables.rs` for the data.

use crate::ads::{Action, Repeat, Rng, SceneState, Sequencer};
use crate::dgds::{self, Archive, Frame};
use crate::render::Mirror;
use crate::scenery::{self, Backdrop, Scenery};
use crate::tables::{self, Scene, WalkStep};
use crate::ttm::{self, Instance, Program, Stage};
use std::collections::HashMap;
use std::rc::Rc;

/// The day is sixteen phases (`seg4:0x0560`). Twelve onwards is night.
pub const PHASES_PER_DAY: u8 = 16;
pub const FIRST_NIGHT_PHASE: u8 = 12;

/// Where the island is drawn. `seg3:0x0262` adds these to a per-run offset
/// before blitting `BACKGRND.BMP`.
pub const ISLAND_X: i32 = 0x121;
pub const ISLAND_Y: i32 = 0x117;

/// The scene table's "no script" marker, in the ADS file id at `+0x0F`.
pub const NO_ADS_FILE: u8 = 0xFF;

/// The story runs for eleven days and then wraps (`seg4:0x0BC8`).
pub const STORY_DAYS: u8 = 11;

/// One tick, in milliseconds. This is the unit every delay in the data is
/// counted in, and getting it wrong is the difference between a screensaver
/// and a slideshow on fast-forward.
///
/// `seg2:0x1516` calls the timer setup with 13, and `seg9:0x0781` turns that
/// into `1000 / (13 * 18)` = 4ms per unit of its software clock. The one
/// timer the screensaver registers (`seg2:0x1522`) asks for an interval of 4
/// of those units, so its callback at `seg2:0x16CE` runs every 16ms. That
/// callback is the whole clock: it bumps the 32-bit tick counter at
/// `ds:0x2E72` and counts down the five delay slots.
pub const MS_PER_TICK: u32 = 16;

/// Milliseconds per tick after `JC_SPEED` is applied.
///
/// The port runs the data at the rate it was authored for, and that is
/// measurably right: `SJGLIMPS.TTM` scene 17 asks for 133 ticks of delay
/// across its twenty frames and the port takes 134. But a 386 could not hold
/// 62.5 passes a second -- the engine has a speed index at `ds:0x30BE` for
/// exactly that reason, and skipped passes push each frame's deadline later --
/// so the screensaver most people remember ran slower than its own data.
///
/// `JC_SPEED` is a percentage of the authored rate. The default is 60, which
/// is where it looks like the thing people remember; `JC_SPEED=100` runs the
/// data exactly as written.
pub const DEFAULT_SPEED: u32 = 60;

pub fn ms_per_tick() -> u32 {
    let pct = std::env::var("JC_SPEED")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(DEFAULT_SPEED)
        .clamp(10, 400);
    (MS_PER_TICK * 100 / pct).max(1)
}

/// 62.5, rounded. The authors clearly thought in 60ths -- the delays in the
/// scripts are 60, 120, 180, 240, 360 -- so a tick is near enough a 60th.
pub const TICKS_PER_SECOND: u32 = 1000 / MS_PER_TICK;

/// Ticks between walk waypoints. `seg4:0x039D` writes 4 into the delay slot at
/// `ds:0x30D6` after drawing each step of a walk, and `seg4:0x00A3` will not
/// draw the next one until it reaches zero. 64ms a frame.
pub const WALK_TICKS_PER_WAYPOINT: u32 = 4;

/// A backstop, not a schedule. Scenes end on their own: the sequence reaches
/// its `End` and the last scene that can finish does. This catches a script
/// the port has misread, so a bad scene costs three minutes rather than the
/// rest of the session.
///
/// The scene table's `cost` field is deliberately not used for this. The
/// chooser compares `cost / 2` against a ceiling when picking (`seg4:0x0D76`),
/// which makes it a price rather than a duration; what it is a price *in* is
/// still an open question.
pub const MAX_SCENE_TICKS: u32 = 180 * TICKS_PER_SECOND;

/// Category masks the chooser is called with, from the four call sites in
/// segment 4.
pub mod category {
    /// Scenes reachable from Johnny's position (`seg4:0x0A26`).
    pub const PRIMARY: u32 = 0x01;
    /// The second slot, same shape (`seg4:0x0A8A`).
    pub const SECONDARY: u32 = 0x02;
    /// Self-contained gags that end back at "anywhere" (`seg4:0x091C`).
    pub const STANDALONE: u32 = 0x04;
    /// The story. Not really a category: `seg4:0x0D9E` takes this one as a
    /// signal to roll one in ten and then match on the day instead
    /// (`seg4:0x08DC`).
    pub const STORY: u32 = 0x80;
}

/// What one queue may spend. `seg4:0x0640` passes 300 and every scene queued
/// takes its `cost` out of it; the queue closes when nothing affordable is
/// left. Scenes cost 5 to 105, so a queue is a handful of them.
pub const QUEUE_BUDGET: i32 = 300;

/// The queue's hard ceiling (`seg4:0x0B0F`). A backstop, not a target.
const QUEUE_CAP: usize = 0x12A;

/// The budget the standalone slot is picked against. `seg4:0x0D90` throws the
/// caller's away and uses this, so the gag that closes a queue is never
/// priced out of it.
const UNPRICED: i32 = 0x3E7;

/// How long a script-less queue entry holds Johnny still: `seg4:0x075B` rolls
/// `rand() % 240 + 180` ticks, which is three to seven seconds.
const IDLE_SPAN: u32 = 0xF0;
const IDLE_LEAST: u32 = 0xB4;

/// How long the title card stays up.
///
/// `seg2:0x14F0` puts `INTRO.SCR` on the screen and then gets on with the
/// loading -- `seg3:0x0136` for the backdrop, `seg4:0x0000` for the walk
/// sprites, `seg10:0x0000`, `seg5:0x0000` for the sounds -- and the island
/// replaces it whenever that finishes. On a 386 decoding a hundred-odd LZW
/// bitmaps that was a few seconds; here it is a few milliseconds, so the card
/// needs a length of its own or nobody would ever see it. `JC_INTRO` is that
/// length in ticks, and 0 skips it.
pub const INTRO_TICKS: u32 = 2 * 62;

/// The dawn the config dialog starts at (`ds:0x00BE`), as HHMM. Segment 2
/// writes this from the spinner in Screen Antics' settings.
pub const DEFAULT_DAWN: u16 = 900;

/// How the time of day is decided.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Clock {
    /// The original: half-hours since the dawn the user set, counted mod
    /// sixteen (`seg4:0x0540`). Johnny's day is eight real hours long, so
    /// with the default dawn of nine the light goes at three and comes back
    /// at five.
    Wall { dawn: u16 },
    /// One phase per queue, so a twenty-minute run still sees a whole day.
    /// The tools use this; nothing shipped does.
    PerQueue,
}

/// The phase a clock time falls in, from `seg4:0x0C48`.
///
/// `hhmm` is `hour * 100 + minute`, and the minute is only ever 0 or 30 --
/// `seg4:0x056B` rounds it down to the half hour before calling. The `+ 14`
/// and the `mod 16` cancel between the two calls in `seg4:0x0587`; what is
/// left is the number of half hours from dawn to now.
pub fn half_hours(hhmm: u16) -> i32 {
    let hhmm = hhmm as i32;
    let index = hhmm / 50 + (hhmm % 100) / 30;
    (index + 14) % 16
}

pub fn phase_at(hhmm: u16, dawn: u16) -> u8 {
    (((half_hours(hhmm) - half_hours(dawn)) + 16) % 16) as u8
}

/// A calendar special. `seg4:0x0560` checks the date and, when it matches,
/// `HOLIDAY.BMP` is drawn over the scene.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Holiday {
    Halloween,
    Christmas,
    NewYear,
    StPatricks,
}

impl Holiday {
    /// Its place in `scenery::HOLIDAY_AT`, which is the order `ds:0x30BC`
    /// numbers them in.
    pub fn index(self) -> usize {
        match self {
            Holiday::Halloween => 0,
            Holiday::Christmas => 1,
            Holiday::NewYear => 2,
            Holiday::StPatricks => 3,
        }
    }

    pub fn for_date(month: u8, day: u8) -> Option<Holiday> {
        match (month, day) {
            (10, 29..=31) => Some(Holiday::Halloween),
            (12, 23..=25) => Some(Holiday::Christmas),
            (12, 29..=31) | (1, 1) => Some(Holiday::NewYear),
            (3, 15..=17) => Some(Holiday::StPatricks),
            _ => None,
        }
    }
}

/// What Johnny is doing right now.
enum Activity {
    Idle,
    /// The title card, up while the island is got ready.
    Intro { until: u32 },
    /// Standing about, for one of the table's script-less entries.
    Idling { until: u32 },
    /// Following one walk path towards `target`.
    Walking {
        step: &'static WalkStep,
        index: usize,
        /// The area the journey started in. The route block is looked up once,
        /// on the pair, and re-read for wherever Johnny has got to since.
        origin: u8,
        target: (u8, u8),
        /// Tick at which the next waypoint is due.
        next: u32,
    },
    /// Running a scene's ADS sequence.
    Scene {
        /// The table row this came from, so the story can tell when its own
        /// scene has been shown.
        row: &'static Scene,
        seq: Box<Sequencer>,
        active: Vec<Active>,
        ends_at: (u8, u8),
        ticks: u32,
        /// The sequence has reached its `End`. What is still on screen plays
        /// out; nothing new will start.
        done: bool,
    },
}

/// One TTM scene the sequencer has started, and the state the engine keeps
/// for it at +0x2F of its instance record.
pub struct Active {
    pub ttm: u16,
    pub scene: u16,
    pub inst: Instance,
    /// Tick at which this instance's next frame is due.
    pub wake: u32,
    /// The run state, which is what the `If` predicates read.
    pub state: SceneState,
    /// Passes still owed, for a positive `flags` (the engine's +0x31).
    pub count: u16,
    /// Deadline, for a negative `flags` (the engine's +0x33).
    pub until: u32,
    /// The scene jumps back to its own start rather than ending: the engine's
    /// self-loop flag at +0xC, set at `seg10:0x1D9E` by comparing the goto
    /// target with the scene's own start.
    pub looping: bool,
}

impl Active {
    /// Begin a scene, in whatever repeat mode the sequencer asked for.
    pub fn start(ttm: u16, scene: u16, inst: Instance, repeat: Repeat, now: u32) -> Active {
        Active {
            ttm,
            scene,
            inst,
            wake: now,
            state: repeat.state(),
            count: repeat.count(),
            until: match repeat {
                Repeat::ForTicks(n) => now + n,
                _ => 0,
            },
            looping: false,
        }
    }

    /// Play this scene again while it is already on screen.
    ///
    /// `seg10:0x0DB6` looks for an instance of this scene first
    /// (`seg10:0x0BF4` walks the list), and when it finds one it writes only
    /// the run state and the repeat count -- the program pointer is left
    /// exactly where it was. So a second `PlayScene` on a scene that is
    /// already going re-arms it; it does not rewind it.
    ///
    /// Rewinding instead is quietly awful: a rule that fires on several
    /// passes running restarts the animation every 16ms, so it plays its
    /// first frame over and over, never reaches its end, and never lets the
    /// scene that is waiting on it start.
    pub fn rearm(&mut self, repeat: Repeat, now: u32) {
        self.state = repeat.state();
        self.count = repeat.count();
        self.until = match repeat {
            Repeat::ForTicks(n) => now + n,
            _ => 0,
        };
    }

    /// Does the runner advance this instance at all?
    ///
    /// `seg10:0x1C56` runs states 1, 2, 3 and 5 and no others. State 0 is a
    /// scene that has been stopped -- `StopScene` writes it straight into the
    /// instance record, and 111 places in the scripts do -- and state 4 is a
    /// scene that has just ended and is waiting one pass to be noticed.
    pub fn runnable(&self) -> bool {
        matches!(self.state, SceneState::Queued(_) | SceneState::Finished)
    }

    /// The scene's program reached its end. `seg10:0x1D2B` decides what that
    /// means from the run state:
    ///
    /// * queued for a count, it starts over until the count runs out
    /// * queued until a deadline, it starts over until the deadline passes
    /// * otherwise it becomes **over** -- state 4, which is not a state it
    ///   plays in. `seg10:0x1DDC` turns it idle on the very next pass, so it
    ///   lasts exactly one, and that one pass is the whole window in which a
    ///   `1350` predicate can see it.
    ///
    /// That handshake is what every script is built on: play a scene, and the
    /// block guarded on it fires on the pass it ends.
    pub fn program_ended(&mut self) -> bool {
        match self.state {
            SceneState::Queued(2) if self.count > 0 => {
                self.count -= 1;
                self.restart()
            }
            // The deadline is what ends this one; until it passes, keep going.
            SceneState::Queued(3) => self.restart(),
            _ => {
                self.state = SceneState::Over;
                true
            }
        }
    }

    fn restart(&mut self) -> bool {
        if self.inst.seek_scene(self.scene) {
            true
        } else {
            self.state = SceneState::Idle;
            false
        }
    }

    /// `seg10:0x1DEB`: a scene queued until a deadline is over once that
    /// deadline passes, and then ends on its next time round.
    pub fn check_deadline(&mut self, now: u32) {
        if self.state == SceneState::Queued(3) && now >= self.until {
            self.state = SceneState::Over;
        }
    }
}

/// Advance every live scene by one tick and settle the run states.
///
/// This is `seg10:0x1C3C`'s loop: walk the instance list, run a frame for each
/// one that is due, act on the end-of-scene flag, then hand the states back so
/// the sequencer's `If` predicates see them. Returns the scenes that ended.
pub fn advance_scenes(
    active: &mut Vec<Active>,
    seq: &mut Sequencer,
    stage: &mut Stage,
    archive: &Archive,
    now: u32,
) -> Vec<(u16, u16)> {
    let mut finished = Vec::new();
    for a in active.iter_mut() {
        // The sequencer and the runner share one field: the run state at
        // +0x2F of the instance record. `StopScene` writes it directly, so
        // read it back before deciding anything -- a scene the sequence has
        // stopped has to stop drawing.
        a.state = seq.state_of(a.ttm, a.scene);
        if !a.runnable() {
            // `seg10:0x1DDC`: a scene in state 4 has had its pass; it is over.
            finished.push((a.ttm, a.scene));
            continue;
        }
        if a.wake > now {
            // Not its turn to move, but it is still on screen.
            a.inst.repaint(stage, archive);
        } else {
            let ended = match a.inst.run_frame(stage, archive) {
                ttm::Step::Finished => true,
                ttm::Step::Frame(delay) => {
                    a.wake = now + delay.max(1) as u32;
                    // The frame carried an `EndScene`, which is how a scene
                    // says it has reached its end -- it does not stop
                    // mid-frame, it sets a flag the runner reads afterwards
                    // (`seg10:0x1D1A`). Every scene in the data ends this way;
                    // almost none simply run out of instructions.
                    if a.inst.last_goto == Some(a.scene) {
                        a.looping = true;
                    }
                    a.inst.ended
                }
            };
            if ended && !a.program_ended() {
                finished.push((a.ttm, a.scene));
            }
        }
        a.check_deadline(now);
        seq.set_state(a.ttm, a.scene, a.state);
    }
    for (t, s) in &finished {
        seq.set_state(*t, *s, SceneState::Idle);
    }
    active.retain(|a| !finished.contains(&(a.ttm, a.scene)));
    finished
}

/// The wall clock as HHMM. `seg1:0x020F` is the C runtime's `gettime`; this
/// is the same idea with whatever this process can see, and the host may know
/// better -- `johnny.scr` asks Windows for the local time and sets `hhmm`.
fn since_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub fn wall_hhmm() -> u16 {
    let secs = since_epoch() % 86_400;
    (secs / 3600 * 100 + secs % 3600 / 60) as u16
}

/// Days since the epoch. The original compares day, month and year
/// (`seg4:0x0BBE`); one number does the same job.
pub fn today() -> u32 {
    (since_epoch() / 86_400) as u32
}

/// A calendar date to days since the epoch, so a test can ask for a holiday.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> u32 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) as u32
}

/// Days since the epoch back to a calendar month and day, which is what the
/// holiday check wants (`seg4:0x0560` reads them out of the C runtime's
/// `getdate`). The usual civil-from-days arithmetic, shifted to a March-based
/// year so the leap day falls at the end.
pub fn month_and_day(days: u32) -> (u8, u8) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let _ = era;
    (month, day)
}

/// Two table rows are the same row. The engine compares the pointers it
/// stored in `ds:0x30E8` and `ds:0x30EA`; so does this.
fn same(a: Option<&'static Scene>, b: &'static Scene) -> bool {
    a.is_some_and(|a| std::ptr::eq(a, b))
}

pub struct Director {
    /// Where Johnny is standing, as (major, minor).
    pub position: (u8, u8),
    /// Time-of-day phase, 0..15.
    pub phase: u8,
    /// Story day, 1..11.
    pub day: u8,
    pub holiday: Option<Holiday>,
    /// Draw against the summed weights rather than a hardcoded 100 when
    /// routing a walk. The original uses 100, which makes one route's last
    /// two options unreachable; see `docs/SCHEDULER.md`.
    pub faithful_router: bool,
    pub rng: Rng,
    pub stage: Stage,
    activity: Activity,
    programs: HashMap<String, Rc<Program>>,
    /// JOHNWALK.BMP, decoded once. `FILES.VIN` preloads it, and decoding it
    /// per walk frame is 36 LZW passes a tick.
    walk_frames: Option<Rc<Vec<Frame>>>,
    /// The walk frame Johnny is standing in, so he can be put back after the
    /// surf repaints the backdrop underneath him. The engine leaves him where
    /// he was drawn; this port repaints, so it has to draw him again.
    standing: Option<(u16, u16, u16, Mirror)>,
    /// The sea, the island and the surf. `None` until the first tick loads it.
    scenery: Option<Scenery>,
    /// Whether the backdrop on show is the night one, so nightfall can be
    /// noticed. `NIGHT.SCR` replaces the oceans from phase 12 (`seg3:0x01EE`),
    /// and the backdrop is only rebuilt between scenes.
    scenery_night: bool,
    /// The background generation the island backdrop was written at. While
    /// the stage still reports it, the island is what is on show and the surf
    /// may keep repainting it; once a scene loads its own backdrop the number
    /// moves on and the surf leaves it alone.
    scenery_gen: u64,
    tick: u32,
    /// ADS opcodes the sequencer met and does not implement, kept across
    /// scenes so a long run can say whether any are ever reached.
    pub ads_todo: std::collections::BTreeMap<u16, usize>,
    pub log: Vec<String>,

    // --- the outer loop, `seg4:0x0540` and what it keeps ------------------
    /// The scenes lined up and how far through them we are (`ds:0x30F4`,
    /// `ds:0x30F2`).
    queue: Vec<&'static Scene>,
    cursor: usize,
    /// Where the island is allowed to stand this time round (`ds:0x30C0`).
    /// The queue's scenes vote on it as they are picked, and a scene that
    /// disagrees with the votes already cast cannot join.
    island_state: u16,
    /// What the queue's scenes between them want the island to look like
    /// (`ds:0x30B0`-`ds:0x30B6`, `ds:0x30C2`).
    backdrop: Backdrop,
    /// The last standalone and the last companion, so neither comes round
    /// twice running (`ds:0x30E8`, `ds:0x30EA`).
    last_gag: Option<&'static Scene>,
    last_mate: Option<&'static Scene>,
    /// The story scene in this queue, if the one-in-ten came up
    /// (`ds:0x30EC`), and how far it has got: 0 not offered, 1 queued,
    /// 2 played, 3 counted (`ds:0x30EE`).
    story_scene: Option<&'static Scene>,
    story_state: u8,
    /// The story day on offer, which runs one ahead of the day reached
    /// (`ds:0x2D9B` against `ds:0x2D9C`, which is `day`).
    story_offered: u8,
    /// The calendar day the story last moved on (`ds:0x2D95`-`ds:0x2D99`),
    /// and today's, as days since the epoch.
    story_date: Option<u32>,
    pub date: u32,
    /// Whether the queue that just ended ran itself out rather than being
    /// abandoned (`ds:0x30F0`).
    queue_spent: bool,
    /// Where the time of day comes from.
    pub clock: Clock,
    /// The wall clock as HHMM, when the host knows it better than this
    /// process does -- `johnny.scr` reads the local time, the tools do not.
    pub hhmm: Option<u16>,
}

impl Director {
    pub fn new(seed: u64) -> Self {
        Director {
            // Johnny starts where the STAND scenes put him.
            position: (1, 1),
            phase: 0,
            day: 1,
            holiday: None,
            faithful_router: false,
            rng: Rng::new(seed),
            stage: Stage::new(),
            activity: Activity::Intro {
                until: std::env::var("JC_INTRO")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(INTRO_TICKS),
            },
            programs: HashMap::new(),
            walk_frames: None,
            standing: None,
            scenery: None,
            scenery_night: false,
            scenery_gen: 0,
            tick: 0,
            ads_todo: Default::default(),
            log: Vec::new(),
            queue: Vec::new(),
            cursor: 0,
            island_state: scenery::ANYWHERE,
            backdrop: Backdrop::default(),
            last_gag: None,
            last_mate: None,
            story_scene: None,
            story_state: 0,
            story_offered: 1,
            story_date: None,
            date: today(),
            queue_spent: false,
            clock: Clock::Wall { dawn: DEFAULT_DAWN },
            hhmm: None,
        }
    }

    fn note(&mut self, line: String) {
        self.log.push(line);
    }

    /// Does this scene's declared position match where Johnny is?
    ///
    /// `seg4:0x0B64` treats a major of 0 as "don't care", which is what the
    /// `(0, 0)` entries in the table mean.
    /// `seg4:0x0B64`: a required major of 0 means anywhere, and a required
    /// minor of -1 means any spot within that area. The two are tested
    /// independently -- no scene in the table uses the minor wildcard, but the
    /// engine allows it.
    fn position_ok(&self, scene: &Scene) -> bool {
        let (major, minor) = scene.from;
        // A required area of 0 is anywhere, and there is nowhere to walk to,
        // so the spot inside it does not apply either. Read strictly,
        // `seg4:0x0B8F` would still want the minor to match -- only -1 is its
        // wildcard -- but all eight rows with no area also ask for spot 0, and
        // "anywhere, but standing at spot 0" is not a place.
        if major == 0 {
            return true;
        }
        major == self.position.0 && (minor == 0xFF || minor == self.position.1)
    }

    /// The chooser at `seg4:0x0D76`: filter, sum the weights, draw against the
    /// total.
    ///
    /// The story path is a category in name only. `category::STORY` rolls one
    /// in ten, and if it comes up it takes whichever scene carries today's
    /// story day, never mind the hour, the price or where the island is.
    pub fn choose(&mut self, category: u32, budget: i32) -> Option<&'static Scene> {
        // `seg4:0x0D8A`: the standalone slot is picked without regard to cost.
        let budget = if category == category::STANDALONE {
            UNPRICED
        } else {
            budget
        };
        if category == category::STORY {
            if self.rng.below(10) != 0 {
                return None;
            }
            // `seg4:0x0DB9`: the day on offer catches up to the day reached.
            self.story_offered = self.day;
        }
        let mut candidates: Vec<&'static Scene> = Vec::new();
        let mut total = 0u32;
        for scene in tables::SCENES {
            if scene.sequence == 0 {
                continue;
            }
            let wanted = if category == category::STORY && scene.day == self.story_offered {
                true
            } else {
                scene.mask & category != 0
                    && scene.day == 0
                    && self.phase >= scene.min_phase
                    && self.phase < scene.max_phase
                    && (scene.cost as i32) / 2 < budget
                    && self.island_allows(scene.mask)
            };
            if wanted {
                total += scene.weight as u32;
                candidates.push(scene);
            }
        }
        if total == 0 {
            return None;
        }
        let mut roll = self.rng.below(total) as i64 + 1;
        for scene in candidates {
            roll -= scene.weight as i64;
            if roll <= 0 {
                return Some(scene);
            }
        }
        None
    }

    /// Can this scene still agree with the queue about where the island is?
    ///
    /// `seg4:0x0EDB`. The six state bits are three pairs, and each pair has a
    /// left member and a right member: the two pins first, then the four
    /// quadrants. A scene that wants the island on one side cannot join a
    /// queue that has already claimed the other, and a scene that wants a
    /// single quadrant cannot join one that has claimed the opposite half.
    fn island_allows(&self, mask: u32) -> bool {
        let state = self.island_state;
        if state == scenery::ANYWHERE {
            return true;
        }
        let want = (mask >> 16) as u16;
        let w = |b: u16| want & b != 0;
        let s = |b: u16| state & b != 0;
        // 0x15 is the three left bits, 0x2A the three right ones, 0x30 the
        // bottom two quadrants and 0x0C the top two.
        if w(0x20) && s(0x2A) {
            return false;
        }
        if w(0x10) && s(0x15) {
            return false;
        }
        if w(0x01) && ((!w(0x02) && s(0x2A)) || (!w(0x04) && s(0x30))) {
            return false;
        }
        if w(0x02) && ((!w(0x01) && s(0x15)) || (!w(0x08) && s(0x30))) {
            return false;
        }
        if w(0x04) && ((!w(0x08) && s(0x2A)) || (!w(0x01) && s(0x0C))) {
            return false;
        }
        if w(0x08) && ((!w(0x04) && s(0x15)) || (!w(0x02) && s(0x0C))) {
            return false;
        }
        true
    }

    /// Take a scene's wishes for the island (`seg3:0x1C41`).
    ///
    /// The low word is what the island looks like; the high word is where it
    /// may stand. Two bits of the high word nail it to a spot and four are
    /// quadrants, and they accumulate: the first scene to ask for anything
    /// clears the "anywhere" marker and every scene after it narrows the
    /// choice further.
    fn take_wishes(&mut self, mask: u32) {
        self.backdrop.apply(mask);
        let want = (mask >> 16) as u16;
        if self.island_state == scenery::ANYWHERE && want & 0x3F != 0 {
            self.island_state = 0;
        }
        if want & 0x20 != 0 {
            self.island_state = 1;
        } else if want & 0x10 != 0 {
            self.island_state = 2;
        }
        for (bit, quadrant) in [(1u16, 4u16), (2, 8), (4, 0x10), (8, 0x20)] {
            if want & bit != 0 {
                self.island_state |= quadrant;
            }
        }
    }

    /// Build the next queue of scenes (`seg4:0x08B9`).
    ///
    /// A queue is not a list of things to do next; it is a set of scenes that
    /// have agreed with each other about where the island is standing, put in
    /// an order. The engine builds one, places the island to suit it, and then
    /// plays it out before thinking again.
    ///
    /// The shape is: roll for the story, pick the gag that will close the
    /// queue, then spend three hundred units of allowance on whatever fits in
    /// between. `cost` is the price, and it is the only thing that ends a
    /// queue early.
    fn build_queue(&mut self) {
        let mut budget = QUEUE_BUDGET;
        self.queue.clear();
        self.cursor = 0;

        // The story, one time in ten. It goes in whichever of the two slots
        // its own category says, and it is dropped if it is one of the two
        // that ran last time.
        let (mut gag, mut mate) = (None, None);
        self.story_scene = None;
        if let Some(story) = self.choose(category::STORY, 0) {
            let repeat = same(self.last_gag, story) || same(self.last_mate, story);
            if !repeat {
                if story.mask & category::STANDALONE != 0 {
                    gag = Some(story);
                } else {
                    mate = Some(story);
                }
                self.story_scene = Some(story);
            }
        }

        // The closing gag. When there is a companion scene as well, the gag
        // has to be one that does not also stand on its own -- `seg4:0x0947`
        // keeps drawing until it finds one.
        let mut guard = 0;
        loop {
            let bad = match gag {
                None => true,
                Some(g) => {
                    same(self.last_gag, g)
                        || (mate.is_some() && g.mask & category::PRIMARY != 0)
                }
            };
            if !bad {
                break;
            }
            guard += 1;
            if guard > 1000 {
                // Nothing in the table can satisfy this phase. Say so rather
                // than spin; the caller falls back to an empty queue.
                self.note("no standalone scene fits this phase".into());
                return;
            }
            gag = self.choose(category::STANDALONE, 0);
        }
        let Some(gag) = gag else { return };

        budget -= gag.cost as i32;
        self.take_wishes(gag.mask);
        self.last_gag = Some(gag);
        let mut mate_pending = mate.is_some();
        if let Some(m) = mate {
            budget -= m.cost as i32;
            self.take_wishes(m.mask);
        }
        self.last_mate = mate;
        // `seg4:0x099B`: is the story in this queue at all?
        if [gag].iter().chain(mate.iter()).any(|s| s.day == self.story_offered) {
            self.story_state = 1;
        }

        // A gag that also stands as a primary takes the whole queue. The four
        // scenes that do this are the cutaways -- the office, Suzy's flat --
        // and they own the screen while they run.
        if gag.mask & category::PRIMARY != 0 {
            self.queue.push(gag);
            return;
        }

        // The opening scene, which is also the one that says where Johnny is
        // standing when the queue starts.
        if mate_pending && mate.is_some_and(|m| m.mask & category::PRIMARY != 0) && self.coin(50)
        {
            self.queue.push(mate.unwrap());
            mate_pending = false;
        } else if let Some(first) = self.choose(category::PRIMARY, budget) {
            budget -= first.cost as i32;
            self.take_wishes(first.mask);
            self.queue.push(first);
        } else {
            // `seg4:0x0A2D` stores whatever came back without looking. With
            // this table something always does; if it ever did not, the
            // original would read a scene out of the start of its own data
            // segment. Stop instead.
            self.note("no primary scene fits this phase".into());
            self.queue.push(gag);
            return;
        }

        // Then fill the time. Every so often the companion scene drops in
        // here instead; the rest is whatever the second category offers, and
        // the repeatable ones go in several times over.
        while budget != 0 && self.queue.len() < QUEUE_CAP {
            if mate_pending && self.coin(5) {
                self.queue.push(mate.unwrap());
                mate_pending = false;
                continue;
            }
            let Some(next) = self.choose(category::SECONDARY, budget) else {
                break;
            };
            let mut repeats = 1;
            // `seg4:0x0AAA`: one to six goes, weighted towards three, redrawn
            // until the queue can afford however many it asked for.
            if next.mask & 0x08 != 0 {
                loop {
                    let n = self.weighted(&[10, 20, 30, 20, 10, 10]) as i32;
                    if n == 0 {
                        continue;
                    }
                    if n <= 1 || next.cost as i32 * n <= budget {
                        repeats = n;
                        break;
                    }
                }
            }
            for _ in 0..repeats {
                if self.queue.len() >= QUEUE_CAP {
                    break;
                }
                self.queue.push(next);
                budget -= next.cost as i32;
                self.take_wishes(next.mask);
            }
        }
        // The gag closes the queue.
        self.queue.push(gag);
    }

    /// `seg4:0x1F1B` with one weight: true that many times in a hundred.
    fn coin(&mut self, percent: u32) -> bool {
        self.weighted(&[percent]) == 1
    }

    /// The engine's weighted choice (`seg4:0x1F1B`): a 16-bit draw against
    /// percentages scaled by 655, returning the 1-based position it lands on
    /// or 0 if it runs off the end.
    fn weighted(&mut self, weights: &[u32]) -> usize {
        let mut roll = self.rng.next_u32() & 0xFFFF;
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

    /// Start the next queue: the clock, the story, the scenes, the island
    /// (`seg4:0x0540`).
    fn next_queue(&mut self, archive: &Archive) {
        self.advance_phase();
        // `seg4:0x05B5` checks the calendar every time it builds a queue.
        let (month, day) = month_and_day(self.date);
        self.holiday = Holiday::for_date(month, day);
        // `seg3:0x1C41(-1, -1)`: the island forgets what the last queue
        // wanted before the next one starts asking.
        self.island_state = scenery::ANYWHERE;
        self.backdrop = Backdrop::default();
        self.advance_story();
        self.build_queue();
        // `seg4:0x064D`: Johnny is simply put where the first scene needs
        // him. He walks between the scenes of a queue, never into one.
        if let Some(first) = self.queue.first() {
            if first.from.0 != 0 {
                self.position = first.from;
            }
        }
        self.cursor = 0;
        self.load_scenery(archive);
        let names: Vec<String> = self
            .queue
            .iter()
            .map(|s| format!("{}:{}", s.ads, s.sequence))
            .collect();
        self.note(format!(
            "queue phase {} day {} island {:?} {:?}: {}",
            self.phase,
            self.day,
            self.island_state,
            self.backdrop,
            names.join(" ")
        ));
    }

    /// Pick the next walk step towards `target`, from `seg4:0x0094`.
    ///
    /// The route block is chosen once, on the pair Johnny set out on
    /// (`seg4:0x03AC` indexes a six by six table of them), and every step of
    /// the journey re-reads that same block for the area he is standing in
    /// now (`seg4:0x00EA` scans it for a `-area` marker). That matters: a
    /// block's own mid-journey sections are the ones that bring him in, while
    /// looking the pair up afresh each leg would offer him the full spread of
    /// ways to set out again -- which is a walk that wanders for half a
    /// minute rather than one that arrives.
    fn next_step(&mut self, origin: u8, target: (u8, u8)) -> Option<&'static WalkStep> {
        let (at, to) = (self.position.0, target.0);
        if at == to || origin == 0 || to == 0 {
            return None;
        }
        let options = tables::ROUTE_TABLE
            .iter()
            .find(|(f, t, a, _)| *f == origin && *t == to && *a == at)
            .map(|(_, _, _, o)| *o)?;
        let total: u32 = options.iter().map(|o| o.weight as u32).sum();
        // The original draws rand() % 100 here regardless of the total, which
        // is why one route's last two options never fire.
        let span = if self.faithful_router { 100 } else { total };
        let mut roll = self.rng.below(span) as i64 + 1;
        let mut chosen = None;
        for opt in options {
            roll -= opt.weight as i64;
            if roll <= 0 {
                chosen = Some(opt.step);
                break;
            }
        }
        let step = chosen?;
        tables::WALK_STEPS.iter().find(|w| w.id == step)
    }

    fn program(&mut self, archive: &Archive, name: &str) -> Option<Rc<Program>> {
        if let Some(p) = self.programs.get(name) {
            return Some(Rc::clone(p));
        }
        let p = Rc::new(Program::load(archive, name)?);
        self.programs.insert(name.to_string(), Rc::clone(&p));
        // The header before the first scene marker does the loads.
        let mut header = Instance::new(Rc::clone(&p));
        let _ = header.run_frame(&mut self.stage, archive);
        // Thirty-two of the forty-one scripts load `ISLETEMP.SCR` there, and
        // it is the island: the same sea, the same horizon, the same palm,
        // the same ripples -- painted once and frozen. The live one has all
        // of that and moves, so keep it and let the scene draw over it. The
        // handful of scripts that load somewhere else (the office, Suzy's
        // beach) mean it, and those do replace the backdrop.
        if self.stage.last_screen.as_deref() == Some("ISLETEMP.SCR") {
            self.restore_island();
        }
        Some(p)
    }

    /// Start a scene: walk there first if Johnny is somewhere else.
    fn begin_scene(&mut self, archive: &Archive, scene: &'static Scene) {
        let file = tables::ADS_FILES
            .iter()
            .find(|(id, _)| *id == scene.ads)
            .map(|(_, n)| *n);
        let Some(file) = file else {
            // ADS file 0xFF is the table's "no script". Fourteen entries carry
            // it, one per position, all weight 1 and cost 5, all with `from`
            // and `to` the same place: Johnny stays where he is and does
            // nothing for a moment. That is the scene, not a gap in it.
            if scene.ads == NO_ADS_FILE {
                let ticks = IDLE_LEAST + self.rng.below(IDLE_SPAN);
                self.note(format!("idle at {:?} for {ticks} ticks", self.position));
                self.activity = Activity::Idling {
                    until: self.tick + ticks,
                };
                return;
            }
            self.note(format!("scene id {} has no ADS file", scene.id));
            return;
        };
        if !self.position_ok(scene) {
            // Already in the right area, wrong spot within it. The router
            // cannot help: `seg4:0x00B7` gives up when the majors already
            // match, because a route block is keyed on the pair. The minor is
            // a spot inside an area rather than somewhere to walk to, so take
            // it and get on with the scene -- which is what stops Johnny
            // teleporting across the island between one gag and the next.
            if scene.from.0 == self.position.0 {
                self.position.1 = scene.from.1;
            } else if let Some(step) = self.next_step(self.position.0, scene.from) {
                self.note(format!(
                    "walk {:?} -> {:?} via step {} ({} waypoints)",
                    self.position,
                    scene.from,
                    step.id,
                    step.path.len()
                ));
                self.activity = Activity::Walking {
                    step,
                    index: 0,
                    origin: self.position.0,
                    target: scene.from,
                    next: self.tick,
                };
                return;
            } else {
                // No route at all. The original would keep trying; move him
                // directly so the scene can still play, and say so.
                self.note(format!(
                    "no route {:?} -> {:?}, moving directly",
                    self.position, scene.from
                ));
                self.position = scene.from;
            }
        }
        let Some(mut seq) = Sequencer::from_record(archive, file) else {
            self.note(format!("no ADS record {file}"));
            return;
        };
        if !seq.start(scene.sequence as u16) {
            self.note(format!("{file} has no sequence {}", scene.sequence));
            return;
        }
        // `seg4:0x07DF`: a scene can ask to be drawn 272 pixels to the right
        // of the island, and eight of them do. Everything else follows the
        // island wherever the queue put it.
        self.set_draw_offset(scene.mask & 0x400 != 0);
        let label = seq
            .labels
            .get(&(scene.sequence as u16))
            .cloned()
            .unwrap_or_else(|| "(unnamed)".into());
        self.note(format!(
            "play {file} seq {} \"{label}\" at {:?} phase {}",
            scene.sequence, self.position, self.phase
        ));
        // A scene draws its own Johnny.
        self.standing = None;
        self.activity = Activity::Scene {
            row: scene,
            seq: Box::new(seq),
            active: Vec::new(),
            ends_at: scene.to,
            ticks: 0,
            done: false,
        };
    }

    /// Move the surf on, and repaint the island if it moved.
    ///
    /// Only while the island is still the backdrop on show. A scene that puts
    /// up its own `.SCR` owns the background until it is over, and the surf
    /// waits it out rather than flickering underneath.
    fn step_surf(&mut self) -> bool {
        let now = self.tick;
        let rng = &mut self.rng;
        let Some(scenery) = self.scenery.as_mut() else {
            return false;
        };
        if !scenery.tick(now, rng) || self.stage.background_gen != self.scenery_gen {
            return false;
        }
        let frame = scenery.compose();
        self.stage.set_background(frame);
        self.scenery_gen = self.stage.background_gen;
        true
    }

    /// Where drawing opcodes land, once the island has moved (`seg3:0x00FC`).
    ///
    /// A cutaway scene owns the whole screen and draws in absolute
    /// coordinates; everything else is drawn against the island, one pixel to
    /// its right, and eight scenes ask for a further 272 on top of that.
    fn set_draw_offset(&mut self, shifted: bool) {
        self.stage.draw_offset = if self.backdrop.cutaway {
            (0, 0)
        } else {
            let (dx, dy) = self.scenery.as_ref().map_or((0, 0), |s| s.offset);
            (dx + if shifted { 0x110 } else { 0 }, dy)
        };
    }

    /// Put the island back up after a scene has had the backdrop.
    fn restore_island(&mut self) {
        if let Some(scenery) = self.scenery.as_ref() {
            let frame = scenery.compose();
            self.stage.set_background(frame);
            self.scenery_gen = self.stage.background_gen;
        }
    }

    /// Lay down the island and the palette.
    ///
    /// `seg3:0x01E7` picks the sea: `NIGHT.SCR` once the phase reaches 12,
    /// otherwise one of `OCEAN00/01/02.SCR` at random, and `seg3:0x0262`
    /// blits `BACKGRND.BMP` over it at the island position.
    ///
    /// Those four screens are 640x480 and use palette index 0 — the
    /// transparency key — across thousands of pixels, so they are the VGA
    /// mode's animated sea drawn over something else rather than a backdrop in
    /// their own right. Working that out properly is still to do. Until then
    /// this uses `ISLETEMP.SCR`, the 640x350 island backdrop that the scenes
    /// themselves load, so the geometry matches what the sprites expect.
    pub fn load_scenery(&mut self, archive: &Archive) {
        if let Some(body) = archive.get("JOHNCAST.PAL") {
            if let Ok(p) = dgds::read_palette(body) {
                self.stage.set_palette(p);
            }
        }
        let roll = self.rng.next_u32();
        // `seg3:0x01E7`: night by the clock, or because a scene in the queue
        // asked for it.
        self.scenery_night = self.is_night() || self.backdrop.night;
        match Scenery::build(archive, self.scenery_night, roll, self.tick, self.day) {
            Some(s) => {
                let mut s = s;
                s.set_story(archive, self.day, self.backdrop, self.holiday.map(|h| h.index()));
                s.start_sky(&mut self.rng);
                // `seg3:0x0233`: the island is dropped where this queue's
                // scenes are willing to have it, and everything drawn against
                // it moves with it.
                s.place(self.island_state, &mut self.rng);
                self.note(format!(
                    "scenery: {}{} at {:?}",
                    s.sea,
                    if s.sandbar() { " + rock and sandbar" } else { "" },
                    s.offset
                ));
                self.stage.set_background(s.compose());
                self.scenery_gen = self.stage.background_gen;
                self.scenery = Some(s);
                self.set_draw_offset(false);
            }
            None => {
                // No sea in the archive. Fall back to the backdrop the scenes
                // load themselves, so the island still appears.
                if let Some(f) = archive.get("ISLETEMP.SCR").and_then(|b| dgds::read_screen(b).ok())
                {
                    self.stage.set_background(f);
                }
                self.note("scenery: ISLETEMP.SCR (no sea found)".into());
            }
        }
        self.stage.restore_background();
    }

    /// Advance one tick. Returns true if the surface changed.
    pub fn tick(&mut self, archive: &Archive) -> bool {
        self.tick += 1;
        // The backdrop is laid down when a queue is built and not before:
        // where the island goes depends on what that queue's scenes want.
        let surf = self.step_surf();
        let acted = match std::mem::replace(&mut self.activity, Activity::Idle) {
            Activity::Idle => {
                // `seg4:0x06D4`: take the next entry, and when there is none
                // left build another queue. The clock only moves here, so a
                // queue plays out in the light it started in.
                if self.cursor >= self.queue.len() {
                    self.queue_spent = true;
                    self.next_queue(archive);
                }
                match self.queue.get(self.cursor).copied() {
                    Some(s) => {
                        self.cursor += 1;
                        self.begin_scene(archive, s);
                    }
                    None => {
                        // Nothing playable at all. Wait rather than spin, and
                        // let the next queue try again.
                        self.note("nothing playable this phase".into());
                        self.activity = Activity::Idling {
                            until: self.tick + TICKS_PER_SECOND,
                        };
                    }
                }
                false
            }
            Activity::Intro { until } => {
                if self.tick > until {
                    return surf;
                }
                self.activity = Activity::Intro { until };
                // Put it up once; the surf is not running yet, so it stays.
                if self.tick > 1 {
                    return false;
                }
                match archive
                    .get("INTRO.SCR")
                    .and_then(|b| dgds::read_screen(b).ok())
                {
                    Some(f) => {
                        if let Some(p) = archive
                            .get("JOHNCAST.PAL")
                            .and_then(|b| dgds::read_palette(b).ok())
                        {
                            self.stage.set_palette(p);
                        }
                        self.stage.set_background(f);
                        self.stage.restore_background();
                        self.note("title card".into());
                        true
                    }
                    None => {
                        self.activity = Activity::Idle;
                        false
                    }
                }
            }
            Activity::Idling { until } => {
                if self.tick < until {
                    self.activity = Activity::Idling { until };
                }
                false
            }
            Activity::Walking {
                step,
                index,
                origin,
                target,
                next,
            } => {
                // Hold the current waypoint until its turn comes round.
                if self.tick < next {
                    self.activity = Activity::Walking {
                        step,
                        index,
                        origin,
                        target,
                        next,
                    };
                    return false;
                }
                self.stage.restore_background();
                if let Some(&(frame, x, y)) = step.path.get(index) {
                    // `seg4:0x02C0`: the waypoint's top bit is draw mode 2.
                    let mirror = Mirror::from_mode(if frame & 0x8000 != 0 { 2 } else { 0 });
                    self.draw_walk_frame(archive, frame & 0x7FFF, x, y, mirror);
                    self.activity = Activity::Walking {
                        step,
                        index: index + 1,
                        origin,
                        target,
                        next: self.tick + WALK_TICKS_PER_WAYPOINT,
                    };
                    true
                } else {
                    // Arrived at wherever this step lands. Keep going if the
                    // target is further on; the route block is re-entered.
                    self.position = step.lands;
                    if self.position.0 != target.0 {
                        if let Some(next_step) = self.next_step(origin, target) {
                            self.activity = Activity::Walking {
                                step: next_step,
                                index: 0,
                                origin,
                                target,
                                next: self.tick + WALK_TICKS_PER_WAYPOINT,
                            };
                            return true;
                        }
                    }
                    self.note(format!("arrived at {:?}", self.position));
                    false
                }
            }
            Activity::Scene {
                row,
                mut seq,
                mut active,
                ends_at,
                ticks,
                mut done,
            } => {
                let drew = self.run_scene(archive, &mut seq, &mut active, &mut done);
                // Scenes loop, so the budget is what ends them: the original
                // deducts each scene's cost from a running time allowance.
                let over_time = ticks > MAX_SCENE_TICKS;
                // The sequence has to have run out, and everything it left
                // running has to have finished. A scene that only loops on
                // itself never will, and does not count: it is scenery, and
                // the engine passes over it the same way (`seg10:0x07E3`
                // skips instances carrying the self-loop flag at +0xC).
                let settled = active.iter().all(|a| a.looping);
                if over_time {
                    self.note(format!(
                        "scene ran past {MAX_SCENE_TICKS} ticks, cutting it short"
                    ));
                }
                if (done && settled) || over_time {
                    self.position = self.landing(ends_at);
                    for (op, n) in seq.todo_seen.iter() {
                        *self.ads_todo.entry(*op).or_insert(0) += n;
                    }
                    self.restore_island();
                    self.note(format!("scene over, Johnny at {:?}", self.position));
                    if same(self.story_scene, row) {
                        // `seg4:0x0884`: the story got told.
                        self.story_scene = None;
                        self.story_state = 2;
                    }
                } else {
                    self.activity = Activity::Scene {
                        row,
                        seq,
                        active,
                        ends_at,
                        ticks: ticks + 1,
                        done,
                    };
                }
                drew
            }
        };
        // Between scenes Johnny is a single walk frame standing still, and
        // the backdrop under him has just been repainted.
        if !matches!(
            self.activity,
            Activity::Scene { .. } | Activity::Intro { .. }
        ) {
            self.redraw_standing(archive);
        }
        surf | acted
    }

    /// Put the standing Johnny back on top of the backdrop.
    fn redraw_standing(&mut self, archive: &Archive) {
        if let Some((frame, x, y, mirror)) = self.standing {
            self.draw_walk_frame(archive, frame, x, y, mirror);
        }
    }

    fn draw_walk_frame(&mut self, archive: &Archive, frame: u16, x: u16, y: u16, mirror: Mirror) {
        // The walk sprites live in JOHNWALK.BMP, which FILES.VIN preloads.
        if self.walk_frames.is_none() {
            let frames = archive
                .get("JOHNWALK.BMP")
                .and_then(|body| dgds::read_bitmap(body).ok())
                .unwrap_or_default();
            self.walk_frames = Some(Rc::new(frames));
        }
        let Some(frames) = self.walk_frames.clone() else {
            return;
        };
        let (sx, sy) = (x, y);
        // `seg4:0x02EA` adds the island's offset to every waypoint, so
        // Johnny walks the beach wherever the beach has got to.
        let (x, y) = self
            .scenery
            .as_ref()
            .map_or((x as i32, y as i32), |s| s.at(x as i32, y as i32));
        if let Some(f) = frames.get(frame as usize) {
            self.stage.surface.draw_sprite(f, x, y, mirror);
        }
        self.standing = Some((frame, sx, sy, mirror));
    }

    fn run_scene(
        &mut self,
        archive: &Archive,
        seq: &mut Sequencer,
        active: &mut Vec<Active>,
        done: &mut bool,
    ) -> bool {
        // The instances move first and the script is interpreted after them.
        // `seg10:0x1ACB` does it in that order and it is not incidental: a
        // scene reaching its end sets state 4, which is cleared the next time
        // that instance is stepped, so a script that ran first would be
        // reading last pass's answers and would miss half of them.
        self.stage.restore_background();
        advance_scenes(active, seq, &mut self.stage, archive, self.tick);

        // One pass of the sequencer, not one instruction: `seg10:0x1925`
        // interprets until a `Step` and only then returns, so everything a
        // guarded block starts happens on the same tick. The bound is a
        // backstop against a script that never reaches one.
        // Every pass re-reads the whole script from the top; `PASS_BUDGET`
        // bounds it in case this port has misread one.
        seq.begin_pass();
        for _ in 0..crate::ads::PASS_BUDGET {
            match seq.next_action(&mut self.rng) {
                Action::PassOver => break,
                Action::Reorder {
                    ttm,
                    scene,
                    to_front,
                } => {
                    // Draw order is list order, and the runner walks it head
                    // to tail, so the tail is on top.
                    if let Some(i) = active.iter().position(|a| (a.ttm, a.scene) == (ttm, scene)) {
                        let a = active.remove(i);
                        if to_front {
                            active.push(a);
                        } else {
                            active.insert(0, a);
                        }
                    }
                }
                Action::Play {
                    file,
                    scene,
                    repeat,
                } => {
                    let ttm_id = seq
                        .files
                        .iter()
                        .find(|(_, f)| **f == file)
                        .map(|(id, _)| *id)
                        .unwrap_or(0);
                    if std::env::var_os("JC_TRACE").is_some() {
                        self.note(format!("  play {file} scene {scene} {repeat:?}"));
                    }
                    // Already on screen: re-arm it where it stands.
                    if let Some(a) = active
                        .iter_mut()
                        .find(|a| (a.ttm, a.scene) == (ttm_id, scene))
                    {
                        a.rearm(repeat, self.tick);
                        continue;
                    }
                    if let Some(program) = self.program(archive, &file) {
                        let mut inst = Instance::new(program);
                        if inst.seek_scene(scene) {
                            active.push(Active::start(ttm_id, scene, inst, repeat, self.tick));
                        }
                    }
                }
            }
        }

        // `seg10:0x0766`: the scene is over when no sequence wants another
        // pass. Whether its instances have finished is the caller's half of
        // the same test.
        *done = !seq.running();
        self.stage.has_background()
    }

    /// Where a scene leaves Johnny standing (`seg4:0x081C`).
    ///
    /// A scene that says "anywhere" does not mean "where he was": the engine
    /// rolls an area and a spot inside it, so a gag that could have happened
    /// anywhere leaves him somewhere new.
    fn landing(&mut self, ends_at: (u8, u8)) -> (u8, u8) {
        let major = if ends_at.0 == 0 {
            (self.rng.below(6) + 1) as u8
        } else {
            ends_at.0
        };
        let minor = if ends_at.1 == 0 {
            (self.rng.next_u32() & 7) as u8
        } else {
            ends_at.1
        };
        (major, minor)
    }

    /// What phase it is now (`seg4:0x0554`).
    fn advance_phase(&mut self) {
        match self.clock {
            Clock::Wall { dawn } => {
                let now = self.hhmm.unwrap_or_else(wall_hhmm);
                self.phase = phase_at(now, dawn);
            }
            Clock::PerQueue => {
                self.phase = (self.phase + 1) % PHASES_PER_DAY;
            }
        }
    }

    /// Move the story on (`seg4:0x0BA5`).
    ///
    /// The story is a calendar, not a timer. `day` only moves when the real
    /// date does, and then only if the day already on offer was actually
    /// shown -- so eleven days of story take eleven days, and a machine that
    /// is off on Tuesday picks up where it left off on Wednesday.
    fn advance_story(&mut self) {
        // A test clock has no calendar to wait for: let the story move when
        // the day it stands for wraps.
        let today = match self.clock {
            Clock::Wall { .. } => self.date,
            Clock::PerQueue => self.tick / (PHASES_PER_DAY as u32 * 4096),
        };
        if self.story_date != Some(today) {
            let was = self.day;
            if self.day < self.story_offered {
                self.day += 1;
            }
            if self.day > STORY_DAYS || self.day < 2 {
                self.day = 1;
                self.story_offered = 1;
            }
            self.story_date = Some(today);
            if self.day != was || self.story_date.is_none() {
                self.note(format!("--- day {} ---", self.day));
            }
        }
        // `seg4:0x0C1A`: the day on offer moves on once its scene has played
        // and the queue that held it has run out.
        if self.queue_spent && self.story_state != 0 && self.story_offered <= self.day {
            self.story_offered += 1;
        }
        self.story_state = 0;
        self.queue_spent = false;
    }

    /// Forget which calendar day the story last moved on, so a test that
    /// jumps the date does not look like it has been running since then.
    pub fn story_date_reset(&mut self) {
        self.story_date = None;
    }

    /// How many TTM scenes are on screen at once. A scene that keeps its
    /// instances alive for ever is the shape of a stall, so the tools watch it.
    pub fn active_count(&self) -> usize {
        match &self.activity {
            Activity::Scene { active, .. } => active.len(),
            _ => 0,
        }
    }

    /// Sound ids the last tick asked for, taken off the stage.
    pub fn take_sounds(&mut self) -> Vec<u16> {
        std::mem::take(&mut self.stage.sounds)
    }

    /// How many waves are on the open water right now.
    pub fn wave_count(&self) -> usize {
        self.scenery.as_ref().map_or(0, |s| s.wave_count())
    }

    /// How many clouds are in the sky right now.
    pub fn cloud_count(&self) -> usize {
        self.scenery.as_ref().map_or(0, |s| s.cloud_count())
    }

    /// How many glints are on the water right now.
    pub fn glint_count(&self) -> usize {
        self.scenery.as_ref().map_or(0, |s| s.glint_count())
    }

    /// Ticks since the director started; one tick is `MS_PER_TICK`.
    pub fn ticks(&self) -> u32 {
        self.tick
    }

    pub fn is_night(&self) -> bool {
        self.phase >= FIRST_NIGHT_PHASE
    }
}
