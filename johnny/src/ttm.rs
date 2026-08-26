//! The TTM animation interpreter.
//!
//! A `.TTM` record is a program that paints one scene, frame by frame. Opcodes
//! are 16-bit; the low nibble is the operand count, and `0x0F` means a single
//! inline NUL-terminated string padded to a word boundary. The interpreter in
//! `SCRANTIC.EXE` segment 12 confirms this at `seg12:0x0461`.
//!
//! Every opcode the original knows about is represented here. Ones it accepts
//! but does nothing with are `Op::NoOp`, and ones this port has not implemented
//! yet are `Op::Todo` — both carry the raw opcode, so an instruction can be
//! ignored but never silently disappear.

use crate::dgds::{self, Archive, Frame};
use crate::render::{Mirror, Surface};

/// Every opcode the original interpreter dispatches, from the three tables in
/// segment 12 (`0x12BD`, `0x03BC` + `0x1900`, `0x00FC`).
pub const GENERAL: [u16; 47] = [
    0x0010, 0x0020, 0x0070, 0x0080, 0x0090, 0x00C0, 0x00E0, 0x0110, 0x0400, 0x0500, 0x0510,
    0x1011, 0x1021, 0x1031, 0x1041, 0x1051, 0x1061, 0x1071, 0x1101, 0x1111, 0x1121, 0x1201,
    0x2002, 0x2012, 0x2022, 0x2032, 0x2302, 0x2312, 0x2322, 0x2402, 0x3003, 0x3103, 0x4004,
    0x4114, 0x4124, 0x4136, 0x4204, 0x4214, 0x5005, 0xB000, 0xB013, 0xB606, 0xF01F, 0xF02F,
    0xF04F, 0xF05F, 0xF08F,
];

pub const DRAW: [u16; 28] = [
    0xA002, 0xA014, 0xA024, 0xA034, 0xA044, 0xA054, 0xA064, 0xA094, 0xA0A4, 0xA0B5, 0xA104,
    0xA114, 0xA404, 0xA424, 0xA504, 0xA506, 0xA514, 0xA516, 0xA524, 0xA526, 0xA534, 0xA536,
    0xA5A7, 0xA601, 0xA704, 0xAF02, 0xAF1F, 0xAF2F,
];

pub const CFAMILY: [u16; 18] = [
    0xC01F, 0xC02F, 0xC031, 0xC041, 0xC051, 0xC061, 0xC071, 0xC081, 0xC091, 0xC0A1, 0xC0B2,
    0xC0C2, 0xC0D3, 0xC0E3, 0xC0F4, 0xC102, 0xCF01, 0xCF11,
];

pub fn is_known(opcode: u16) -> bool {
    matches!(opcode, 0x0FF0 | 0x1301 | 0x1311)
        || GENERAL.contains(&opcode)
        || DRAW.contains(&opcode)
        || CFAMILY.contains(&opcode)
}

/// One decoded instruction.
#[derive(Debug, Clone)]
pub enum Op {
    /// Present what has been drawn and pause for the current delay.
    EndFrame,
    /// Load a `.SCR` as the background.
    LoadScreen(String),
    /// Load a `.BMP` into the selected bitmap slot.
    LoadBitmap(String),
    /// Load a `.PAL` into the selected palette slot.
    LoadPalette(String),
    SelectBitmapSlot(u16),
    SelectPaletteSlot(u16),
    SetBackground(u16),
    /// Frames to wait at the next `EndFrame`.
    SetDelay(u16),
    /// Marks the start of a numbered scene. The original jumps straight to its
    /// "done" label for this, so it is a marker for the caller, not an action.
    SceneMarker(u16),
    GotoFrame(u16),
    /// Release the bitmap held in the selected slot.
    FreeShapes,
    Purge,
    /// Draw frame `frame` of slot `slot` at `x`,`y`.
    DrawSprite {
        x: i16,
        y: i16,
        frame: u16,
        slot: u16,
        mirror: Mirror,
    },
    /// Trigger sound effect `n`. `seg12:0x0063` hands it to `seg5:0x00BA`,
    /// which indexes a 25-entry table of loaded sounds and is gated on the
    /// `Sounds` setting in `SCRANTIC.INI`. This release ships no audio at all,
    /// so it does nothing here -- but the cues are in the scripts, 535 of them,
    /// ready for a release that has the WAVs.
    PlaySound(u16),
    /// Accepted by the original interpreter, but its handler does nothing.
    NoOp(u16),
    /// Decoded correctly, not yet implemented here. Carries everything needed
    /// to implement it later, and is counted rather than ignored.
    Todo {
        opcode: u16,
        args: Vec<i16>,
        text: Option<String>,
    },
}

