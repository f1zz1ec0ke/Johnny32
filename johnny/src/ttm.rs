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
use std::rc::Rc;
use crate::render::{Clip, Mirror, Surface};

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
    /// The scene has finished a pass. `seg12:0x0F73` is two instructions --
    /// set `ds:0x4644` and carry on -- so the frame still finishes drawing;
    /// it is the ADS runner that reads the flag afterwards (`seg10:0x1D1A`)
    /// and decides from the repeat mode whether to start the scene again.
    /// Every scene ends with one: 528 across the 41 scripts.
    EndScene,
    /// Draw frame `frame` of slot `slot` at `x`,`y`.
    DrawSprite {
        x: i16,
        y: i16,
        frame: u16,
        slot: u16,
        mirror: Mirror,
    },
    /// Wait a random number of ticks between the two operands.
    /// `seg12:0x0E12` computes `min + rand() % (max - min)`, which is what
    /// keeps repeated animations from looking metronomic.
    SetRandomDelay { min: u16, max: u16 },
    /// Set a single pixel in the foreground colour.
    DrawPixel { x: i16, y: i16 },
    /// Set the clip rectangle. `seg12:0x0F40` stores the two x values
    /// together and the two y values together, so the operands are
    /// (x1, y1, x2, y2) and it also raises the clip-enabled flag.
    SetClip { x1: i16, y1: i16, x2: i16, y2: i16 },
    /// Draw a line between two points.
    DrawLine { x1: i16, y1: i16, x2: i16, y2: i16 },
    /// Draw a rectangle at (x, y) of the given size, filled or outlined.
    DrawRect { x: i16, y: i16, w: i16, h: i16, filled: bool },
    /// Draw an ellipse in the given bounding box, filled or outlined.
    DrawEllipse { x: i16, y: i16, w: i16, h: i16, filled: bool },
    /// Set the drawing colours: foreground then background. `seg12:0x06A9`
    /// stores them on the scene instance and mirrors them into the globals
    /// the vector-drawing opcodes read.
    SetColors { fg: u8, bg: u8 },
    /// Trigger sound effect `n`. `seg12:0x0063` hands it to `seg5:0x00BA`,
    /// which indexes a 25-entry table of loaded sounds and is gated on the
    /// `Sounds` setting in `SCRANTIC.INI`. This release ships no audio at all,
    /// so it does nothing here -- but the cues are in the scripts, 535 of them,
    /// ready for a release that has the WAVs.
    PlaySound(u16),
    /// The `A014`..`A094` and `A0B5` region family, dispatched at
    /// `seg12:0x02F6` into `seg7:0x1510` with the opcode, a rectangle and a
    /// mode. Two of them appear once each in the shipped scripts, so what
    /// each variant draws is not pinned down yet.
    RegionOp {
        opcode: u16,
        x: i16,
        y: i16,
        w: i16,
        h: i16,
    },
    /// `B606`: six operands into `seg8:0x06CE` at `seg12:0x0EBE`, with two of
    /// them converted first. A region copy of some kind; four call sites in
    /// the whole corpus.
    CopyRegion { args: Vec<i16> },
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
        0x0110 => Op::EndScene,

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

        0x2022 => Op::SetRandomDelay {
            min: a(0) as u16,
            max: a(1) as u16,
        },
        0xA002 => Op::DrawPixel { x: a(0), y: a(1) },
        0x4004 => Op::SetClip {
            x1: a(0),
            y1: a(1),
            x2: a(2),
            y2: a(3),
        },
        0xA0A4 => Op::DrawLine {
            x1: a(0),
            y1: a(1),
            x2: a(2),
            y2: a(3),
        },
        0xA104 | 0xA114 => Op::DrawRect {
            x: a(0),
            y: a(1),
            w: a(2),
            h: a(3),
            filled: opcode == 0xA104,
        },
        0xA404 | 0xA424 => Op::DrawEllipse {
            x: a(0),
            y: a(1),
            w: a(2),
            h: a(3),
            filled: opcode == 0xA404,
        },
        0x2002 => Op::SetColors {
            fg: a(0) as u8,
            bg: a(1) as u8,
        },
        0xC051 => Op::PlaySound(a(0) as u16),

        // Handlers that reach their "done" label down every path. 0x4214 is
        // the disabled predecessor of 0x4004: 60 call sites carry a rectangle,
        // but seg12:0x0860 jumps to "done" whichever way the guard goes.
        // 0x2012 sets a flag and calls seg12:0x197C, which is an empty
        // function -- prologue, epilogue, nothing between.
        //
        // 0x4204 saves a screen region (seg12:0x0E69). This renderer repaints
        // the whole background every tick, so there is nothing for a saved
        // region to restore and no-oping it is correct rather than lazy.
        0xA601 | 0x4214 | 0x5005 | 0x2012 | 0x4204 => Op::NoOp(opcode),

        0xA014 | 0xA024 | 0xA034 | 0xA044 | 0xA054 | 0xA064 | 0xA094 | 0xA0B5 => {
            Op::RegionOp {
                opcode,
                x: a(0),
                y: a(1),
                w: a(2),
                h: a(3),
            }
        }
        0xB606 => Op::CopyRegion { args },

        other => Op::Todo {
            opcode: other,
            args,
            text,
        },
    }
}

