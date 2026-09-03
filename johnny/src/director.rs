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

/// The default `Speed` setting: a percentage of the authored frame rate.
///
/// 100 runs the data exactly as written, and `tools/oracle.py tick` confirms
/// the port's frame timing matches `seg10:0x1ACB` tick for tick. Many people
/// remember the screensaver running slower, because a 386 could not hold
/// 62.5 passes a second and the engine's frame gate re-arms each deadline
/// from the moment it missed rather than catching up. That is a property of
/// the hardware, not the data, so slowing down is a setting rather than the
/// default.
pub const DEFAULT_SPEED: u32 = 100;

pub fn ms_per_tick_at(speed: u32) -> u32 {
    (MS_PER_TICK * 100 / speed.clamp(10, 400)).max(1)
}

/// What the tools use, which is the settings file plus whatever the
/// environment says on top of it.
pub fn ms_per_tick() -> u32 {
    ms_per_tick_at(crate::settings::Settings::load(&crate::settings::Settings::path())
        .with_env()
        .speed)
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
/// The scene table's `cost` field is not a duration: the queue builder spends
/// it out of a budget of 300 (`seg4:0x08B9`), and nothing times a scene by it.
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
    /// Getting Johnny where the next scene wants him.
    Moving(Box<Journey>),
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
    ///
    /// `seg10:0x196E` keeps the same thing as an absolute deadline at
    /// `+0x37`, set to `now + delay` each time it steps the instance on --
    /// and it steps on `now >= deadline`, *after* the frame has been drawn.
    /// So a frame asking for a delay of `d` is drawn `d + 1` times and this
    /// is one past the last of them.
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
    /// The frame on screen carried an `EndScene`, and its delay has not run
    /// out yet.
    ///
    /// `seg10:0x1D65` does not take a scene's word for it: the end flag only
    /// becomes state 4 once `seg10:0x196E` says the frame's delay is up. So
    /// the last frame of a scene is held for as long as it asked for, like
    /// every other frame. Ending on the tick it *starts* throws that hold
    /// away -- and a scene whose last frame asks for two to four seconds,
    /// which is what every standing pose does, is over before it is seen and
    /// starts again immediately.
    pub ending: bool,
}

