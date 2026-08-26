//! The ADS sequencer: which TTM scene plays, and when.
//!
//! An `.ADS` record holds numbered sequences. Each sequence is a run of
//! guarded blocks — a condition, some instructions, then a `Step` — and the
//! engine walks them, waiting at each `Step` for the scene it started to
//! finish.
//!
//! ADS does **not** use the TTM length rule. Its operand counts are not the
//! low nibble; each handler advances the script pointer itself, and the
//! lengths come from the dedicated function at `seg10:0x0303`. See
//! `docs/FILE_FORMATS.md`.

use crate::dgds::{self, Archive};
use std::collections::BTreeMap;

/// Instruction lengths, from the 35-entry table at `seg10:0x0383` whose
/// handlers do nothing but add a constant. Anything absent advances 2, which
/// covers the sequence labels, the block delimiters and the terminators.
pub fn length(opcode: u16) -> usize {
    match opcode {
        0x2000 | 0x2005 => 10,
        0x2010 | 0x2015 | 0x2020 | 0x4000 | 0x4010 => 8,
        0x1010..=0x1070 | 0x1310..=0x1370 if opcode & 0x000F == 0 => 6,
        0xF010 | 0xF200 | 0xF210 | 0x1080 | 0x1380 | 0x1390 | 0x3020 => 4,
        0x13A0 | 0x13A1 | 0x13B0 | 0x13B1 | 0x13C0 | 0x13C1 => 4,
        _ => 2,
    }
}

/// The predicates the `10x0` and `13x0` families test, from the handler table
/// at `seg10:0x14EB`. `state` is the scene's run state and `started` its
/// separate started flag (the field at +0x2D).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Cond {
    Finished,
    NotFinished,
    NotStarted,
    Started,
    Running,
    Idle,
    Paused,
    CountAtMost,
    CountAtLeast,
    VarAtMost,
    VarAtLeast,
    VarEquals,
}

impl Cond {
    fn from_opcode(op: u16) -> Option<Cond> {
        Some(match op {
            0x1010 | 0x1310 => Cond::Finished,
            0x1020 | 0x1320 => Cond::NotFinished,
            0x1030 | 0x1330 => Cond::NotStarted,
            0x1040 | 0x1340 => Cond::Started,
            0x1050 | 0x1350 => Cond::Running,
            0x1060 | 0x1360 => Cond::Idle,
            0x1070 | 0x1370 => Cond::Paused,
            0x1080 | 0x1380 => Cond::CountAtMost,
            0x1390 => Cond::CountAtLeast,
            0x13A0 | 0x13A1 => Cond::VarAtMost,
            0x13B0 | 0x13B1 => Cond::VarAtLeast,
            0x13C0 | 0x13C1 => Cond::VarEquals,
            _ => return None,
        })
    }
}

/// A scene's life cycle, in the field at +0x2F of its instance record.
///
/// `PlayScene` puts it in one of the queued states, depending on its flags
/// operand (`seg10:0x0DE3`, `0x0DF2`, `0x0E03`). The TTM runner promotes a
/// queued scene to `Running` the first time it advances it (`seg10:0x1D76`),
/// and `Finished` is set when the scene is stopped (`seg10:0x0E92`).
///
/// The ordering matters: `IF_RUNNING` on a scene is written immediately after
/// the `Step` that follows playing it, so a scene must reach `Running` during
/// that yield or the whole chain stalls.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum SceneState {
    /// No record at all, or explicitly cleared to 0.
    #[default]
    Idle,
    /// Queued: 1 plays once, 2 plays a set number of times, 3 loops.
    Queued(u8),
    /// State 4, being advanced by the runner.
    Running,
    /// State 5.
    Finished,
}

#[derive(Debug, Clone)]
pub enum Op {
    /// Start of sequence `n`; the `TAG:` table names them.
    Sequence(u16),
    /// Play `scene` of the `ttm`th file in the `RES:` table. `weight` is what
    /// a surrounding random block selects on.
    PlayScene {
        ttm: u16,
        scene: u16,
        flags: u16,
        weight: u16,
    },
    /// Wait here until the scene that was started finishes.
    Step,
    StepEnd,
    /// Weighted random choice of one enclosed entry, up to `RandomEnd`.
    RandomBegin,
    RandomEnd,
    /// A weighted "play nothing" choice inside a random block.
    RandomSkip { weight: u16 },
    /// Guards the block that follows, up to the next `Step`.
    If {
        cond: Cond,
        ttm: u16,
        scene: u16,
    },
    /// Chains onto the preceding condition.
    ChainOr,
    Stop,
    End,
    Todo { opcode: u16, args: Vec<u16> },
}