/// Decode a TTM program. Stops at the first word that is not a known opcode.
pub fn decode(code: &[u8]) -> (Vec<Op>, usize) {
    let mut out = Vec::new();
    let mut p = 0usize;
    while p + 2 <= code.len() {
        let w = u16::from_le_bytes([code[p], code[p + 1]]);
        if !is_known(w) {
            break;
        }
        let count = (w & 0x0F) as usize;
        let (args, text, next) = if count == 0x0F {
            let Some(end) = code[p + 2..].iter().position(|&b| b == 0).map(|i| p + 2 + i) else {
                break;
            };
            let s = String::from_utf8_lossy(&code[p + 2..end]).into_owned();
            let mut next = end + 1;
            if next & 1 == 1 {
                next += 1;
            }
            (Vec::new(), Some(s), next)
        } else {
            if p + 2 + count * 2 > code.len() {
                break;
            }
            let args: Vec<i16> = (0..count)
                .map(|i| i16::from_le_bytes([code[p + 2 + i * 2], code[p + 3 + i * 2]]))
                .collect();
            (args, None, p + 2 + count * 2)
        };
        out.push(build(w, args, text));
        p = next;
    }
    (out, p)
}

fn build(opcode: u16, args: Vec<i16>, text: Option<String>) -> Op {
    let a = |i: usize| args.get(i).copied().unwrap_or(0);
    match opcode {
        0x0FF0 => Op::EndFrame,
        0xF01F => Op::LoadScreen(text.unwrap_or_default()),
        0xF02F => Op::LoadBitmap(text.unwrap_or_default()),
        0xF05F => Op::LoadPalette(text.unwrap_or_default()),
        0x1051 => Op::SelectBitmapSlot(a(0) as u16),
        0x1061 => Op::SelectPaletteSlot(a(0) as u16),
        0x1121 => Op::SetBackground(a(0) as u16),
        0x1021 => Op::SetDelay(a(0) as u16),
        0x1101 | 0x1111 => Op::SceneMarker(a(0) as u16),
        0x1201 => Op::GotoFrame(a(0) as u16),
        0x0080 => Op::FreeShapes,
        0x0110 => Op::Purge,

        // The A5xx family is one handler at seg12:0x025B. Operands are
        // x, y, frame, slot, and the mirror mode is (opcode & 0x00F0) >> 4.
        // Low nibble 7 returns without drawing, and 6 takes the extended
        // six-operand path this port does not need yet.
        0xA504 | 0xA514 | 0xA524 | 0xA534 => Op::DrawSprite {
            x: a(0),
            y: a(1),
            frame: a(2) as u16,
            slot: a(3) as u16,
            mirror: Mirror::from_mode((opcode & 0x00F0) >> 4),
        },
        0xA5A7 => Op::NoOp(opcode),

        0xC051 => Op::PlaySound(a(0) as u16),

        // Handlers that reach their "done" label down every path. 0x4214 is
        // the surprising one: 60 call sites carry rectangle-shaped operands,
        // so it was probably clipping once, but seg12:0x0860 jumps to "done"
        // whichever way the guard goes.
        0xA601 | 0x4214 | 0x5005 => Op::NoOp(opcode),

        other => Op::Todo {
            opcode: other,
            args,
            text,
        },
    }
}

/// A loaded bitmap slot.
#[derive(Default)]
struct Slot {
    frames: Vec<Frame>,
}

/// What one step of the interpreter produced.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// A frame is ready; hold it for this many delay units.
    Frame(u16),
    /// The program ran off the end.
    Finished,
}

