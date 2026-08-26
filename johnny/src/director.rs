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
use crate::scenery::Scenery;
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

/// How long a script-less entry holds Johnny still. Its `cost` is 5, and the
/// chooser prices scenes in it, so a few seconds is the shape of it.
fn idle_ticks(scene: &Scene) -> u32 {
    scene.cost.max(1) as u32 * TICKS_PER_SECOND
}

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
    /// Standing about, for one of the table's script-less entries.
    Idling { until: u32 },
    /// Following one walk path towards `target`.
    Walking {
        step: &'static WalkStep,
        index: usize,
        target: (u8, u8),
        /// Tick at which the next waypoint is due.
        next: u32,
    },
    /// Running a scene's ADS sequence.
    Scene {
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

    /// The scene's program reached its end. `seg10:0x1D2B` decides what that
    /// means from the run state, and the answer is never simply "stop":
    ///
    /// * queued to play once, it starts over and becomes **running**
    /// * queued for a count, it starts over until the count runs out
    /// * queued until a deadline, it starts over until the deadline passes
    /// * **running**, and only then, it goes idle and is dropped
    ///
    /// So a scene played with `flags` of zero runs its program through twice:
    /// once to get from queued to running, once to finish. That is what makes
    /// `IF_RUNNING` on the scene you just started ever come true, and every
    /// script is built on it.
    pub fn program_ended(&mut self) -> bool {
        match self.state {
            SceneState::Queued(1) => {
                self.state = SceneState::Running;
                self.restart()
            }
            SceneState::Queued(2) if self.count > 0 => {
                self.count -= 1;
                self.restart()
            }
            SceneState::Queued(2) => {
                self.state = SceneState::Running;
                self.restart()
            }
            // The deadline is what ends this one; until it passes, keep going.
            SceneState::Queued(_) => self.restart(),
            _ => {
                self.state = SceneState::Idle;
                false
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

    /// `seg10:0x1DEB`: a scene queued until a deadline becomes running once
    /// that deadline passes, and then ends on its next time round.
    pub fn check_deadline(&mut self, now: u32) {
        if self.state == SceneState::Queued(3) && now >= self.until {
            self.state = SceneState::Running;
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
        if a.wake <= now {
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
    pub log: Vec<String>,
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
            activity: Activity::Idle,
            programs: HashMap::new(),
            walk_frames: None,
            scenery: None,
            scenery_night: false,
            scenery_gen: 0,
            tick: 0,
            log: Vec::new(),
        }
    }

    fn note(&mut self, line: String) {
        self.log.push(line);
    }

    /// Does this scene's declared position match where Johnny is?
    ///
    /// `seg4:0x0B64` treats a major of 0 as "don't care", which is what the
    /// `(0, 0)` entries in the table mean.
    fn position_ok(&self, scene: &Scene) -> bool {
        scene.from.0 == 0 || scene.from == self.position
    }

    /// The chooser at `seg4:0x0D76`: filter, sum the weights, draw against the
    /// total. The story path is separate — it matches on the day instead of
    /// the category mask, and is offered on a one-in-ten roll.
    pub fn choose(&mut self, category: u32, budget: u8) -> Option<&'static Scene> {
        let mut candidates: Vec<&'static Scene> = Vec::new();
        let mut total = 0u32;
        for scene in tables::SCENES {
            if scene.sequence == 0 || scene.weight == 0 {
                continue;
            }
            if scene.mask & category == 0 || scene.day != 0 {
                continue;
            }
            if self.phase < scene.min_phase || self.phase >= scene.max_phase {
                continue;
            }
            if scene.cost / 2 >= budget {
                continue;
            }
            total += scene.weight as u32;
            candidates.push(scene);
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

    /// The day's story scene, if the one-in-ten roll comes up.
    pub fn choose_story(&mut self) -> Option<&'static Scene> {
        if self.rng.below(10) != 0 {
            return None;
        }
        tables::SCENES
            .iter()
            .find(|s| s.day == self.day && s.sequence != 0)
    }

    /// Pick the next walk step towards `target`, from `seg4:0x0094`.
    fn next_step(&mut self, target: (u8, u8)) -> Option<&'static WalkStep> {
        let (from, to) = (self.position.0, target.0);
        if from == to || from == 0 || to == 0 {
            return None;
        }
        let options = tables::ROUTE_TABLE
            .iter()
            .find(|(f, t, at, _)| *f == from && *t == to && *at == from)
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
                self.note(format!("idle at {:?} for {} ticks", self.position, idle_ticks(scene)));
                self.activity = Activity::Idling {
                    until: self.tick + idle_ticks(scene),
                };
                return;
            }
            self.note(format!("scene id {} has no ADS file", scene.id));
            return;
        };
        if !self.position_ok(scene) {
            if let Some(step) = self.next_step(scene.from) {
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
                    target: scene.from,
                    next: self.tick,
                };
                return;
            }
            // No route: the original would keep trying. Move him directly so
            // the scene can still play, and say so.
            self.note(format!(
                "no route {:?} -> {:?}, moving directly",
                self.position, scene.from
            ));
            self.position = scene.from;
        }
        let Some(mut seq) = Sequencer::from_record(archive, file) else {
            self.note(format!("no ADS record {file}"));
            return;
        };
        if !seq.start(scene.sequence as u16) {
            self.note(format!("{file} has no sequence {}", scene.sequence));
            return;
        }
        let label = seq
            .labels
            .get(&(scene.sequence as u16))
            .cloned()
            .unwrap_or_else(|| "(unnamed)".into());
        self.note(format!(
            "play {file} seq {} \"{label}\" at {:?} phase {}",
            scene.sequence, self.position, self.phase
        ));
        self.activity = Activity::Scene {
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
        self.scenery_night = self.is_night();
        match Scenery::build(archive, self.scenery_night, roll, self.tick) {
            Some(s) => {
                self.note(format!(
                    "scenery: {}{}",
                    s.sea,
                    if s.extras { " + rock and sandbar" } else { "" }
                ));
                self.stage.set_background(s.compose());
                self.scenery_gen = self.stage.background_gen;
                self.scenery = Some(s);
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
        if !self.stage.has_background() {
            self.load_scenery(archive);
        }
        let surf = self.step_surf();
        surf | match std::mem::replace(&mut self.activity, Activity::Idle) {
            Activity::Idle => {
                // Night fell, or day broke, while the last scene was running.
                if self.scenery.is_some() && self.is_night() != self.scenery_night {
                    self.load_scenery(archive);
                }
                // Offer the day's story scene first, then an ordinary one.
                let scene = self
                    .choose_story()
                    .or_else(|| self.choose(category::PRIMARY, 100))
                    .or_else(|| self.choose(category::STANDALONE, 100));
                match scene {
                    Some(s) => self.begin_scene(archive, s),
                    None => self.note("nothing playable this phase".into()),
                }
                false
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
                target,
                next,
            } => {
                // Hold the current waypoint until its turn comes round.
                if self.tick < next {
                    self.activity = Activity::Walking {
                        step,
                        index,
                        target,
                        next,
                    };
                    return false;
                }
                self.stage.restore_background();
                if let Some(&(frame, x, y)) = step.path.get(index) {
                    let mirror = Mirror {
                        x: frame & 0x8000 != 0,
                        y: false,
                    };
                    self.draw_walk_frame(archive, frame & 0x7FFF, x, y, mirror);
                    self.activity = Activity::Walking {
                        step,
                        index: index + 1,
                        target,
                        next: self.tick + WALK_TICKS_PER_WAYPOINT,
                    };
                    true
                } else {
                    // Arrived at wherever this step lands. Keep going if the
                    // target is further on; the route block is re-entered.
                    self.position = step.lands;
                    if self.position.0 != target.0 {
                        if let Some(next_step) = self.next_step(target) {
                            self.activity = Activity::Walking {
                                step: next_step,
                                index: 0,
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
                if (done && settled) || over_time {
                    if ends_at.0 != 0 {
                        self.position = ends_at;
                    }
                    self.restore_island();
                    self.note(format!("scene over, Johnny at {:?}", self.position));
                    self.advance_clock();
                } else {
                    self.activity = Activity::Scene {
                        seq,
                        active,
                        ends_at,
                        ticks: ticks + 1,
                        done,
                    };
                }
                drew
            }
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
        if let Some(f) = frames.get(frame as usize) {
            self.stage
                .surface
                .draw_sprite(f, x as i32, y as i32, mirror);
        }
    }

    fn run_scene(
        &mut self,
        archive: &Archive,
        seq: &mut Sequencer,
        active: &mut Vec<Active>,
        done: &mut bool,
    ) -> bool {
        // One pass of the sequencer, not one instruction: `seg10:0x1925`
        // interprets until a `Step` and only then returns, so everything a
        // guarded block starts happens on the same tick. The bound is a
        // backstop against a script that never reaches one.
        for _ in 0..64 {
            match seq.next_action(&mut self.rng) {
                Action::Done => {
                    *done = true;
                    break;
                }
                Action::Wait => break,
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
                    if let Some(program) = self.program(archive, &file) {
                        let mut inst = Instance::new(program);
                        if inst.seek_scene(scene) {
                            // Playing a scene that is already going restarts
                            // it, the way `seg10:0x0BF4` hands back the
                            // instance it already has rather than making a
                            // second one.
                            active.retain(|a| (a.ttm, a.scene) != (ttm_id, scene));
                            active.push(Active::start(ttm_id, scene, inst, repeat, self.tick));
                        }
                    }
                }
            }
        }

        self.stage.restore_background();
        advance_scenes(active, seq, &mut self.stage, archive, self.tick);
        self.stage.has_background()
    }

    /// Move the day on. The story only advances on a real calendar day in the
    /// original; here each completed scene nudges the phase, and a wrap of the
    /// phase counter moves the story on.
    fn advance_clock(&mut self) {
        self.phase = (self.phase + 1) % PHASES_PER_DAY;
        if self.phase == 0 {
            self.day = if self.day >= STORY_DAYS { 1 } else { self.day + 1 };
            self.note(format!("--- day {} ---", self.day));
        }
    }

    /// How many waves are on the open water right now.
    pub fn wave_count(&self) -> usize {
        self.scenery.as_ref().map_or(0, |s| s.wave_count())
    }

    /// Ticks since the director started; one tick is `MS_PER_TICK`.
    pub fn ticks(&self) -> u32 {
        self.tick
    }

    pub fn is_night(&self) -> bool {
        self.phase >= FIRST_NIGHT_PHASE
    }
}
