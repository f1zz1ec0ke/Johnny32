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

/// Every ADS opcode the shipped scripts contain, sequence labels aside.
/// `tools/verify.py` checks it still matches.
///
/// Five the dispatch table accepts never appear: `2000` (only `2005` is
/// used), `2015`, `2020`, `4010` and `F000`/`F210`. So "which of `4000` and
/// `4010` is the front of the draw order" is a question about one
/// instruction, four times.
pub const USED_IN_DATA: [u16; 18] = [
    0x1070, 0x1330, 0x1350, 0x1360, 0x1370, 0x1420, 0x1430, 0x1510, 0x1520, 0x2005, 0x2010,
    0x3010, 0x3020, 0x30FF, 0x4000, 0xF010, 0xF200, 0xFFFF,
];

/// The predicates the `10x0` and `13x0` families test, from the handler table
/// at `seg10:0x14EB`. `state` is the scene's run state and `started` its
/// separate started flag (the field at +0x2D).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Cond {
    Finished,
    NotFinished,
    NotStarted,
    Started,
    JustEnded,
    NotQueued,
    Queued,
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
            0x1050 | 0x1350 => Cond::JustEnded,
            0x1060 | 0x1360 => Cond::NotQueued,
            0x1070 | 0x1370 => Cond::Queued,
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
/// operand (`seg10:0x0DE3`, `0x0DF2`, `0x0E03`), and it **stays** there while
/// the scene animates -- there is no separate "running". State 4 is written
/// at `seg10:0x1D76`, when a frame carrying `EndScene` has used up the
/// scene's last repeat: it means *this one has just ended*. It lasts exactly
/// one pass, because `seg10:0x1DE2` clears it back to 0 the next time the
/// instance is stepped.
///
/// That single pass is the whole handshake the scripts are built on, and the
/// order within a pass is what makes it reliable: `seg10:0x1ACB` steps every
/// instance first and only then interprets the scripts, so a script always
/// gets to see the state-4 its own scene reached this pass.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum SceneState {
    /// No record at all, or explicitly cleared to 0.
    #[default]
    Idle,
    /// Queued: 1 plays once, 2 plays a set number of times, 3 loops. A scene
    /// is in one of these for as long as it is on screen.
    Queued(u8),
    /// State 4: the scene reached its end on this pass. One pass only.
    Over,
    /// State 5. Only `FinishScene` writes this (`seg10:0x0E92`); a scene that
    /// simply runs out goes back to `Idle` (`seg10:0x1DE2`).
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
    /// `1510`: the end of a guarded block. Inside one it closes the block
    /// and the pass carries on with the next rule; at the top level it ends
    /// the sequence's pass.
    ///
    /// `seg10:0x1588` sets the stop flag at `ds:0x460C`, and the interpreter
    /// loop at `seg10:0x1943` returns as soon as it sees it -- but the loop
    /// that returns is the *inner* one a true `13x0` guard recursed into
    /// (`seg10:0x1771`), and `seg10:0x194A` clears the flag on the way out.
    /// So the outer loop never sees it.
    Step,
    /// `1520`: the same, except it returns a null pointer rather than a live
    /// one (`seg10:0x158A` jumps to the null exit). That null travels all the
    /// way out through every enclosing guard, so it ends the whole sequence's
    /// pass however deeply nested it is. One instruction in the shipped data,
    /// in `ACTIVITY.ADS`.
    StepEnd,
    /// `1500`: scan a block (`seg10:0x106B`) and then stop, at
    /// `seg10:0x1567`. No shipped script uses it; decoded so that stays a
    /// fact rather than an assumption.
    SkipStep,
    /// Weighted random choice of one enclosed entry, up to `RandomEnd`.
    RandomBegin,
    RandomEnd,
    /// A weighted "play nothing" choice inside a random block.
    RandomSkip { weight: u16 },
    /// Reset a scene to idle (`seg10:0x0E9B` writes state 0).
    StopScene {
        ttm: u16,
        scene: u16,
        /// Its third operand, which is what a random block weighs it by.
        weight: u16,
    },
    /// Mark a scene finished (`seg10:0x0E6E` writes state 5).
    FinishScene {
        ttm: u16,
        scene: u16,
        weight: u16,
    },
    /// Move a scene in the draw order.
    ///
    /// Both handlers unlink the scene from the instance list and put it back
    /// somewhere else. `0x4000` (`seg10:0x0F8C`) walks to the tail and
    /// appends it; `0x4010` (`seg10:0x0EF2`) makes it the head. The runner
    /// walks head to tail, so the tail is drawn last: `0x4000` brings a scene
    /// to the front, `0x4010` sends it to the back.
    Reorder {
        ttm: u16,
        scene: u16,
        /// True for `0x4000`.
        to_front: bool,
    },
    /// `13x0`: guards the block that follows, up to the next `Step`.
    ///
    /// `seg10:0x1758`. True recurses into the interpreter to run the block
    /// and then returns to the rule after it, so the whole script is a list
    /// of rules re-tested every pass.
    If {
        cond: Cond,
        ttm: u16,
        scene: u16,
    },
    /// `10x0`: the same predicate, an entirely different instruction.
    ///
    /// `seg10:0x17A0` parks a resume pointer at *this* guard
    /// (`ctx+0x292+seq*4`), runs the block, and then returns null -- ending
    /// the sequence's pass. The next pass restarts at the guard rather than
    /// at the top of the sequence, so it is a "stay here while this holds"
    /// loop rather than a rule. When it goes false it clears the park and the
    /// consecutive-hit counter at `ctx+0xB2+seq*2` and carries on.
    ///
    /// One instruction in the shipped data -- a `1070` in `ACTIVITY.ADS`.
    Wait {
        cond: Cond,
        ttm: u16,
        scene: u16,
    },
    /// `1420` and `1430`, the two ways a predicate can be joined to the next.
    And,
    Or,
    /// `F010`: put a sequence into state 2, which is how it says it is done.
    /// The operand is a sequence id, or -1 for the one saying it.
    Stop { sequence: u16 },
    /// `FFFF`, the end of a sequence body.
    End,
    /// `F000`, `F200` and `F210` name another sequence of this same script and
    /// set it running alongside this one (`seg10:0x16C2` writes state 4).
    ///
    /// `STAND.ADS` settles what the operand is: every one of its twelve
    /// standing sequences opens with `F200 14`, and 14 is its own `TAG:`
    /// entry "STAND INIT" -- four instructions that put up the base pose,
    /// `MJAMBWLK.TTM` scene 42. Without it Johnny stands about with no Johnny
    /// in the picture.
    Run { opcode: u16, sequence: u16 },
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
            0x2010 => Op::StopScene {
                ttm: a(0),
                scene: a(1),
                weight: a(2),
            },
            0x2015 => Op::FinishScene {
                ttm: a(0),
                scene: a(1),
                weight: a(2),
            },
            0xF010 => Op::Stop { sequence: a(0) },
            0x4000 | 0x4010 => Op::Reorder {
                ttm: a(0),
                scene: a(1),
                to_front: w == 0x4000,
            },
            0x1500 => Op::SkipStep,
            0x1510 => Op::Step,
            0x1520 => Op::StepEnd,
            0x3010 => Op::RandomBegin,
            0x30FF => Op::RandomEnd,
            0x3020 => Op::RandomSkip { weight: a(0) },
            0xF000 | 0xF200 | 0xF210 => Op::Run {
                opcode: w,
                sequence: a(0),
            },
            // `seg10:0x13E7` reads these as links between predicates, and
            // they are not the same link: `1420` carries the chain on when
            // the last one was true and short-circuits when it was false,
            // `1430` the other way about.
            0x1420 => Op::And,
            0x1430 => Op::Or,
            0xFFFF => Op::End,
            other => match Cond::from_opcode(other) {
                // The two families share their predicates exactly -- the
                // table at `cs:0x14BD` sends `1010` and `1310` to the same
                // handler, and so on down -- but not their instructions.
                Some(cond) if other & 0x0F00 == 0x0300 => Op::If {
                    cond,
                    ttm: a(0),
                    scene: a(1),
                },
                Some(cond) => Op::Wait {
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
/// The engine's own generator, from `seg4:0x1E86`.
///
/// A fifty-six entry additive lagged Fibonacci: two cursors twenty-four apart
/// walk the table, and each call adds the entry under one to the entry under
/// the other, hands back the sum, and steps both on. The table and the two
/// cursors are constants in the data segment (`ds:0x1CE6`, `ds:0x1CE2` and
/// `ds:0x1CE4`), so with no seeding the sequence is fixed for all time.
///
/// It returns **sixteen bits**, which matters: every caller either divides
/// `dx:ax` with `dx` zeroed or compares the whole word against a weight scaled
/// by 655. A generator handing back thirty-two would land somewhere else.
///
/// Seeding is `seg4:0x1EDC`, which calls it `n` times and throws the results
/// away; `seg4:0x1EFA` does that at startup with the low word of `time(0)`.
/// So the whole run is determined by one number from 0 to 65535 -- which is
/// what makes it possible to check this port against the binary instead of
/// against another copy of my own reasoning.
#[derive(Clone, PartialEq, Eq)]
pub struct Rng {
    table: [u16; 56],
    i: usize,
    j: usize,
}

/// `ds:0x1CE6`, verbatim.
const SEED_TABLE: [u16; 56] = [
    0xDD23, 0x9533, 0xF3CD, 0x4BC2, 0xF7E2, 0xA2DF, 0xAA70, 0x60AA,
    0x144B, 0x8948, 0x192A, 0x6B0D, 0x9E0B, 0xFBF1, 0x8607, 0x465D,
    0xEAF4, 0x69FD, 0x6C44, 0x1B27, 0xDB71, 0xE38F, 0xABB4, 0xDC47,
    0x0FDE, 0x4D8E, 0xFC7D, 0x8D38, 0x5948, 0xE36F, 0xAA22, 0x2F26,
    0x43FA, 0xB5A5, 0x20EB, 0xED85, 0xF5C3, 0x819F, 0x2D9C, 0xB26D,
    0xE92B, 0x97D5, 0x3DA1, 0x89ED, 0xE3F4, 0x74B5, 0x002C, 0xD35F,
    0x3B08, 0x0DF5, 0xF27E, 0xC270, 0x2589, 0x2E8B, 0x6352, 0xDA2D,
];

impl Rng {
    /// Start the generator and discard `seed` values, which is the only kind
    /// of seeding the engine has. The count is a word there, so it wraps.
    pub fn new(seed: u64) -> Self {
        let mut rng = Rng {
            table: SEED_TABLE,
            i: 24,
            j: 55,
        };
        for _ in 0..(seed as u16) {
            rng.next_u16();
        }
        rng
    }

    /// One call of `rand()`.
    pub fn next_u16(&mut self) -> u16 {
        let sum = self.table[self.j].wrapping_add(self.table[self.i]);
        self.table[self.j] = sum;
        self.j = (self.j + 1) % 56;
        self.i = (self.i + 1) % 56;
        sum
    }

    /// `rand() % n`, the way every caller does it: `xor dx, dx` and a 16-bit
    /// divide.
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            0
        } else {
            self.next_u16() as u32 % n
        }
    }
}