/// Decode an ADS program. Every shipped script consumes exactly.
pub fn decode(code: &[u8]) -> (Vec<(usize, Op)>, usize) {
    let mut out = Vec::new();
    let mut p = 0usize;
    while p + 2 <= code.len() {
        let w = u16::from_le_bytes([code[p], code[p + 1]]);
        let n = length(w);
        if p + n > code.len() {
            break;
        }
        let args: Vec<u16> = (0..(n - 2) / 2)
            .map(|i| u16::from_le_bytes([code[p + 2 + i * 2], code[p + 3 + i * 2]]))
            .collect();
        let a = |i: usize| args.get(i).copied().unwrap_or(0);
        let op = match w {
            0x0001..=0x00FF if w & 0xFF00 == 0 => Op::Sequence(w),
            0x2000 | 0x2005 => Op::PlayScene {
                ttm: a(0),
                scene: a(1),
                flags: a(2),
                weight: a(3),
            },
            0x1510 => Op::Step,
            0x1520 => Op::StepEnd,
            0x3010 => Op::RandomBegin,
            0x30FF => Op::RandomEnd,
            0x3020 => Op::RandomSkip { weight: a(0) },
            0x1420 | 0x1430 => Op::ChainOr,
            0xFFFF => Op::End,
            other => match Cond::from_opcode(other) {
                Some(cond) => Op::If {
                    cond,
                    ttm: a(0),
                    scene: a(1),
                },
                None => Op::Todo { opcode: other, args },
            },
        };
        out.push((p, op));
        p += n;
    }
    (out, p)
}

/// A tiny deterministic PRNG, so a run can be reproduced from a seed.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    pub fn next_u32(&mut self) -> u32 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            0
        } else {
            self.next_u32() % n
        }
    }
}

/// What the sequencer wants next.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Start this scene of this `.TTM` file.
    Play { file: String, scene: u16 },
    /// Wait for the running scene to finish.
    Wait,
    /// The sequence is over.
    Done,
}

pub struct Sequencer {
    ops: Vec<(usize, Op)>,
    pub files: BTreeMap<u16, String>,
    pub labels: BTreeMap<u16, String>,
    pc: usize,
    pub states: BTreeMap<(u16, u16), SceneState>,
    pub todo_seen: BTreeMap<u16, usize>,
}

impl Sequencer {
    /// Load an `.ADS` record: its `RES:` file table, `TAG:` labels and script.
    pub fn from_record(archive: &Archive, name: &str) -> Option<Self> {
        let body = archive.get(name)?;
        let scr = dgds::leaf(body, b"SCR:")?;
        let code = dgds::decompress(scr.payload).ok()?;
        let (ops, _) = decode(&code);
        Some(Sequencer {
            ops,
            files: dgds::leaf(body, b"RES:")
                .map(|l| id_table(l.payload))
                .unwrap_or_default(),
            labels: dgds::leaf(body, b"TAG:")
                .map(|l| id_table(l.payload))
                .unwrap_or_default(),
            pc: 0,
            states: BTreeMap::new(),
            todo_seen: BTreeMap::new(),
        })
    }

    pub fn ops(&self) -> &[(usize, Op)] {
        &self.ops
    }

    /// Jump to the start of a numbered sequence.
    pub fn start(&mut self, sequence: u16) -> bool {
        for (i, (_, op)) in self.ops.iter().enumerate() {
            if matches!(op, Op::Sequence(n) if *n == sequence) {
                self.pc = i + 1;
                return true;
            }
        }
        false
    }

    fn state(&self, ttm: u16, scene: u16) -> SceneState {
        self.states
            .get(&(ttm, scene))
            .copied()
            .unwrap_or(SceneState::Idle)
    }

    /// The predicates from `seg10:0x14EB`. "Absent" and `Idle` coincide here,
    /// because a scene the sequencer has never started has no record.
    fn test(&self, cond: Cond, ttm: u16, scene: u16) -> bool {
        let known = self.states.contains_key(&(ttm, scene));
        let s = self.state(ttm, scene);
        let queued = matches!(s, SceneState::Queued(_));
        match cond {
            Cond::Finished => known && s == SceneState::Finished,
            Cond::NotFinished => !known || s != SceneState::Finished,
            Cond::NotStarted => !known || s == SceneState::Idle,
            Cond::Started => known && s != SceneState::Idle,
            Cond::Running => known && s == SceneState::Running,
            Cond::Idle => !known || s == SceneState::Running || s == SceneState::Idle,
            // States 1, 2 and 3 -- queued but not yet advanced.
            Cond::Paused => known && queued,
            // The counter and story-variable predicates. No shipped script
            // uses them, so there is nothing here to be faithful to yet.
            Cond::CountAtMost
            | Cond::CountAtLeast
            | Cond::VarAtMost
            | Cond::VarAtLeast
            | Cond::VarEquals => true,
        }
    }