/// The resources a set of concurrently running scenes share: the loaded
/// bitmap slots, the palette, the background and the surface they paint on.
///
/// The original keeps these per ADS context rather than per scene — the
/// instance record carries only a cursor and some render state, and the
/// slots are common. That is why one `.TTM` file can have several of its
/// scenes live at once without reloading anything.
pub struct Stage {
    slots: Vec<Vec<Frame>>,
    background: Option<Frame>,
    /// Bumped every time the background is replaced. The director watches it
    /// to know whether the island it composed is still the one on show, or
    /// whether a scene has put its own backdrop up.
    pub background_gen: u64,
    /// The `.SCR` the last `LoadScreen` named, if any.
    pub last_screen: Option<String>,
    palette: [[u8; 3]; 256],
    pub surface: Surface,
    /// Current drawing colours, for the vector opcodes.
    pub fg: u8,
    pub bg: u8,
    /// Drives `SetRandomDelay`.
    pub rng: crate::ads::Rng,
    /// Opcodes decoded but not implemented, tallied so they stay visible.
    pub todo_seen: std::collections::BTreeMap<u16, usize>,
}

impl Default for Stage {
    fn default() -> Self {
        Stage {
            slots: (0..16).map(|_| Vec::new()).collect(),
            background: None,
            background_gen: 0,
            last_screen: None,
            palette: [[0; 3]; 256],
            surface: Surface::screen(),
            fg: 0,
            bg: 0,
            rng: crate::ads::Rng::new(0x7A11_5EED),
            todo_seen: Default::default(),
        }
    }
}

impl Stage {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn palette(&self) -> &[[u8; 3]; 256] {
        &self.palette
    }

    pub fn set_palette(&mut self, palette: [[u8; 3]; 256]) {
        self.palette = palette;
    }

    pub fn set_background(&mut self, frame: Frame) {
        self.surface.draw_background(&frame);
        self.background = Some(frame);
        self.background_gen += 1;
    }

    /// Paint a sprite permanently into the background, the way the island is
    /// composited over the sea before anything animates on top.
    pub fn blit_into_background(&mut self, sprite: &Frame, x: i32, y: i32) {
        let Some(mut bg) = self.background.take() else {
            return;
        };
        let saved = std::mem::replace(&mut self.surface.pixels, bg.pixels);
        let clip = self.surface.clip.take();
        self.surface
            .draw_sprite(sprite, x, y, crate::render::Mirror::none());
        bg.pixels = std::mem::replace(&mut self.surface.pixels, saved);
        self.surface.clip = clip;
        self.background = Some(bg);
        self.background_gen += 1;
    }

    /// Repaint the background. Scenes composite over this each tick.
    pub fn restore_background(&mut self) {
        if let Some(bg) = self.background.take() {
            self.surface.draw_background(&bg);
            self.background = Some(bg);
        }
    }

    pub fn has_background(&self) -> bool {
        self.background.is_some()
    }
}

/// A decoded `.TTM` program. Shared by every instance of the same file.
pub struct Program {
    pub ops: Vec<Op>,
}

impl Program {
    pub fn load(archive: &Archive, name: &str) -> Option<Program> {
        let body = archive.get(name)?;
        let tt3 = dgds::leaf(body, b"TT3:")?;
        let code = dgds::decompress(tt3.payload).ok()?;
        let (ops, _) = decode(&code);
        Some(Program { ops })
    }

    /// The instruction index just after a scene marker.
    pub fn scene_start(&self, scene: u16) -> Option<usize> {
        self.ops
            .iter()
            .position(|op| matches!(op, Op::SceneMarker(n) if *n == scene))
            .map(|i| i + 1)
    }
}

/// What one step of an instance produced.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// A frame is ready; hold it for this many ticks.
    Frame(u16),
    /// The program ran off the end.
    Finished,
}

/// One running scene: a cursor into a program, plus its own frame delay.
///
/// Several of these can be live at once and are advanced together, which is
/// how the fire, a seagull and Johnny end up in the same frame.
pub struct Instance {
    pub program: Rc<Program>,
    pub pc: usize,
    pub delay: u16,
    bitmap_slot: usize,
    palette_slot: usize,
    /// `GotoFrame` does not jump on the spot. `seg12:0x0FC3` stashes the
    /// target and it is taken at the frame boundary — which is also what stops
    /// a scene that loops back on itself from spinning inside one call.
    pending_goto: Option<u16>,
    /// The jump the last frame boundary took, if it took one -- the engine's
    /// `ds:0x4646`, which `seg10:0x1D84` reads back after each frame.
    pub last_goto: Option<u16>,
    /// The frame just run contained an `EndScene`: the engine's `ds:0x4644`,
    /// zeroed before each frame at `seg10:0x1CAE` and read at `0x1D1A`.
    pub ended: bool,
}