/// What `PlayScene`'s third operand asks for, from `seg10:0x0DDB`.
///
/// The engine keeps it in the scene's run-state field: flags of zero is state
/// 1, a positive count is state 2 with the count at +0x31, and a negative
/// value is state 3 with a deadline of `tick + |flags|` at +0x33. When a
/// scene's program runs off the end, `seg10:0x1D2B` reads that state back to
/// decide whether to start it again.
///
/// The shipped scripts use `-60` through `-1000`, which at the engine's 16ms
/// tick is one second to sixteen. This is where a scene's length comes from.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Repeat {
    /// Flags 0: run the program through once.
    Once,
    /// Flags > 0: run it again this many times.
    Times(u16),
    /// Flags < 0: keep running it until this many ticks have passed.
    ForTicks(u32),
}

impl Repeat {
    /// The operand is signed, and the file stores it as a word.
    pub fn from_flags(flags: u16) -> Repeat {
        match flags as i16 {
            0 => Repeat::Once,
            n if n > 0 => Repeat::Times(n as u16),
            n => Repeat::ForTicks(n.unsigned_abs() as u32),
        }
    }

    /// The run-state the engine writes for this mode.
    pub fn state(self) -> SceneState {
        match self {
            Repeat::Once => SceneState::Queued(1),
            Repeat::Times(_) => SceneState::Queued(2),
            Repeat::ForTicks(_) => SceneState::Queued(3),
        }
    }