pub struct Machine {
    ops: Vec<Op>,
    pc: usize,
    bitmap_slot: usize,
    palette_slot: usize,
    slots: Vec<Slot>,
    background: Option<Frame>,
    palette: [[u8; 3]; 256],
    delay: u16,
    /// Opcodes decoded but not implemented, tallied so they are visible.
    pub todo_seen: std::collections::BTreeMap<u16, usize>,
    pub surface: Surface,
}

impl Machine {
    pub fn new(ops: Vec<Op>) -> Self {
        Machine {
            ops,
            pc: 0,
            bitmap_slot: 0,
            palette_slot: 0,
            slots: (0..16).map(|_| Slot::default()).collect(),
            background: None,
            palette: [[0; 3]; 256],
            delay: 1,
            todo_seen: Default::default(),
            surface: Surface::screen(),
        }
    }

    /// Decode a `.TTM` record from the archive and prepare it to run.
    pub fn from_record(archive: &Archive, name: &str) -> Option<Self> {
        let body = archive.get(name)?;
        let tt3 = dgds::leaf(body, b"TT3:")?;
        let code = dgds::decompress(tt3.payload).ok()?;
        let (ops, _) = decode(&code);
        Some(Machine::new(ops))
    }

    pub fn palette(&self) -> &[[u8; 3]; 256] {
        &self.palette
    }

    /// Jump to the instruction after a given scene marker.
    pub fn seek_scene(&mut self, scene: u16) -> bool {
        for (i, op) in self.ops.iter().enumerate() {
            if matches!(op, Op::SceneMarker(n) if *n == scene) {
                self.pc = i + 1;
                return true;
            }
        }
        false
    }

    /// Run until the next `EndFrame`, painting into `self.surface`.
    pub fn run_frame(&mut self, archive: &Archive) -> Step {
        loop {
            let Some(op) = self.ops.get(self.pc).cloned() else {
                return Step::Finished;
            };
            self.pc += 1;
            match op {
                Op::EndFrame => return Step::Frame(self.delay),
                Op::LoadScreen(name) => {
                    if let Some(body) = archive.get(&name) {
                        if let Ok(f) = dgds::read_screen(body) {
                            self.surface.draw_background(&f);
                            self.background = Some(f);
                        }
                    }
                }
                Op::LoadBitmap(name) => {
                    if let Some(body) = archive.get(&name) {
                        if let Ok(frames) = dgds::read_bitmap(body) {
                            self.slots[self.bitmap_slot].frames = frames;
                        }
                    }
                }
                Op::LoadPalette(name) => {
                    if let Some(body) = archive.get(&name) {
                        if let Ok(p) = dgds::read_palette(body) {
                            self.palette = p;
                        }
                    }
                }
                Op::SelectBitmapSlot(n) => self.bitmap_slot = (n as usize).min(15),
                Op::SelectPaletteSlot(n) => self.palette_slot = (n as usize).min(15),
                Op::SetDelay(n) => self.delay = n,
                Op::FreeShapes => self.slots[self.bitmap_slot].frames.clear(),
                Op::DrawSprite {
                    x,
                    y,
                    frame,
                    slot,
                    mirror,
                } => {
                    let slot = (slot as usize).min(15);
                    if let Some(f) = self.slots[slot].frames.get(frame as usize) {
                        self.surface.draw_sprite(f, x as i32, y as i32, mirror);
                    }
                }
                Op::GotoFrame(n) => {
                    // Restart the scene rather than run off the end.
                    if !self.seek_scene(n) {
                        return Step::Finished;
                    }
                }
                // Markers, no-ops and the not-yet-implemented, all explicit.
                Op::SceneMarker(_)
                | Op::SetBackground(_)
                | Op::Purge
                | Op::PlaySound(_)
                | Op::NoOp(_) => {}
                Op::Todo { opcode, .. } => {
                    *self.todo_seen.entry(opcode).or_insert(0) += 1;
                }
            }
        }
    }

    /// Repaint the background, which is what most scenes do between frames.
    pub fn restore_background(&mut self) {
        if let Some(bg) = &self.background {
            let bg = bg.clone();
            self.surface.draw_background(&bg);
        }
    }
}

impl Clone for Frame {
    fn clone(&self) -> Self {
        Frame {
            width: self.width,
            height: self.height,
            pixels: self.pixels.clone(),
        }
    }
}