/// Getting Johnny where a scene wants him standing, and facing the way it
/// wants him to face. `seg4:0x0094` is all of this in one function.
///
/// The second half of a position is a **facing**, not a spot: `seg4:0x0B64`
/// takes the scene's `(area, minor)` and writes the area into `ds:0x30E2` and
/// the minor into `ds:0x30DE`, then says whether Johnny is already both. He
/// pivots on the spot an eighth of a turn at a time until he is -- before the
/// walk, to line up with the path, and again at the end, to line up with the
/// scene.
pub struct Journey {
    /// The scene this is for. It begins when he arrives.
    scene: &'static Scene,
    /// `ds:0x30E2`: the area he is heading for.
    target: u8,
    /// `ds:0x30DE`: the facing the scene asked for, until it is satisfied.
    want_face: Option<u8>,
    /// `ds:0x30CA`: the facing he is turning toward right now -- the walk
    /// path's own starting facing while walking, the scene's once he is
    /// there.
    turn_goal: u8,
    /// The area he set out from. The route block is looked up once, on the
    /// pair (`ds:0x30D2`), and re-read for wherever he has got to since.
    origin: u8,
    /// `ds:0x30D0`: the path he is on, and how far along it he is.
    path: Option<(&'static WalkStep, usize)>,
    /// `ds:0x30D6`: the tick the next waypoint or turn is due.
    next: u32,
}

/// One eighth of a turn from `at` toward `to`, the short way round
/// (`seg4:0x01D6`).
///
/// Dead opposite has no short way, so the engine tosses a coin -- which is
/// the only reason two identical walks can start differently.
pub fn turn_toward(at: u8, to: u8, rng: &mut Rng) -> u8 {
    // `seg4:0x01CD` tests this before anything else, and it matters that the
    // test comes first: the coin toss below would otherwise be thrown for a
    // turn nobody is making, and every later draw would come out different.
    if at == to {
        return at;
    }
    let delta = at as i16 - to as i16;
    let dir: i16 = if delta == 4 || delta == -4 {
        if rng.next_u16() & 1 != 0 {
            -1
        } else {
            1
        }
    } else if delta < -4 || (0 < delta && delta < 4) {
        -1
    } else {
        1
    };
    ((at as i16 + dir) & 7) as u8
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
            ending: false,
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
        // Not `ending`. `seg10:0x0DB6` writes the run state and the repeat
        // count and nothing else, and the end flag is not a field it could
        // clear: `ds:0x4644` is per frame, raised again by the `0110` in the
        // frame that is being held. So a scene re-armed while its last frame
        // is still on screen still ends when that frame's delay runs out --
        // and then the repeat mode decides whether it starts again. Clearing
        // it here instead steps the instance out of the held frame and into
        // whatever is filed after the scene.
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
                // `seg10:0x1D21` puts the scene's own start into the pending
                // jump before it decides anything else, so an instance that
                // has reached its end is wound back whether or not it is
                // going to play again. Leaving it at the end means the next
                // `PlayScene` re-arms something with nothing left to run --
                // and since `seg10:0x0DB6` re-arms in place rather than
                // starting over, that scene then ends on the tick it starts,
                // for ever.
                self.inst.seek_scene(self.scene);
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

    /// `seg10:0x1DBA`: the runner could not run this scene at all. It is
    /// wound back to the scene's own start and put into state 4 whatever the
    /// repeat mode says -- `seg10:0x1D21`'s count and deadline restarts are
    /// on the other side of that branch and never see it.
    pub fn cannot_run(&mut self) {
        self.inst.seek_scene(self.scene);
        self.state = SceneState::Over;
    }

    /// `seg10:0x1DEB`: a scene queued until a deadline is over once that
    /// deadline passes, and then ends on its next time round.
    pub fn check_deadline(&mut self, now: u32) {
        if self.state == SceneState::Queued(3) && now >= self.until {
            self.state = SceneState::Over;
        }
    }
}

/// Where a scene sits in the instance list, which is how deep it is drawn.
///
/// A scene the list has not got goes at the end -- the engine cannot get
/// there at all, since `seg10:0x0DB6` does nothing without a record, and the
/// port only reaches it for a scene its TTM does not define.
pub fn depth(order: &[(u16, u16)], ttm: u16, scene: u16) -> usize {
    order
        .iter()
        .position(|e| *e == (ttm, scene))
        .unwrap_or(order.len())
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
        // Not its turn to move -- or its last frame is on screen and being
        // held for the delay it asked for, which is the same thing as far as
        // the picture is concerned. Stepping it again would run the instance
        // straight out of its own scene and into whatever follows it in the
        // file.
        if a.wake > now || a.ending {
            let (t, sc, f) = (a.ttm, a.scene, a.inst.held_frame());
            stage.say(|| format!("run {t} {sc} {f} 1"));
            a.inst.repaint(stage);
        } else {
            let (t, sc, f) = (a.ttm, a.scene, a.inst.frame_index());
            stage.say(|| format!("run {t} {sc} {f} 0"));
            match a.inst.run_frame(stage, archive) {
                // `seg10:0x1DBA`: nothing ran. That is not a scene that has
                // played out, it is a scene that cannot play, so it goes
                // straight to state 4 with none of the count or deadline
                // restarts `seg10:0x1D21` allows -- restarting a program
                // with nothing left in it would only do this again.
                ttm::Step::Finished => a.cannot_run(),
                ttm::Step::Frame(delay) => {
                    // Not floored, and one past the last tick the frame is
                    // drawn on: `seg10:0x196E` is asked *after* the frame has
                    // been drawn and steps on `now >= deadline`, so the tick
                    // the deadline falls on still shows this frame. A delay
                    // of zero is a special case there -- it is always up --
                    // and lands on the same answer.
                    a.wake = now + delay as u32 + a.inst.rebased() as u32;
                    // The frame carried an `EndScene`, which is how a scene
                    // says it has reached its end -- it does not stop
                    // mid-frame, it sets a flag the runner reads afterwards
                    // (`seg10:0x1D1A`). Every scene in the data ends this way;
                    // almost none simply run out of instructions.
                    if a.inst.last_goto == Some(a.scene) {
                        a.looping = true;
                    }
                    // `seg10:0x1D65`: the end flag waits for the frame's own
                    // delay before it counts.
                    a.ending = a.inst.ended;
                }
            }
        }
        // The held last frame's delay has run out, so now the scene is over.
        // The end and the step share one test in the engine -- `seg10:0x1D65`
        // and `seg10:0x1DAD` both call `seg10:0x196E` -- so they share one
        // here.
        if a.ending && now + 1 >= a.wake {
            a.ending = false;
            if !a.program_ended() {
                finished.push((a.ttm, a.scene));
            }
        }
        a.check_deadline(now);
        seq.set_state(a.ttm, a.scene, a.state);
    }
    // `seg10:0x1E2F`: with the list walked, the clip goes back to the whole
    // screen. Everything drawn outside the runner -- Johnny's walk sprite,
    // the island, the surf -- is unclipped, whatever the last scene set.
    stage.surface.clip = None;
    for (t, s) in &finished {
        seq.set_state(*t, *s, SceneState::Idle);
    }
    active.retain(|a| !finished.contains(&(a.ttm, a.scene)));
    finished
}