    /// The engine stores one less than the operand (`seg10:0x0DF9`).
    pub fn count(self) -> u16 {
        match self {
            Repeat::Times(n) => n.saturating_sub(1),
            _ => 0,
        }
    }
}

/// What the sequencer wants next.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Start this scene of this `.TTM` file.
    Play {
        file: String,
        scene: u16,
        repeat: Repeat,
    },
    /// Move a scene to the front or the back of the draw order.
    Reorder {
        ttm: u16,
        scene: u16,
        to_front: bool,
    },
    /// Nothing more to do this pass. Whether the scene is over is a separate
    /// question -- ask `running()`.
    PassOver,
}

pub struct Sequencer {
    ops: Vec<(usize, Op)>,
    pub files: BTreeMap<u16, String>,
    pub labels: BTreeMap<u16, String>,
    pc: usize,
    pub states: BTreeMap<(u16, u16), SceneState>,
    /// The state word of every sequence the script has, `es:[bx+0x12]` in the
    /// engine: 1 runs this pass, 2 is stopped, 4 runs from the next one.
    /// Several are live at once -- that is what `F200` is for.
    seqs: BTreeMap<u16, u8>,
    /// The sequence being interpreted right now, `ds:0x2606`.
    current: u16,
    /// How many guarded blocks deep the interpreter is. `seg10:0x1758` runs a
    /// true block through a nested `seg10:0x1925`, and `seg10:0x194A` swallows
    /// the yield that block ends on -- so a `Step` inside a block closes the
    /// block and the pass carries on, while one at the top closes the pass.
    depth: u16,
    /// How many times each scene has been started, `[si+0x2D]`: zeroed when
    /// the instance record is made (`seg10:0x0B8F`) and bumped by every
    /// `PlayScene` (`seg10:0x0E25`). It is never cleared, which is what makes
    /// `1330` mean "never played" rather than "not playing".
    plays: BTreeMap<(u16, u16), u32>,
    /// Record every action a pass takes, for `tools/oracle.py` to diff
    /// against the same pass run inside the original interpreter. Off in the
    /// screensaver; `jcoracle seqpass` turns it on.
    pub tracing: bool,
    pub trace: Vec<String>,
    /// Where a `10x0` guard parked each sequence (`ctx+0x292+seq*4`). A
    /// sequence with one resumes there instead of at its top.
    resume: BTreeMap<u16, usize>,
    /// How many passes running a `10x0` guard has held (`ctx+0xB2+seq*2`),
    /// which is what `1080` tests.
    hits: BTreeMap<u16, u32>,
    /// The nesting depth a `Wait` was entered at, so its block's `Step` can
    /// end the sequence's pass rather than merely close the block.
    wait_at: Option<u16>,
    /// Interpret one sequence and stop, instead of walking on to the next.
    ///
    /// The engine has two functions here and the port has one: `seg10:0x1925`
    /// interprets a single sequence, and `seg10:0x1BE8` is the driver that
    /// walks every sequence in turn and calls it. `begin_pass` is the driver;
    /// `begin_sequence` is `seg10:0x1925` on its own, which is what the
    /// oracle's `seqpass` compares against. Nothing in the screensaver sets
    /// it.
    solo: bool,
    pub todo_seen: BTreeMap<u16, usize>,
}