impl Instance {
    pub fn new(program: Rc<Program>) -> Self {
        Instance {
            program,
            pc: 0,
            delay: 1,
            bitmap_slot: 0,
            palette_slot: 0,
            pending_goto: None,
            last_goto: None,
            ended: false,
        }
    }

    pub fn seek_scene(&mut self, scene: u16) -> bool {
        match self.program.scene_start(scene) {
            Some(i) => {
                self.pc = i;
                true
            }
            None => false,
        }
    }

    /// Run until the next `EndFrame`, painting into the stage.
    ///
    /// The instruction budget is a backstop: a malformed or misunderstood
    /// program that never reaches an `EndFrame` should end the scene, not hang
    /// the screensaver.
    pub fn run_frame(&mut self, stage: &mut Stage, archive: &Archive) -> Step {
        // Both of these are per-frame in the engine too: `seg10:0x1CAE` clears
        // the end flag and `seg12:0x0452` resets the goto target before the
        // frame runs.
        self.ended = false;
        let mut budget = 100_000u32;
        loop {
            budget -= 1;
            if budget == 0 {
                return Step::Finished;
            }
            let Some(op) = self.program.ops.get(self.pc).cloned() else {
                return Step::Finished;
            };
            self.pc += 1;
            match op {
                Op::EndFrame => {
                    self.last_goto = self.pending_goto.take();
                    if let Some(target) = self.last_goto {
                        if !self.seek_scene(target) {
                            return Step::Finished;
                        }
                    }
                    return Step::Frame(self.delay);
                }
                Op::LoadScreen(name) => {
                    if let Some(body) = archive.get(&name) {
                        if let Ok(f) = dgds::read_screen(body) {
                            stage.set_background(f);
                            stage.last_screen = Some(name);
                        }
                    }
                }
                Op::LoadBitmap(name) => {
                    if let Some(body) = archive.get(&name) {
                        if let Ok(frames) = dgds::read_bitmap(body) {
                            stage.slots[self.bitmap_slot] = frames;
                        }
                    }
                }
                Op::LoadPalette(name) => {
                    if let Some(body) = archive.get(&name) {
                        if let Ok(p) = dgds::read_palette(body) {
                            stage.palette = p;
                        }
                    }
                }
                Op::SelectBitmapSlot(n) => self.bitmap_slot = (n as usize).min(15),
                Op::SelectPaletteSlot(n) => self.palette_slot = (n as usize).min(15),
                Op::SetDelay(n) => self.delay = n,
                Op::SetRandomDelay { min, max } => {
                    self.delay = if max > min {
                        min + stage.rng.below((max - min) as u32) as u16
                    } else {
                        min
                    };
                }
                Op::DrawPixel { x, y } => {
                    let fg = stage.fg;
                    stage.surface.draw_line(x as i32, y as i32, x as i32, y as i32, fg);
                }
                Op::SetColors { fg, bg } => {
                    stage.fg = fg;
                    stage.bg = bg;
                }
                Op::SetClip { x1, y1, x2, y2 } => {
                    stage.surface.clip = Some(Clip {
                        x1: x1 as i32,
                        y1: y1 as i32,
                        x2: x2 as i32,
                        y2: y2 as i32,
                    });
                }
                Op::DrawLine { x1, y1, x2, y2 } => {
                    let fg = stage.fg;
                    stage
                        .surface
                        .draw_line(x1 as i32, y1 as i32, x2 as i32, y2 as i32, fg);
                }
                Op::DrawRect { x, y, w, h, filled } => {
                    let fg = stage.fg;
                    stage
                        .surface
                        .draw_rect(x as i32, y as i32, w as i32, h as i32, fg, filled);
                }
                Op::DrawEllipse { x, y, w, h, filled } => {
                    let fg = stage.fg;
                    stage
                        .surface
                        .draw_ellipse(x as i32, y as i32, w as i32, h as i32, fg, filled);
                }
                Op::FreeShapes => stage.slots[self.bitmap_slot].clear(),
                Op::DrawSprite {
                    x,
                    y,
                    frame,
                    slot,
                    mirror,
                } => {
                    let slot = (slot as usize).min(15);
                    if let Some(f) = stage.slots[slot].get(frame as usize) {
                        stage.surface.draw_sprite(f, x as i32, y as i32, mirror);
                    }
                }
                Op::GotoFrame(n) => self.pending_goto = Some(n),
                Op::EndScene => self.ended = true,
                // Markers, no-ops and the not-yet-implemented, all explicit.
                Op::SceneMarker(_)
                | Op::SetBackground(_)
                | Op::PlaySound(_)
                | Op::NoOp(_) => {}
                // Known, named, and not yet drawn. Counted rather than
                // silently skipped so they stay visible.
                Op::RegionOp { opcode, .. } => {
                    *stage.todo_seen.entry(opcode).or_insert(0) += 1;
                }
                Op::CopyRegion { .. } => {
                    *stage.todo_seen.entry(0xB606).or_insert(0) += 1;
                }
                Op::Todo { opcode, .. } => {
                    *stage.todo_seen.entry(opcode).or_insert(0) += 1;
                }
            }
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