/// Hold the repaint model to its promise, and report anything that broke it.
///
/// The port paints the whole picture afresh every tick, so a held frame has
/// to go back on top of the new backdrop each time. That is only the same
/// picture the engine would have left standing if putting a frame back (a)
/// draws exactly what stepping it drew and (b) changes nothing else at all --
/// no bitmap loaded, no noise made, no clip left over, no draw from the
/// generator.
///
/// Four bugs came from an opcode that quietly broke one half or the other,
/// and every one of them was found by watching the screensaver rather than by
/// testing. So this checks it: take the picture a tick finished with, paint
/// the whole tick again out of the frames' recorded drawings alone, and
/// compare both the pixels and everything else. Since `Instance::repaint`
/// only replays resolved marks it should not be *able* to fail -- which is
/// the point. What the check still catches is a drawing opcode that paints
/// without recording, so the list stays complete as opcodes are added.
///
/// It is a diagnostic rather than part of the engine -- `jcplay` and
/// `jcisland` switch it on with `JC_CHECK=1`.
pub fn check_repaint(active: &mut [Active], stage: &mut Stage) -> Vec<String> {
    let painted = stage.surface.pixels.clone();
    let stepped = stage.untouched();
    let cursors: Vec<_> = active.iter().map(|a| a.inst.cursor()).collect();

    stage.restore_background();
    for a in active.iter_mut() {
        a.inst.repaint(stage);
    }

    let mut bad: Vec<String> = stage
        .untouched()
        .differences(&stepped)
        .into_iter()
        .map(|w| format!("a repaint {w}"))
        .collect();
    if stage.surface.pixels != painted {
        let n = stage
            .surface
            .pixels
            .iter()
            .zip(&painted)
            .filter(|(a, b)| a != b)
            .count();
        let who: Vec<String> = active
            .iter()
            .map(|a| format!("{}/{} x{}", a.ttm, a.scene, a.inst.marks()))
            .collect();
        bad.push(format!(
            "a repaint drew a different picture ({n} pixels), live: {}",
            who.join(" ")
        ));
    }
    for (a, was) in active.iter().zip(cursors) {
        if a.inst.cursor() != was {
            bad.push(format!(
                "a repaint moved {}/{} on: {:?} -> {:?}",
                a.ttm,
                a.scene,
                was,
                a.inst.cursor()
            ));
        }
    }
    bad
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
pub fn civil_from_days(days: u32) -> (u16, u8, u8) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year as u16, month, day)
}

/// Just the month and day, which is what the holiday check wants.
pub fn month_and_day(days: u32) -> (u8, u8) {
    let (_, month, day) = civil_from_days(days);
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
    pub stage: Stage,
    activity: Activity,
    programs: HashMap<String, Rc<Program>>,
    /// The instance list, in the order the engine builds it.
    ///
    /// `seg11:0x0645` makes one record per `1111` marker as each TTM loads
    /// and appends it to one list, so the list reads: every scene of the
    /// first file in marker order, then every scene of the second, and so
    /// on. `seg10:0x1C35` walks that list head to tail, so it is the draw
    /// order, and `seg10:0x0DB6` re-arms the record it finds rather than
    /// making a new one -- a scene that ends and plays again comes back at
    /// the same depth. Only `4000` and `4010` move anything.
    ///
    /// Appending a scene to `active` when it starts, which is the obvious
    /// thing to do, instead puts whatever played most recently on top. With
    /// five scenes on screen at once that is visible.
    order: Vec<(u16, u16)>,
    /// Files loaded but whose header frame has not been run yet.
    fresh: Vec<String>,
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
    pub island_state: u16,
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
    /// Everything the settings file decides. `Director::configure` applies
    /// it; nothing in here reads the environment on its own.
    pub settings: crate::settings::Settings,
    /// The wall clock as HHMM, when the host knows it better than this
    /// process does -- `johnny.scr` reads the local time, the tools do not.
    pub hhmm: Option<u16>,
    /// `JC_CHECK=1`: hold every tick's repaint to its invariant. A diagnostic
    /// for the tools; see `check_repaint`.
    checking: bool,
    /// What that check has complained about, so a run can end with a verdict.
    pub complaints: Vec<String>,
}