/// What a random block weighs an instruction by (`seg10:0x0C8D`): the word at
/// offset 6 -- the third operand -- except for the two that play a scene,
/// which use the fourth, and the bare weight, which is all operand.
fn weight_of(op: &Op) -> u16 {
    match op {
        Op::PlayScene { weight, .. } => *weight,
        Op::RandomSkip { weight } => *weight,
        Op::StopScene { weight, .. } | Op::FinishScene { weight, .. } => *weight,
        // Anything else has no third operand to read, so it weighs nothing
        // and cannot be drawn. Nothing shipped puts one in a block.
        _ => 0,
    }
}

/// The raw opcode an `Op` came from, for counting one this port meets
/// somewhere it did not expect.
pub fn opcode_of(op: &Op) -> u16 {
    match op {
        Op::Sequence(_) => 0x0001,
        Op::PlayScene { .. } => 0x2005,
        Op::StopScene { .. } => 0x2010,
        Op::FinishScene { .. } => 0x2015,
        Op::Reorder { to_front, .. } => {
            if *to_front {
                0x4000
            } else {
                0x4010
            }
        }
        Op::If { .. } => 0x1350,
        Op::And => 0x1420,
        Op::Or => 0x1430,
        Op::Stop { .. } => 0xF010,
        Op::End => 0xFFFF,
        Op::Run { opcode, .. } => *opcode,
        Op::Step => 0x1510,
        Op::StepEnd => 0x1520,
        Op::SkipStep => 0x1500,
        Op::Wait { .. } => 0x1070,
        Op::RandomBegin => 0x3010,
        Op::RandomEnd => 0x30FF,
        Op::RandomSkip { .. } => 0x3020,
        Op::Todo { opcode, .. } => *opcode,
    }
}