    /// Skip forward past the end of the current guarded block.
    fn skip_block(&mut self) {
        while let Some((_, op)) = self.ops.get(self.pc) {
            self.pc += 1;
            match op {
                Op::Step | Op::StepEnd => return,
                Op::End | Op::Sequence(_) => {
                    self.pc -= 1;
                    return;
                }
                _ => {}
            }
        }
    }

    /// Pick one entry of a random block, as `seg10:0x0CDA` does: sum the
    /// weights, draw below the total, walk until the running total passes it.
    ///
    /// The original draws `rand() % 100` here rather than against the sum,
    /// which is a bug where a block's weights do not total 100. That bug is in
    /// the walk router, not this one — this picker already uses the sum.
    fn pick_random(&mut self, rng: &mut Rng) -> Option<usize> {
        let start = self.pc;
        let mut entries = Vec::new();
        let mut total = 0u32;
        let mut i = start;
        while let Some((_, op)) = self.ops.get(i) {
            match op {
                Op::RandomEnd => break,
                Op::PlayScene { weight, .. } => {
                    total += *weight as u32;
                    entries.push((i, *weight as u32));
                }
                Op::RandomSkip { weight } => {
                    total += *weight as u32;
                    entries.push((i, *weight as u32));
                }
                _ => {}
            }
            i += 1;
        }
        let end = i + 1; // past RandomEnd
        let chosen = if total == 0 {
            None
        } else {
            let mut roll = rng.below(total) as i64 + 1;
            let mut hit = None;
            for (idx, w) in &entries {
                roll -= *w as i64;
                if roll <= 0 {
                    hit = Some(*idx);
                    break;
                }
            }
            hit
        };
        self.pc = end;
        chosen
    }

    /// Advance until something has to happen outside the sequencer.
    pub fn next_action(&mut self, rng: &mut Rng) -> Action {
        loop {
            let Some((_, op)) = self.ops.get(self.pc).cloned() else {
                return Action::Done;
            };
            self.pc += 1;
            match op {
                Op::Sequence(_) => return Action::Done,
                Op::End | Op::Stop => return Action::Done,
                Op::Step | Op::StepEnd => return Action::Wait,
                Op::If { cond, ttm, scene } => {
                    let mut ok = self.test(cond, ttm, scene);
                    // A chain continues while the next op is ChainOr.
                    while matches!(self.ops.get(self.pc), Some((_, Op::ChainOr))) {
                        self.pc += 1;
                        if let Some((_, Op::If { cond, ttm, scene })) = self.ops.get(self.pc).cloned()
                        {
                            self.pc += 1;
                            ok |= self.test(cond, ttm, scene);
                        }
                    }
                    if !ok {
                        self.skip_block();
                    }
                }
                Op::ChainOr => {}
                Op::RandomBegin => {
                    if let Some(idx) = self.pick_random(rng) {
                        if let Some((_, Op::PlayScene { ttm, scene, .. })) =
                            self.ops.get(idx).cloned()
                        {
                            if let Some(action) = self.begin(ttm, scene) {
                                return action;
                            }
                        }
                    }
                }
                Op::RandomEnd | Op::RandomSkip { .. } => {}
                Op::PlayScene { ttm, scene, .. } => {
                    if let Some(action) = self.begin(ttm, scene) {
                        return action;
                    }
                }
                Op::Todo { opcode, .. } => {
                    *self.todo_seen.entry(opcode).or_insert(0) += 1;
                }
            }
        }
    }

    fn begin(&mut self, ttm: u16, scene: u16) -> Option<Action> {
        let file = self.files.get(&ttm)?.clone();
        // Flags 0 means play once, which is all any shipped script uses.
        self.states.insert((ttm, scene), SceneState::Queued(1));
        Some(Action::Play { file, scene })
    }

    /// The runner has begun advancing this scene: queued -> running.
    pub fn scene_running(&mut self, ttm: u16, scene: u16) {
        self.states.insert((ttm, scene), SceneState::Running);
    }

    /// The scene has been stopped.
    pub fn scene_finished(&mut self, ttm: u16, scene: u16) {
        self.states.insert((ttm, scene), SceneState::Finished);
    }
}

/// `TAG:` and `RES:` share a layout: a count, then id and NUL-terminated name.
fn id_table(payload: &[u8]) -> BTreeMap<u16, String> {
    let mut out = BTreeMap::new();
    if payload.len() < 2 {
        return out;
    }
    let count = u16::from_le_bytes([payload[0], payload[1]]) as usize;
    let mut p = 2usize;
    for _ in 0..count {
        if p + 2 > payload.len() {
            break;
        }
        let id = u16::from_le_bytes([payload[p], payload[p + 1]]);
        p += 2;
        let Some(end) = payload[p..].iter().position(|&b| b == 0).map(|i| p + i) else {
            break;
        };
        out.insert(id, String::from_utf8_lossy(&payload[p..end]).into_owned());
        p = end + 1;
    }
    out
}
