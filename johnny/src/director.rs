//! The scheduler: what Johnny does next, and getting him there.
//!
//! This is the layer above the ADS sequencer. It picks a scene out of the
//! catalogue by weight, walks Johnny to wherever that scene needs him to be
//! standing, runs it, and keeps the clock and the story moving.
//!
//! All of it is read out of `SCRANTIC.EXE` segment 4; see `docs/SCHEDULER.md`
//! for the write-up and `tables.rs` for the data.

use crate::ads::{Action, Rng, Sequencer};
use crate::dgds::{self, Archive, Frame};
use crate::render::Mirror;
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

/// The story runs for eleven days and then wraps (`seg4:0x0BC8`).
pub const STORY_DAYS: u8 = 11;

/// How long one scene may hold the island before the director moves on.
pub const MAX_SCENE_TICKS: u32 = 240;

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
    /// Following one walk path towards `target`.
    Walking {
        step: &'static WalkStep,
        index: usize,
        target: (u8, u8),
    },
    /// Running a scene's ADS sequence.
    Scene {
        seq: Box<Sequencer>,
        active: Vec<Active>,
        ends_at: (u8, u8),
        ticks: u32,
    },
}

struct Active {
    ttm: u16,
    scene: u16,
    inst: Instance,
    wake: u32,
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
        Some(p)
    }

    /// Start a scene: walk there first if Johnny is somewhere else.
    fn begin_scene(&mut self, archive: &Archive, scene: &'static Scene) {
        let file = tables::ADS_FILES
            .iter()
            .find(|(id, _)| *id == scene.ads)
            .map(|(_, n)| *n);
        let Some(file) = file else {
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
        };
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
        let base = if self.is_night() {
            "NIGHT.SCR"
        } else {
            "ISLETEMP.SCR"
        };
        if let Some(body) = archive.get(base) {
            if let Ok(f) = dgds::read_screen(body) {
                self.stage.set_background(f);
            }
        }
        self.stage.restore_background();
        self.note(format!("scenery: {base}"));
    }

    /// Advance one tick. Returns true if the surface changed.
    pub fn tick(&mut self, archive: &Archive) -> bool {
        self.tick += 1;
        if !self.stage.has_background() {
            self.load_scenery(archive);
        }
        match std::mem::replace(&mut self.activity, Activity::Idle) {
            Activity::Idle => {
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
            Activity::Walking {
                step,
                index,
                target,
            } => {
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
                    };
                    true
                } else {
                    // Arrived at wherever this step lands. Keep going if the
                    // target is further on; the route block is re-entered.
                    self.position = step.lands;
                    if self.position.0 != target.0 {
                        if let Some(next) = self.next_step(target) {
                            self.activity = Activity::Walking {
                                step: next,
                                index: 0,
                                target,
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
            } => {
                let drew = self.run_scene(archive, &mut seq, &mut active);
                // Scenes loop, so a cap keeps the island moving. The original
                // ends them by deducting each scene's cost from a time budget;
                // this stands in for that until the budget is wired up.
                let over_time = ticks > MAX_SCENE_TICKS;
                if (active.is_empty() && ticks > 2) || over_time {
                    if ends_at.0 != 0 {
                        self.position = ends_at;
                    }
                    self.note(format!("scene over, Johnny at {:?}", self.position));
                    self.advance_clock();
                } else {
                    self.activity = Activity::Scene {
                        seq,
                        active,
                        ends_at,
                        ticks: ticks + 1,
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
    ) -> bool {
        match seq.next_action(&mut self.rng) {
            Action::Done => {}
            Action::Wait => {}
            Action::Play { file, scene } => {
                let ttm_id = seq
                    .files
                    .iter()
                    .find(|(_, f)| **f == file)
                    .map(|(id, _)| *id)
                    .unwrap_or(0);
                if let Some(program) = self.program(archive, &file) {
                    let mut inst = Instance::new(program);
                    if inst.seek_scene(scene) {
                        active.push(Active {
                            ttm: ttm_id,
                            scene,
                            inst,
                            wake: self.tick,
                        });
                    }
                }
            }
        }

        self.stage.restore_background();
        let mut finished = Vec::new();
        for a in active.iter_mut() {
            if a.wake > self.tick {
                continue;
            }
            seq.scene_running(a.ttm, a.scene);
            match a.inst.run_frame(&mut self.stage, archive) {
                ttm::Step::Finished => finished.push((a.ttm, a.scene)),
                ttm::Step::Frame(delay) => a.wake = self.tick + delay.max(1) as u32,
            }
        }
        for (t, s) in &finished {
            seq.scene_finished(*t, *s);
        }
        active.retain(|a| !finished.contains(&(a.ttm, a.scene)));
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

    pub fn is_night(&self) -> bool {
        self.phase >= FIRST_NIGHT_PHASE
    }
}