/// A pass over one sequence cannot execute more instructions than the script
/// has. This is a backstop against a script this port has misread, not a
/// budget anything approaches.
pub const PASS_BUDGET: usize = 100_000;

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
            seqs: BTreeMap::new(),
            current: 0,
            depth: 0,
            plays: BTreeMap::new(),
            tracing: false,
            trace: Vec::new(),
            resume: BTreeMap::new(),
            hits: BTreeMap::new(),
            wait_at: None,
            solo: false,
            todo_seen: BTreeMap::new(),
        })
    }

    /// Build a sequencer straight from a decoded script, with no file table
    /// and no labels. For tests and tools that have the bytes but not the
    /// archive they came out of.
    pub fn from_script(code: &[u8]) -> Sequencer {
        let (ops, _) = decode(code);
        Sequencer {
            ops,
            files: BTreeMap::new(),
            labels: BTreeMap::new(),
            pc: 0,
            states: BTreeMap::new(),
            seqs: BTreeMap::new(),
            current: 0,
            depth: 0,
            plays: BTreeMap::new(),
            tracing: false,
            trace: Vec::new(),
            resume: BTreeMap::new(),
            hits: BTreeMap::new(),
            wait_at: None,
            solo: false,
            todo_seen: BTreeMap::new(),
        }
    }

    pub fn ops(&self) -> &[(usize, Op)] {
        &self.ops
    }

    /// Where a numbered sequence's body begins.
    fn body_of(&self, sequence: u16) -> Option<usize> {
        self.ops
            .iter()
            .position(|(_, op)| matches!(op, Op::Sequence(n) if *n == sequence))
            .map(|i| i + 1)
    }

    /// Set a sequence running, the way the scheduler does.
    ///
    /// `seg4:0x07CE` calls `seg10:0x0874`, which is `seg10:0x0805(ctx, seq,
    /// 3)`: state **3**, not 1, and the sequence's parked resume pointer
    /// cleared. The driver turns 3 into 1 when it reaches the sequence
    /// (`seg10:0x1BE8`), but not before resetting every instance record that
    /// sequence owns (`seg10:0x1B33` calls `seg10:0x0B3E` on each): rewound
    /// to the scene's start, play count zero, run state zero.
    ///
    /// Both halves are already true here, because this port builds a
    /// `Sequencer` per scene and the engine keeps one context per ADS file
    /// loaded for the whole run (`seg10:0x04FC` only ever *selects* one).
    /// The difference that leaves is the play count `1330` reads: after two
    /// scenes from the same file, the engine still remembers what the first
    /// one played and this port does not.
    ///
    /// That difference is the repaint model, not an oversight. The engine
    /// leaves what a scene drew on the screen, so a backdrop scene guarded by
    /// `1330` needs to run once and no more. This port paints every tick from
    /// nothing, so the same scene has to run again to be on screen at all --
    /// `STAND.ADS` sequence 14 and `MJ_AMB.BMP` are exactly that, and
    /// remembering the play count across scenes would leave Johnny standing
    /// on an empty island.
    pub fn start(&mut self, sequence: u16) -> bool {
        if self.body_of(sequence).is_none() {
            return false;
        }
        self.resume.remove(&sequence);
        self.hits.remove(&sequence);
        self.seqs.insert(sequence, 3);
        true
    }

    /// Open a pass: whatever `F200` queued last time starts running now
    /// (`seg10:0x1BF2` turns state 4 and state 3 back into state 1), and the
    /// first runnable sequence is put under the cursor.
    ///
    /// A sequence is re-read **from the top** every pass. The engine never
    /// stores the pointer its interpreter stopped at -- `seg10:0x1C18` throws
    /// the return value away -- so a script is not a program with a counter,
    /// it is a list of guarded rules re-tested against the world every 16ms.
    /// That is what makes a cascade work: the block waiting on a scene fires
    /// on the pass that scene ends, however many passes later that is.
    pub fn begin_pass(&mut self) {
        self.solo = false;
        self.current = 0;
        self.next_sequence();
    }

    /// Interpret one named sequence and stop when its pass ends -- what
    /// `seg10:0x1925` does when the driver hands it a sequence. The oracle
    /// needs this because the binary's interpreter has no idea there are
    /// other sequences; walking on to them is the caller's doing.
    pub fn begin_sequence(&mut self, sequence: u16) -> bool {
        let Some(body) = self.body_of(sequence) else {
            return false;
        };
        self.solo = true;
        self.seqs.insert(sequence, 1);
        self.current = sequence;
        self.depth = 0;
        self.wait_at = None;
        self.pc = self.resume.get(&sequence).copied().unwrap_or(body);
        true
    }

    /// Move the cursor to the next sequence that wants to run this pass.
    ///
    /// `seg10:0x1BE8` turns states 3 and 4 into 1 **as it reaches each
    /// sequence**, not before the pass begins, and that is not a detail.
    /// `F200` is usually the first instruction of the sequence that uses it,
    /// so it runs again every pass and puts its target back into state 4. A
    /// port that converts every 4 up front therefore converts the one from
    /// *last* pass, watches this pass set it back to 4, and never runs it --
    /// so `STAND.ADS`'s twelve standing sequences never reach the four
    /// instructions that put Johnny in the picture.
    fn next_sequence(&mut self) -> bool {
        if self.solo {
            self.current = u16::MAX;
            return false;
        }
        let next = self
            .seqs
            .iter()
            .find(|(id, state)| matches!(**state, 1 | 3 | 4) && **id > self.current)
            .map(|(id, _)| *id);
        if let Some(id) = next {
            self.seqs.insert(id, 1);
        }
        self.depth = 0;
        self.wait_at = None;
        let start = |me: &Self, id: u16| {
            me.resume.get(&id).copied().or_else(|| me.body_of(id))
        };
        match next.and_then(|id| start(self, id).map(|pc| (id, pc))) {
            Some((id, pc)) => {
                self.current = id;
                self.pc = pc;
                true
            }
            None => {
                // Nothing wants this pass. Park the pointer past the end of
                // the script, because the caller cannot see this answer:
                // `begin_pass` returns nothing and `next_action` would go on
                // reading from wherever the *last* pass stopped.
                //
                // `seg10:0x1C0B` cannot make that mistake -- it calls the
                // interpreter with an explicit pointer, and only when the
                // sequence is in state 1 -- but here the pointer is a field,
                // and a field keeps its value. Left alone it walks: the
                // instruction after the `F010` that ended the sequence, then
                // the `FFFF` after that, then the next sequence's header, and
                // then that sequence's body -- which plays its scenes. A
                // scene ending would start the animation filed after it.
                self.current = u16::MAX;
                self.pc = self.ops.len();
                false
            }
        }
    }

    /// Is anything still going? `seg10:0x0766`: a sequence in state 1 or 4
    /// keeps the scene alive; so does any instance still on screen, which the
    /// caller checks separately because it owns the instance list.
    pub fn running(&self) -> bool {
        self.seqs.values().any(|s| *s == 1 || *s == 4)
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
            Cond::NotStarted => self.plays.get(&(ttm, scene)).copied().unwrap_or(0) == 0,
            Cond::Started => self.plays.get(&(ttm, scene)).copied().unwrap_or(0) > 0,
            // `seg10:0x13BD`: state 4 exactly. This is the one the scripts
            // are built out of -- five hundred of the six hundred and
            // thirty-odd predicates in the shipped data are this -- and it
            // does not mean "is playing". It means "has just ended", which is
            // what makes a cascade a cascade: each block starts the next
            // scene when the one before it has finished, not when it started.
            Cond::JustEnded => known && s == SceneState::Over,
            // `seg10:0x136F`: no record, state 4 or state 0. Not "idle" so
            // much as "not waiting its turn" -- it is true of a scene that is
            // playing right now as well as one that has been and gone.
            Cond::NotQueued => !known || s == SceneState::Over || s == SceneState::Idle,
            // `seg10:0x1384`: states 1, 2 and 3, queued but not yet advanced.
            Cond::Queued => known && queued,
            // The counter and story-variable predicates. No shipped script
            // uses them, so there is nothing here to be faithful to yet.
            // `seg10:0x132F`: how many passes running the `10x0` guard on
            // this sequence has held.
            Cond::CountAtMost
            | Cond::CountAtLeast
            | Cond::VarAtMost
            | Cond::VarAtLeast
            | Cond::VarEquals => true,
        }
    }

    /// A predicate and whatever `And`/`Or` links follow it (`seg10:0x1223`).
    ///
    /// Left to right and short-circuiting: a link whose answer is already
    /// decided skips the predicate on the other side of it rather than
    /// evaluating it. `Or` also sets a sticky flag that wins at the end
    /// (`seg10:0x14A2`), which is how the engine resolves a chain that mixes
    /// the two. No shipped script mixes them, but the rule is cheap to keep.
    fn eval_chain(&mut self, cond: Cond, ttm: u16, scene: u16) -> bool {
        let mut sticky = false;
        let mut result = self.test(cond, ttm, scene);
        loop {
            let is_or = match self.ops.get(self.pc) {
                Some((_, Op::Or)) => true,
                Some((_, Op::And)) => false,
                _ => break,
            };
            self.pc += 1;
            // Whether this link's answer is settled by what we have already.
            let decided = result == is_or;
            if decided {
                if is_or {
                    sticky = true;
                }
                // Short-circuit: step over the predicate on the far side.
                self.pc += 1;
            } else {
                if !is_or {
                    result = false;
                }
                match self.ops.get(self.pc).cloned() {
                    Some((_, Op::If { cond, ttm, scene })) => {
                        self.pc += 1;
                        result = self.test(cond, ttm, scene);
                    }
                    _ => break,
                }
            }
        }
        sticky || result
    }

    /// Skip a block that is not going to run (`seg10:0x106B`).
    ///
    /// The scan stops at this block's own `Step`. A condition met on the way
    /// is consumed **whole** -- its chain, its block, and the `Step` that ends
    /// that block -- because a rule nested inside a skipped rule must not
    /// steal its terminator. `BUILDING.ADS` builds its fire out of exactly
    /// that shape, and a skipper that stops at the first `Step` it sees leaves
    /// the sequence stuck for good.
    /// Returns true if it stopped on the end of the sequence rather than on a
    /// `Step`. The caller has to know: the end is left under the cursor for
    /// the main loop to act on, and a nested skip that hits it must hand that
    /// straight back up rather than let the level above re-read the condition
    /// it came from and start again.
    /// Swallow a block, the way `seg10:0x106B` does.
    ///
    /// Its eighteen-entry table at `cs:0x1142` is the whole specification.
    /// `1510` and `1500` end the block; `FFFF` ends the script; the fifteen
    /// `13x0` guards recurse, because a nested rule's own block belongs to
    /// it. **Everything else is skipped one instruction at a time** --
    /// including `1520`, and including the `10x0` guards, neither of which is
    /// in the table at all.
    ///
    /// Stopping early is the expensive mistake: whatever is left of the block
    /// then runs as though it were the next rule, so a guard that just said
    /// no still starts the scenes underneath it. That is several animations
    /// beginning at once, at the moment one ends.
    ///
    /// Returns true if it ran off the end without finding a terminator.
    fn skip_block(&mut self) -> bool {
        while let Some((_, op)) = self.ops.get(self.pc).cloned() {
            self.pc += 1;
            match op {
                Op::Step | Op::SkipStep => return false,
                Op::End => {
                    self.pc -= 1;
                    return true;
                }
                Op::If { .. } => {
                    self.skip_chain();
                    if self.skip_block() {
                        return true;
                    }
                }
                _ => {}
            }
        }
        true
    }

    /// A `10x0` guard that has gone false hands its block to a scanner of
    /// its own (`seg10:0x118A`), and that one is looking for a `1520` --
    /// not for the `1510` that ends an ordinary block. It resumes just past
    /// it, so whatever follows the `1520` is what the sequence does once the
    /// wait is over. Running off the end instead ends the pass.
    fn skip_to_stepend(&mut self) -> bool {
        while let Some((_, op)) = self.ops.get(self.pc).cloned() {
            match op {
                Op::End => return true,
                Op::StepEnd => {
                    self.pc += 1;
                    return false;
                }
                _ => self.pc += 1,
            }
        }
        true
    }

    /// Where a block starting at `from` ends, for the oracle: the same
    /// question `seg10:0x106B` is asked, answered by the same walk the
    /// sequencer uses to skip one.
    pub fn block_end(&mut self, from: usize) -> Option<usize> {
        self.pc = from;
        let overran = self.skip_block();
        if overran {
            None
        } else {
            // `seg10:0x106B` stops *on* the instruction that closes the
            // block; its caller is what steps past it.
            self.ops.get(self.pc - 1).map(|(off, _)| *off)
        }
    }

    /// Step over the `And`/`Or` links after a condition, and the predicates
    /// they join, without evaluating any of them.
    fn skip_chain(&mut self) {
        while matches!(
            self.ops.get(self.pc),
            Some((_, Op::And)) | Some((_, Op::Or))
        ) {
            self.pc += 1;
            if matches!(self.ops.get(self.pc), Some((_, Op::If { .. }))) {
                self.pc += 1;
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
            if matches!(op, Op::RandomEnd) {
                break;
            }
            // Every instruction in the block is a candidate, weighted by
            // `seg10:0x0C8D`. Only counting the ones that play a scene means
            // an entry that stops one can never come up -- which is how the
            // twelve standing sequences end, so they never did.
            let weight = weight_of(op) as u32;
            total += weight;
            entries.push((i, weight));
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
                if self.next_sequence() {
                    continue;
                }
                return Action::PassOver;
            };
            self.pc += 1;
            match op {
                // The end of this sequence's body. On to the next one that
                // wants a pass, and when there is none the pass is over.
                Op::Sequence(_) | Op::End => {
                    if self.next_sequence() {
                        continue;
                    }
                    return Action::PassOver;
                }
                // `seg10:0x1669`: `F010` puts a sequence into state 2, which
                // is how a sequence says it is done. An operand of -1 means
                // this one, and then the pass stops here as well.
                Op::Stop { sequence } => {
                    let target = if sequence == 0xFFFF {
                        self.current
                    } else {
                        sequence
                    };
                    // Not traced: `seg10:0x1669` writes the state itself
                    // rather than calling a handler, so there is nothing for
                    // the oracle to watch. The sequence states it compares
                    // afterwards cover this instruction instead.
                    self.seqs.insert(target, 2);
                    if target == self.current {
                        if self.next_sequence() {
                            continue;
                        }
                        return Action::PassOver;
                    }
                }
                // Not a yield across ticks. `seg10:0x194A` swallows the flag
                // inside a guarded block, so a `Step` ends the block it is in
                // and the pass carries on to the next rule. Reaching one at
                // the top level ends the pass.
                // A `Step` inside a guarded block closes the block, and the
                // pass carries on with the next rule.
                Op::Step | Op::SkipStep => {
                    if self.depth > 0 {
                        self.depth -= 1;
                        // The block a `10x0` guard opened. Its pass ends here.
                        if self.wait_at == Some(self.depth) {
                            self.wait_at = None;
                            if self.next_sequence() {
                                continue;
                            }
                            return Action::PassOver;
                        }
                        continue;
                    }
                    if self.next_sequence() {
                        continue;
                    }
                    return Action::PassOver;
                }
                // `1520` does not. Its null return climbs out through every
                // enclosing guard, so the sequence is done for this pass
                // however deep it was.
                Op::StepEnd => {
                    self.depth = 0;
                    if self.next_sequence() {
                        continue;
                    }
                    return Action::PassOver;
                }
                Op::If { cond, ttm, scene } => {
                    // `seg10:0x1758`: evaluate, then either run the block or
                    // skip it. There is no third answer -- the engine never
                    // sits on a guard waiting for one to settle.
                    if self.eval_chain(cond, ttm, scene) {
                        self.depth += 1;
                        continue;
                    }
                    self.skip_block();
                }
                // `seg10:0x17A0`. True: park here, count the hit, run the
                // block, and the sequence is done for this pass. False: clear
                // both and carry on with the instruction after the block.
                Op::Wait { cond, ttm, scene } => {
                    let at = self.pc - 1;
                    if self.eval_chain(cond, ttm, scene) {
                        let id = self.current;
                        self.resume.insert(id, at);
                        *self.hits.entry(id).or_insert(0) += 1;
                        self.wait_at = Some(self.depth);
                        self.depth += 1;
                        continue;
                    }
                    let id = self.current;
                    self.resume.remove(&id);
                    self.hits.remove(&id);
                    if self.skip_to_stepend() && self.next_sequence() {
                        continue;
                    }
                }
                Op::And | Op::Or => {}
                Op::RandomBegin => {
                    // `seg10:0x0D9C` hands the entry it landed on to the
                    // general dispatcher, so a random block can hold anything.
                    // The shipped scripts use three: play a scene, stop one,
                    // and a bare weight that means "do nothing this time".
                    if let Some(idx) = self.pick_random(rng) {
                        match self.ops.get(idx).cloned() {
                            Some((_, Op::PlayScene { ttm, scene, flags, .. })) => {
                                if let Some(action) = self.begin(ttm, scene, flags) {
                                    return action;
                                }
                            }
                            Some((_, Op::StopScene { ttm, scene, .. })) => {
                                self.stop_scene(ttm, scene);
                            }
                            Some((_, Op::FinishScene { ttm, scene, .. })) => {
                                self.finish_scene(ttm, scene);
                            }
                            Some((_, Op::RandomSkip { .. })) | None => {}
                            Some((_, other)) => {
                                *self.todo_seen.entry(opcode_of(&other)).or_insert(0) += 1;
                            }
                        }
                    }
                }
                Op::RandomEnd | Op::RandomSkip { .. } => {}
                Op::StopScene { ttm, scene, .. } => self.stop_scene(ttm, scene),
                Op::FinishScene { ttm, scene, .. } => self.finish_scene(ttm, scene),
                Op::PlayScene {
                    ttm, scene, flags, ..
                } => {
                    if let Some(action) = self.begin(ttm, scene, flags) {
                        return action;
                    }
                }
                Op::Reorder {
                    ttm,
                    scene,
                    to_front,
                } => {
                    self.note(format!(
                        "{} {ttm} {scene}",
                        if to_front { "front" } else { "back" }
                    ));
                    return Action::Reorder {
                        ttm,
                        scene,
                        to_front,
                    };
                }
                // `seg10:0x16C2`: mark that sequence state 4, so the next
                // pass runs it alongside this one.
                Op::Run { opcode, sequence } => {
                    self.note(format!("setseq {sequence}"));
                    if self.body_of(sequence).is_some() {
                        self.seqs.insert(sequence, 4);
                    } else {
                        *self.todo_seen.entry(opcode).or_insert(0) += 1;
                    }
                }
                Op::Todo { opcode, .. } => {
                    *self.todo_seen.entry(opcode).or_insert(0) += 1;
                }
            }
        }
    }

    /// `seg10:0x0E9B`: back to state 0. A random block can hold one of
    /// these as readily as a `PlayScene`, so both ways in go through here --
    /// otherwise the one inside the block is invisible to the oracle, which
    /// is precisely where a difference would hide.
    fn stop_scene(&mut self, ttm: u16, scene: u16) {
        self.note(format!("stop {ttm} {scene}"));
        self.states.insert((ttm, scene), SceneState::Idle);
    }

    /// `seg10:0x0E6E`: state 5.
    fn finish_scene(&mut self, ttm: u16, scene: u16) {
        self.note(format!("finish {ttm} {scene}"));
        self.states.insert((ttm, scene), SceneState::Finished);
    }

    /// Record one action, when the oracle is watching.
    fn note(&mut self, line: String) {
        if self.tracing {
            self.trace.push(line);
        }
    }

    fn begin(&mut self, ttm: u16, scene: u16, flags: u16) -> Option<Action> {
        self.note(format!("play {ttm} {scene} {flags}"));
        let file = self.files.get(&ttm)?.clone();
        *self.plays.entry((ttm, scene)).or_insert(0) += 1;
        let repeat = Repeat::from_flags(flags);
        self.states.insert((ttm, scene), repeat.state());
        Some(Action::Play {
            file,
            scene,
            repeat,
        })
    }

    /// Put a scene into a known state before a pass, for the oracle: the
    /// instance record's play count at +0x2D and run state at +0x2F.
    pub fn preload(&mut self, ttm: u16, scene: u16, played: u32, state: u16) {
        self.plays.insert((ttm, scene), played);
        self.states.insert(
            (ttm, scene),
            match state {
                1 | 2 | 3 => SceneState::Queued(state as u8),
                4 => SceneState::Over,
                5 => SceneState::Finished,
                _ => SceneState::Idle,
            },
        );
    }

    /// Every sequence the script defines and the state it is in, in id
    /// order. A sequence nothing has touched is state 0, the same as the
    /// engine's freshly zeroed context.
    pub fn sequence_states(&self) -> Vec<(u16, u8)> {
        let mut out: Vec<(u16, u8)> = self
            .ops
            .iter()
            .filter_map(|(_, op)| match op {
                Op::Sequence(n) => Some(*n),
                _ => None,
            })
            .map(|id| (id, self.seqs.get(&id).copied().unwrap_or(0)))
            .collect();
        out.sort();
        out.dedup_by_key(|(id, _)| *id);
        out
    }

    /// A scene's run state as the number the instance record holds at +0x2F.
    pub fn raw_state(&self, ttm: u16, scene: u16) -> u16 {
        match self.state(ttm, scene) {
            SceneState::Idle => 0,
            SceneState::Queued(n) => n as u16,
            SceneState::Over => 4,
            SceneState::Finished => 5,
        }
    }

    /// The runner owns a scene's run state once it has started it; this is
    /// how it hands the state back. `seg10:0x1D2B` onwards is the only place
    /// states 1, 2, 3 and 4 ever change.
    pub fn set_state(&mut self, ttm: u16, scene: u16, state: SceneState) {
        self.states.insert((ttm, scene), state);
    }

    /// What the sequence currently believes about a scene.
    pub fn state_of(&self, ttm: u16, scene: u16) -> SceneState {
        self.state(ttm, scene)
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