impl Director {
    pub fn new(seed: u64) -> Self {
        // One generator for the whole screensaver, on the stage, because that
        // is what a TTM frame can reach. See `ttm::Stage::rng`.
        let mut stage = Stage::new();
        stage.rng = Rng::new(seed);
        Director {
            // `seg4:0x002C`: area 1, facing 0, and heading nowhere.
            position: (1, 0),
            phase: 0,
            day: 1,
            holiday: None,
            faithful_router: false,
            stage,
            activity: Activity::Intro { until: INTRO_TICKS },
            programs: HashMap::new(),
            order: Vec::new(),
            fresh: Vec::new(),
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
            checking: std::env::var_os("JC_CHECK").is_some(),
            complaints: Vec::new(),
            settings: crate::settings::Settings::default(),
        }
    }

    /// Take the settings on. Story state included: `Introduction` and
    /// `NumDays` are the day on offer and the day reached, and
    /// `CurrentYear`/`Month`/`Day` is when they last moved -- so an
    /// eleven-day story picks up where the last session left it rather than
    /// starting again every time the screen goes blank.
    pub fn configure(&mut self, s: crate::settings::Settings) {
        self.clock = Clock::Wall { dawn: s.start_time };
        self.backdrop.allow_waves = s.waves;
        self.backdrop.allow_clouds = s.clouds;
        self.activity = Activity::Intro { until: s.intro_ticks };
        if s.story_reached > 0 {
            // `Introduction` is the day on offer (`ds:0x2D9B`) and `NumDays`
            // the day reached (`ds:0x2D9C`). They leapfrog -- the offer moves
            // on when a queue holding it runs out, the reached day catches up
            // on the next real day -- so the offer is often one ahead, and
            // taking it for both jumps the story forward on every reload.
            self.story_offered = s.story_offered.max(1);
            self.day = s.story_reached;
            let (y, m, d) = s.story_date;
            if y > 0 {
                self.story_date = Some(days_from_civil(y as i64, m as u32, d as u32));
            }
        }
        self.settings = s;
    }

    /// The story state to write back when the screensaver stops.
    pub fn story_snapshot(&self) -> (u8, u8, (u16, u8, u8)) {
        (
            self.story_offered,
            self.day,
            self.story_date.map(civil_from_days).unwrap_or_default(),
        )
    }

    fn note(&mut self, line: String) {
        self.log.push(line);
    }

    /// Does this scene's declared position match where Johnny is?
    ///
    /// `seg4:0x0B64`: a required area of 0 means anywhere, and a required
    /// facing of -1 means any facing. No scene in the table uses the facing
    /// wildcard, but the engine allows it.
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
            if self.stage.rng.below(10) != 0 {
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
        let mut roll = self.stage.rng.below(total) as i64 + 1;
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
    pub fn island_allows(&self, mask: u32) -> bool {
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
    pub fn take_wishes(&mut self, mask: u32) {
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
        let opener = mate
            .filter(|_| mate_pending)
            .filter(|m| m.mask & category::PRIMARY != 0)
            .filter(|_| self.coin(50));
        if let Some(m) = opener {
            self.queue.push(m);
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
            if let Some(m) = mate.filter(|_| mate_pending).filter(|_| self.coin(5)) {
                self.queue.push(m);
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
        let mut roll = self.stage.rng.next_u16() as u32;
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
        self.backdrop.allow_waves = self.settings.waves;
        self.backdrop.allow_clouds = self.settings.clouds;
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
        // `seg4:0x0115`: subtract, and take this one the moment the running
        // count stops being *positive*. Starting at `roll + 1` instead takes
        // the next option whenever the draw lands exactly on a boundary --
        // twenty-seven times in the oracle's 4,736 rolls, which is a wrong
        // turn about once every two hundred walks.
        let mut roll = self.stage.rng.below(span) as i64;
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

    /// One roll of the route picker, for the oracle: which step does the
    /// engine choose to get from `at` toward `to`, on a route block that was
    /// entered at `from`?
    pub fn route_roll(&mut self, from: u8, to: u8, at: u8) -> Option<u8> {
        self.position = (at, 0);
        self.next_step(from, (to, 0)).map(|w| w.id)
    }

    /// Load a TTM, or hand back the one already loaded. Nothing is run: the
    /// instance list is built from what the file says, and the engine builds
    /// it as the file loads.
    fn program(&mut self, archive: &Archive, name: &str) -> Option<Rc<Program>> {
        if let Some(p) = self.programs.get(name) {
            return Some(Rc::clone(p));
        }
        let p = Rc::new(Program::load(archive, name)?);
        self.programs.insert(name.to_string(), Rc::clone(&p));
        self.fresh.push(name.to_string());
        Some(p)
    }

    /// The same, with the header frame run if it has not been yet.
    fn ready(&mut self, archive: &Archive, name: &str) -> Option<Rc<Program>> {
        let p = self.program(archive, name)?;
        let Some(i) = self.fresh.iter().position(|f| f == name) else {
            return Some(p);
        };
        self.fresh.remove(i);
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
        if file.is_none() && scene.ads != NO_ADS_FILE {
            self.note(format!("scene id {} has no ADS file", scene.id));
            return;
        }
        // `seg4:0x0716` asks where Johnny is *before* it asks what the entry
        // is (`seg4:0x0740`), so an entry with no script is walked to like
        // any other and only then pauses. The fourteen of them each name a
        // spot -- (1,3), (2,7), (6,0) and so on -- and none says "anywhere",
        // so this is the difference between Johnny strolling somewhere to
        // stand and think for a few seconds and Johnny stopping dead
        // wherever he was.
        if !self.position_ok(scene) {
            // `seg4:0x0B64` writes the scene's area into `ds:0x30E2` and its
            // facing into `ds:0x30DE` and reports whether Johnny is already
            // both. He is not, so he sets off -- and the scene waits, rather
            // than being dropped for the next one in the queue.
            let target = if scene.from.0 == 0 {
                self.position.0
            } else {
                scene.from.0
            };
            self.note(format!(
                "walk {:?} -> {:?} for {} seq {}",
                self.position,
                scene.from,
                file.unwrap_or("(no script)"),
                scene.sequence
            ));
            self.activity = Activity::Moving(Box::new(Journey {
                scene,
                target,
                want_face: Some(scene.from.1),
                turn_goal: self.position.1,
                origin: self.position.0,
                path: None,
                next: self.tick,
            }));
            return;
        }
        // ADS file 0xFF is the table's "no script". Fourteen entries carry
        // it, one per position, all weight 1 and cost 5, all with `from` and
        // `to` the same place: Johnny stands there and does nothing for a
        // moment. That is the scene, not a gap in it.
        let Some(file) = file else {
            let ticks = IDLE_LEAST + self.stage.rng.below(IDLE_SPAN);
            self.note(format!("idle at {:?} for {ticks} ticks", self.position));
            self.activity = Activity::Idling {
                until: self.tick + ticks,
            };
            return;
        };
        let Some(mut seq) = Sequencer::from_record(archive, file) else {
            self.note(format!("no ADS record {file}"));
            return;
        };
        if !seq.start(scene.sequence as u16) {
            self.note(format!("{file} has no sequence {}", scene.sequence));
            return;
        }
        // Build the instance list, which is what draw order is. The engine
        // loads the files an ADS names as the ADS loads and makes one record
        // per `1111` marker as each arrives (`seg11:0x0645`), so the list is
        // file order then marker order. `seq.files` is keyed by the id in the
        // `RES:` table, and that table is the load order.
        self.order.clear();
        let files: Vec<(u16, String)> =
            seq.files.iter().map(|(id, f)| (*id, f.clone())).collect();
        for (id, name) in files {
            let Some(p) = self.program(archive, &name) else {
                continue;
            };
            for scene in &p.scenes {
                self.order.push((id, *scene));
            }
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
        // Start it unclipped, which by now it already is: the clip belongs
        // to the instance (`seg10:0x1C90` restores it before every frame) and
        // `seg10:0x1E2F` puts the screen's own back to full after each pass.
        // This is here because the two together are what the engine does and
        // one of them living in the runner is not a reason to leave the other
        // out.
        self.stage.surface.clip = None;
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
        let rng = &mut self.stage.rng;
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

    /// Lay down the island and the palette: `seg3:0x0136`, which is run once
    /// per queue.
    ///
    /// The random draws come in the engine's order, because the generator is
    /// one stream and where each value lands in it matters: `seg3:0x01F5`
    /// picks the ocean, in daylight and not for a cutaway; `seg3:0x0226`
    /// flips for the sandbar, unless a scene in the queue said no rock;
    /// `seg3:0x1B00` places the island; `seg3:0x0A38` sets the sky up; and
    /// `seg3:0x040E(1)` runs the backdrop pass once.
    pub fn load_scenery(&mut self, archive: &Archive) {
        if let Some(body) = archive.get("JOHNCAST.PAL") {
            if let Ok(p) = dgds::read_palette(body) {
                self.stage.set_palette(p);
            }
        }
        // `seg3:0x01E7`: night by the clock, or because a scene in the queue
        // asked for it. A cutaway clears the screen instead (`seg3:0x01C4`)
        // and makes no draw at all.
        self.scenery_night = self.is_night() || self.backdrop.night;
        let sea = if self.scenery_night || self.backdrop.cutaway {
            scenery::NIGHT_SEA
        } else {
            scenery::SEAS[self.stage.rng.below(3) as usize]
        };
        let extras = !self.backdrop.no_rock && self.stage.rng.next_u16() & 1 == 1;
        match Scenery::build(archive, sea, extras, self.tick, self.day) {
            Some(s) => {
                let mut s = s;
                s.set_palette(*self.stage.palette());
                s.set_story(archive, self.day, self.backdrop, self.holiday.map(|h| h.index()));
                // `seg3:0x0233`: the island is dropped where this queue's
                // scenes are willing to have it, and everything drawn against
                // it moves with it.
                s.place(self.island_state, &mut self.stage.rng);
                s.start_sky(&mut self.stage.rng);
                s.setup(self.tick, &mut self.stage.rng);
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
                if let Some(f) = archive.screen("ISLETEMP.SCR") {
                    self.stage.set_background(f);
                }
                self.note("scenery: ISLETEMP.SCR (no sea found)".into());
            }
        }
        self.stage.restore_background();
    }

    /// Advance one tick: `seg4:0x06BF`. Returns true if the surface changed.
    pub fn tick(&mut self, archive: &Archive) -> bool {
        self.tick += 1;
        // `seg4:0x06D4`: when the queue has run out, build another one before
        // anything else. The clock only moves here, so a queue plays out in
        // the light it started in. The backdrop is laid down as part of it,
        // because where the island goes depends on what the queue wants.
        if matches!(self.activity, Activity::Idle) && self.cursor >= self.queue.len() {
            self.queue_spent = true;
            self.next_queue(archive);
        }
        // `seg4:0x070E`: the backdrop pass, every tick.
        let surf = self.step_surf();
        let acted = match std::mem::replace(&mut self.activity, Activity::Idle) {
            Activity::Idle => {
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
                match archive.screen("INTRO.SCR") {
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
            Activity::Moving(j) => self.step_journey(archive, *j),
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
                // running has to have finished. `seg10:0x07D7` walks the
                // instances and asks three questions of each: state 4 does
                // not hold the scene open, nor state 0, nor one carrying the
                // self-loop flag at +0xC -- a scene that only loops on itself
                // is scenery and will never finish on its own. Anything else
                // does.
                //
                // Where this is still wider than `seg10:0x0766`: that asks
                // about *one* sequence -- the one the scheduler started --
                // and its instances alone, while `running` here asks about
                // every sequence in the file. So a sequence woken by `F200`
                // and left running keeps the scene alive here and would not
                // there. It waits longer rather than cutting something off,
                // and nothing shipped leaves one running: every sequence in
                // all ten scripts ends by stopping itself.
                let settled = active
                    .iter()
                    .all(|a| a.looping || matches!(a.state, SceneState::Idle | SceneState::Over));
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
                    if let Some(c) = self.stage.surface.clip {
                        self.note(format!("scene left a clip set: {c:?}"));
                    }
                    self.note(format!("scene over, Johnny at {:?}", self.position));
                    // The scene was drawing its own Johnny; from here he is
                    // the engine's again, in the pose for wherever the scene
                    // left him standing.
                    self.stand(archive, true);
                    if same(self.story_scene, row) {
                        // `seg4:0x0884`: the story got told. It asks for
                        // sound 25 first (`seg5:0x00BA` with 0x19), which is
                        // one past the last wave in `SCRANTIC.EXE` and so
                        // plays nothing -- the engine wanted a noise here and
                        // the file it shipped with has not got one. Asked for
                        // anyway, so a sound census says what the scripts and
                        // the engine actually ask for rather than what could
                        // be heard.
                        self.stage.sounds.push(25);
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

    /// One pass of `seg4:0x0094`: look up a route, pick a step, pivot, walk.
    ///
    /// The order is the engine's. A tick that finishes a pivot goes straight
    /// on to draw the first waypoint, which is why the last frame of a turn
    /// is never seen.
    fn step_journey(&mut self, archive: &Archive, mut j: Journey) -> bool {
        // `seg4:0x00A3`: nothing moves until the delay slot runs out. He is
        // still standing there in the meantime, and the surf has just
        // repainted the beach under him.
        if self.tick < j.next {
            self.redraw_standing(archive);
            self.activity = Activity::Moving(Box::new(j));
            return true;
        }
        self.stage.restore_background();

        // `seg4:0x00E1`: no path and not there yet, so pick a step out of the
        // route block for wherever he is standing.
        if j.path.is_none() && self.position.0 != j.target {
            match self.next_step(j.origin, (j.target, 0)) {
                Some(step) => {
                    self.note(format!(
                        "  step {} ({} waypoints), facing {} first",
                        step.id,
                        step.path.len(),
                        step.face
                    ));
                    j.turn_goal = step.face;
                    j.path = Some((step, 0));
                }
                None => {
                    // No block covers this pair. The original would stand
                    // there for good; move him and say so.
                    self.note(format!(
                        "no route {:?} -> {}, moving directly",
                        self.position, j.target
                    ));
                    self.position = (j.target, j.want_face.unwrap_or(self.position.1));
                }
            }
        }

        // `seg4:0x0179`: in the right area and done turning, so the facing
        // the scene asked for becomes the next thing to turn to -- and when
        // he is already at it, the request is satisfied.
        if j.path.is_none() && self.position.0 == j.target && self.position.1 == j.turn_goal {
            match j.want_face {
                Some(want) if want != j.turn_goal => j.turn_goal = want,
                _ => j.want_face = None,
            }
        }

        // `seg4:0x01CA`: an eighth of a turn, and the pose that goes with it.
        if self.position.1 != j.turn_goal {
            self.position.1 = turn_toward(self.position.1, j.turn_goal, &mut self.stage.rng);
            self.stand(archive, self.position.0 == j.target);
            j.next = self.tick + WALK_TICKS_PER_WAYPOINT;
            self.activity = Activity::Moving(Box::new(j));
            return true;
        }

        // `seg4:0x0274`: facing the right way, so take the next waypoint.
        if let Some((step, i)) = j.path {
            match step.path.get(i) {
                Some(&(frame, x, y)) => {
                    // `seg4:0x02C0`: the waypoint's top bit is draw mode 2.
                    let mirror = Mirror::from_mode(if frame & 0x8000 != 0 { 2 } else { 0 });
                    self.draw_walk_frame(archive, frame & 0x7FFF, x, y, mirror);
                    j.path = Some((step, i + 1));
                    j.next = self.tick + WALK_TICKS_PER_WAYPOINT;
                    self.activity = Activity::Moving(Box::new(j));
                    return true;
                }
                None => {
                    // `seg4:0x0294`: the path has run out, so he is standing
                    // where it said it would leave him, facing the way it
                    // said. The route block is re-entered from there.
                    self.position = step.lands;
                    j.turn_goal = self.position.1;
                    j.path = None;
                    self.note(format!("arrived at {:?}", self.position));
                    j.next = self.tick + WALK_TICKS_PER_WAYPOINT;
                    self.activity = Activity::Moving(Box::new(j));
                    return true;
                }
            }
        }

        // In the right area, facing the right way: the scene can start.
        let scene = j.scene;
        self.stand(archive, true);
        self.begin_scene(archive, scene);
        true
    }

    /// Draw Johnny standing at this spot, facing whichever way he faces.
    ///
    /// `seg4:0x0223` picks one of two pose tables per area -- the first for
    /// when he has reached the area he was heading for, the second while he
    /// is still travelling -- and `seg4:0x0258` indexes it by the facing.
    /// Eight poses, one per point of the compass.
    fn stand(&mut self, archive: &Archive, arrived: bool) {
        let area = self.position.0 as usize;
        let Some(&(here, onward)) = tables::POSES.get(area) else {
            return;
        };
        let id = if arrived { here } else { onward };
        let Some(pose) = tables::WALK_STEPS.iter().find(|w| w.id == id) else {
            return;
        };
        let Some(&(frame, x, y)) = pose.path.get(self.position.1 as usize) else {
            return;
        };
        let mirror = Mirror::from_mode(if frame & 0x8000 != 0 { 2 } else { 0 });
        self.draw_walk_frame(archive, frame & 0x7FFF, x, y, mirror);
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
            let frames = archive.bitmap("JOHNWALK.BMP").unwrap_or_default();
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
        // The script is interpreted first and the instances move after it.
        // `seg10:0x1ACB` does it in that order -- the pass at `seg10:0x1B6B`,
        // then the walk at `seg10:0x1C32` -- and it is not incidental: a
        // scene the pass starts draws its first frame on the same tick rather
        // than sixteen milliseconds later, and a scene that reached its end
        // last tick is still in state 4 when this pass reads it, because it
        // is the walk that clears it (`seg10:0x1DDC`).
        //
        // One pass of the sequencer, not one instruction: `seg10:0x1925`
        // interprets until a `Step` and only then returns, so everything a
        // guarded block starts happens on the same tick. The bound is a
        // backstop against a script that never reaches one.
        // Every pass re-reads the whole script from the top; `PASS_BUDGET`
        // bounds it in case this port has misread one.
        seq.begin_pass();
        for _ in 0..crate::ads::PASS_BUDGET {
            match seq.next_action(&mut self.stage.rng) {
                Action::PassOver => break,
                Action::Reorder {
                    ttm,
                    scene,
                    to_front,
                } => {
                    // Draw order is list order, and the runner walks it head
                    // to tail, so the tail is on top.
                    // The engine moves the record inside the instance list
                    // (`seg10:0x0F8C` to the tail, `seg10:0x0EF2` to the
                    // head), so the move outlives the scene it was asked
                    // for: the record is still there, at its new depth, when
                    // the scene plays again.
                    if let Some(i) = self.order.iter().position(|e| *e == (ttm, scene)) {
                        let e = self.order.remove(i);
                        if to_front {
                            self.order.push(e);
                        } else {
                            self.order.insert(0, e);
                        }
                    }
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
                    // A scene the TTM has not got -- or a TTM that is not
                    // in the archive at all -- still gets an instance.
                    // `seg10:0x0DB6` never checks, and `seg10:0x1DBA` is
                    // what deals with it: the runner cannot run the scene,
                    // so it goes into state 4 on its first tick and the
                    // block waiting on it fires. Dropping the play instead
                    // leaves that block waiting for ever.
                    let program = self
                        .program(archive, &file)
                        .unwrap_or_else(|| Rc::new(ttm::Program::empty()));
                    let mut inst = Instance::new(program);
                    if !inst.seek_scene(scene) {
                        inst.exhaust();
                    }
                    let _ = self.ready(archive, &file);
                    // Back where the instance list has always had it, not on
                    // top: the engine's record was made when the file loaded
                    // and has been sitting there, idle, ever since.
                    let rank = depth(&self.order, ttm_id, scene);
                    let at = active
                        .iter()
                        .position(|a| depth(&self.order, a.ttm, a.scene) > rank)
                        .unwrap_or(active.len());
                    active.insert(at, Active::start(ttm_id, scene, inst, repeat, self.tick));
                }
            }
        }

        self.stage.restore_background();
        advance_scenes(active, seq, &mut self.stage, archive, self.tick);

        // The picture for this tick is complete: everything live has been
        // painted onto a fresh backdrop. If the repaint model is sound, the
        // same picture comes back out of the retained frames alone.
        if self.checking {
            for what in check_repaint(active, &mut self.stage) {
                let tick = self.tick;
                self.complaints.push(format!("[tick {tick}] {what}"));
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
            (self.stage.rng.below(6) + 1) as u8
        } else {
            ends_at.0
        };
        let minor = if ends_at.1 == 0 {
            (self.stage.rng.next_u16() & 7) as u8
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

    /// Run the story's per-queue settling once, for the tests. The
    /// screensaver reaches this through `next_queue`.
    pub fn settle_story_for_test(&mut self) {
        self.advance_story();
    }

    /// Forget which calendar day the story last moved on, so a test that
    /// jumps the date does not look like it has been running since then.
    pub fn story_date_reset(&mut self) {
        self.story_date = None;
    }

    /// Build one queue and report it as row numbers in the scene table, so
    /// `tools/oracle.py` can put the same question to `seg4:0x08B9` in the
    /// original and diff the answers.
    pub fn queue_rows(&mut self) -> Vec<usize> {
        // The same reset `seg4:0x0633` does before every build.
        self.island_state = scenery::ANYWHERE;
        self.backdrop = Backdrop::default();
        self.backdrop.allow_waves = self.settings.waves;
        self.backdrop.allow_clouds = self.settings.clouds;
        self.build_queue();
        let base = tables::SCENES.as_ptr() as usize;
        self.queue
            .iter()
            .map(|s| (*s as *const _ as usize - base) / std::mem::size_of::<Scene>())
            .collect()
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
