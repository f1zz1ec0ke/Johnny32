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

/// Every TTM opcode the shipped scripts actually contain, counted out of
/// `RESOURCE.001`. `tools/verify.py` checks it still matches.
///
/// The list is not decoration: several readings of the disassembly differ
/// only where the data never goes, and this is what says so. `3003` is
/// absent, which is why the general pre-shift at `seg12:0x0569` never runs
/// and one island offset is the whole story. `AF1F`/`AF2F` are absent, which
/// is why the string-length rule has no exception to make. `5005` is absent,
/// and so is every `A0x4` region opcode but two.
pub const USED_IN_DATA: [u16; 30] = [
    0x0080, 0x0110, 0x0FF0, 0x1021, 0x1051, 0x1061, 0x1101, 0x1111, 0x1121, 0x1201, 0x2002,
    0x2012, 0x2022, 0x4004, 0x4204, 0x4214, 0xA002, 0xA054, 0xA064, 0xA0A4, 0xA104, 0xA404,
    0xA504, 0xA524, 0xA601, 0xB606, 0xC051, 0xF01F, 0xF02F, 0xF05F,
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
    /// A scene marker. Both `1101` and `1111` name a frame -- `seg12:0x13AA`
    /// looks a scene number up by walking the frame table and testing the
    /// first opcode of each frame against either -- but only `1111` gets an
    /// instance record at load time (`seg11:0x0645`), and a scene with no
    /// record cannot be played at all: `seg10:0x0DB6` looks one up and
    /// returns without doing anything when there is none. `SASKDATE.TTM` has
    /// the only five `1101` markers in the data; nothing plays them and they
    /// are reachable only by goto from inside the file.
    SceneMarker { scene: u16, playable: bool },
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
    /// The region family, from the second draw table at `seg12:0x1900`.
    ///
    /// Only two of the eight are ever used, and only once each, in
    /// `GJGULIVR.TTM` with the same rectangle: `A054` (`seg12:0x1747`) writes
    /// it into the structure at `ds:0x2638`, and `A064` (`seg12:0x1801`)
    /// walks that structure and draws it back. A save and a restore of one
    /// piece of screen -- which is what `B606` does the short way.
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
    /// Copy a rectangle from one page to another: `(x, y, w, h, from, to)`.
    ///
    /// `seg12:0x0EB4` resolves the page ids through `seg12:0x14E0` -- 0 is the
    /// working page, 1 the composite, 2 the island backdrop -- and hands the
    /// rectangle to the copier at `seg8:0x06CE`. All six uses in the data are
    /// `(.., 2, 1)`: put a piece of the island back over whatever has been
    /// drawn on top of it, which is how a scene rubs out what it drew.
    CopyRegion {
        x: i16,
        y: i16,
        w: i16,
        h: i16,
        from: i16,
        to: i16,
    },
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
            // `seg11:0x04a8` counts the bytes before the NUL and steps over
            // the NUL as well, then one byte further when that count is
            // even -- so a string always consumes an even number of bytes
            // and the next opcode stays word aligned.
            let text_len = end - (p + 2);
            let next = end + 1 + if text_len % 2 == 0 { 1 } else { 0 };
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
        0x1101 | 0x1111 => Op::SceneMarker {
            scene: a(0) as u16,
            playable: opcode == 0x1111,
        },
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
        0xB606 => Op::CopyRegion {
            x: a(0),
            y: a(1),
            w: a(2),
            h: a(3),
            from: a(4),
            to: a(5),
        },

        other => Op::Todo {
            opcode: other,
            args,
            text,
        },
    }
}

/// One resolved drawing operation, kept so a held frame can be painted again.
///
/// The engine leaves what a frame drew on the screen until something paints
/// over it, so it only ever runs a frame once. This port repaints the backdrop
/// every tick and so has to put every held frame back on top of it -- and
/// re-running the frame's instructions is *not* the same thing.
///
/// One set of bitmap slots is shared by every scene on screen, and slots are
/// loaded and freed from inside frames rather than from a header: 263
/// `LOAD_BITMAP`s and 190 `FREE_SHAPES` in the shipped scripts, all of them
/// inside scenes, and sixty-six frames that load or free artwork *and* draw
/// with it. By the time such a frame is painted again the slot it drew from
/// holds something else, or nothing. Re-running it then paints the wrong
/// sprite or no sprite at all, for as many ticks as the frame is held.
///
/// So what is kept is what the frame *drew*: the artwork itself, with the
/// colour, the clip and the island's offset already resolved. Replaying that
/// cannot load anything, cannot make a noise and cannot read a slot, which
/// makes the invariant the repaint model needs true by construction rather
/// than by discipline.
pub struct Paint {
    /// The clip in force when the frame drew this, already in screen
    /// coordinates.
    clip: Option<Clip>,
    mark: Mark,
}

enum Mark {
    Sprite {
        art: Rc<Frame>,
        x: i32,
        y: i32,
        mirror: Mirror,
    },
    Line {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        colour: u8,
    },
    Rect {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        colour: u8,
        filled: bool,
    },
    Ellipse {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        colour: u8,
        filled: bool,
    },
    /// `A064` and `B606`: put a rectangle of the backdrop back over whatever
    /// has been drawn on it. Kept in the list because it is part of the
    /// picture the frame made, not a side effect of making it.
    Restore {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    },
}

/// The resources a set of concurrently running scenes share: the loaded
/// bitmap slots, the palette, the background and the surface they paint on.
///
/// The original keeps these per ADS context rather than per scene — the
/// instance record carries only a cursor and some render state, and the
/// slots are common. That is why one `.TTM` file can have several of its
/// scenes live at once without reloading anything.
pub struct Stage {
    slots: Vec<Vec<Rc<Frame>>>,
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
    /// Sound ids the frames just run asked for, oldest first. The host
    /// drains it: making a noise is the one thing left that has to know what
    /// platform it is on.
    pub sounds: Vec<u16>,
    /// How many times artwork has been loaded or freed.
    ///
    /// Nothing in the engine counts this. It is here so a repaint can be
    /// held to its promise: painting a frame again must not reload a bitmap,
    /// free a slot, or put a new backdrop up. Comparing a counter catches a
    /// reload that happens to produce byte-identical artwork, which comparing
    /// the artwork would not.
    pub loads: u64,
    /// Where the island is standing, added to every drawing opcode.
    ///
    /// `seg12:0x05B9` shifts an opcode's first coordinate pair by
    /// `ds:0x46E4`/`ds:0x46E6` before the handler sees it, but only for the
    /// `0xA000` family and `0x4004`/`0x4204`, and only when the offset is not
    /// zero. Two of those -- `0x4004` and `0xA0A4`, the ones whose second pair
    /// is another point rather than a size -- get both pairs shifted
    /// (`seg12:0x0608`, off the table at `cs:0x1379`).
    ///
    /// `seg3:0x00FC` sets it: the island's own offset plus one, or nothing at
    /// all for a scene that has taken the screen over.
    pub draw_offset: (i32, i32),
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
            sounds: Vec::new(),
            loads: 0,
            draw_offset: (0, 0),
        }
    }
}

impl Stage {
    pub fn new() -> Self {
        Self::default()
    }

    /// A drawing opcode's coordinates, once the island has moved.
    fn at(&self, x: i16, y: i16) -> (i32, i32) {
        (x as i32 + self.draw_offset.0, y as i32 + self.draw_offset.1)
    }

    pub fn palette(&self) -> &[[u8; 3]; 256] {
        &self.palette
    }

    pub fn set_palette(&mut self, palette: [[u8; 3]; 256]) {
        self.palette = palette;
        // The original hands its DIB a palette, so a change recolours
        // everything already on screen. The colour buffer has to follow.
        self.surface.set_palette(&palette);
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
        // Borrow the surface to draw into the background, colours and all.
        // The background only grows a colour buffer if the sprite or the
        // background itself already had one; while neither does, it stays
        // palette art and a later `F05F` still recolours it.
        let tinted = self.surface.tinted;
        let saved = std::mem::replace(&mut self.surface.pixels, bg.pixels);
        let had = bg.colour.take();
        let saved_colour = std::mem::replace(
            &mut self.surface.colour,
            had.clone().unwrap_or_else(|| {
                self.surface
                    .pixels
                    .iter()
                    .map(|i| crate::render::pack(self.palette[*i as usize]))
                    .collect()
            }),
        );
        self.surface.tinted = had.is_some();
        let clip = self.surface.clip.take();
        self.surface
            .draw_sprite(sprite, x, y, crate::render::Mirror::none());
        bg.pixels = std::mem::replace(&mut self.surface.pixels, saved);
        let now = std::mem::replace(&mut self.surface.colour, saved_colour);
        bg.colour = self.surface.tinted.then_some(now);
        self.surface.tinted = tinted;
        self.surface.clip = clip;
        self.background = Some(bg);
        self.background_gen += 1;
    }

    /// Put one rectangle of the background back, rubbing out whatever has
    /// been drawn over it. `B606`'s whole job.
    pub fn restore_region(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let Some(bg) = self.background.take() else {
            return;
        };
        let sw = self.surface.width as i32;
        let sh = self.surface.height as i32;
        for row in y.max(0)..(y + h).min(sh).min(bg.height as i32) {
            let from = x.max(0);
            let to = (x + w).min(sw).min(bg.width as i32);
            if to <= from {
                continue;
            }
            let src = row as usize * bg.width as usize;
            let dst = row as usize * sw as usize;
            let (a, b) = (from as usize, to as usize);
            self.surface.pixels[dst + a..dst + b].copy_from_slice(&bg.pixels[src + a..src + b]);
            match &bg.colour {
                Some(c) => {
                    self.surface.colour[dst + a..dst + b].copy_from_slice(&c[src + a..src + b])
                }
                None => {
                    for k in a..b {
                        self.surface.colour[dst + k] =
                            crate::render::pack(self.palette[bg.pixels[src + k] as usize]);
                    }
                }
            }
        }
        self.background = Some(bg);
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

    /// Draw one recorded mark. The only way a held frame reaches the screen.
    pub fn paint(&mut self, p: &Paint) {
        let was = self.surface.clip;
        self.surface.clip = p.clip;
        match &p.mark {
            Mark::Sprite { art, x, y, mirror } => {
                self.surface.draw_sprite(art, *x, *y, *mirror)
            }
            Mark::Line { x1, y1, x2, y2, colour } => {
                self.surface.draw_line(*x1, *y1, *x2, *y2, *colour)
            }
            Mark::Rect { x, y, w, h, colour, filled } => {
                self.surface.draw_rect(*x, *y, *w, *h, *colour, *filled)
            }
            Mark::Ellipse { x, y, w, h, colour, filled } => {
                self.surface.draw_ellipse(*x, *y, *w, *h, *colour, *filled)
            }
            Mark::Restore { x, y, w, h } => {
                self.surface.clip = None;
                self.restore_region(*x, *y, *w, *h);
            }
        }
        self.surface.clip = was;
    }

    /// Everything a repaint is required to leave exactly as it found it.
    ///
    /// See `Untouched`.
    pub fn untouched(&self) -> Untouched {
        Untouched {
            background_gen: self.background_gen,
            loads: self.loads,
            last_screen: self.last_screen.clone(),
            palette: self.palette,
            sounds: self.sounds.len(),
            rng: self.rng.clone(),
            fg: self.fg,
            bg: self.bg,
            clip: self.surface.clip,
            draw_offset: self.draw_offset,
        }
    }
}

/// Everything a repaint is required to leave exactly as it found it.
///
/// The port paints the whole picture again every tick, which is only the same
/// thing the engine does if painting a frame a second time changes nothing
/// but the pixels it draws. Four separate bugs came from an opcode that
/// quietly broke that -- a bitmap reloaded, a whole backdrop replaced, a
/// sound made twice, a clip left behind -- so the invariant is checked rather
/// than assumed. `director::check_repaint` does the checking; `jcplay` and
/// `jcisland` turn it on with `JC_CHECK=1`.
#[derive(Clone, PartialEq, Eq)]
pub struct Untouched {
    background_gen: u64,
    loads: u64,
    last_screen: Option<String>,
    palette: [[u8; 3]; 256],
    sounds: usize,
    rng: crate::ads::Rng,
    fg: u8,
    bg: u8,
    clip: Option<Clip>,
    draw_offset: (i32, i32),
}

impl Untouched {
    /// Name whatever moved between two of these, for a report.
    pub fn differences(&self, then: &Untouched) -> Vec<String> {
        let mut out = Vec::new();
        let mut note = |ok: bool, what: &str| {
            if !ok {
                out.push(what.to_string());
            }
        };
        note(self.background_gen == then.background_gen, "put a new backdrop up");
        note(self.loads == then.loads, "loaded or freed artwork");
        note(self.last_screen == then.last_screen, "changed the screen on show");
        note(self.palette == then.palette, "changed the palette");
        note(self.sounds == then.sounds, "made a noise");
        note(self.rng == then.rng, "drew from the generator");
        note(self.fg == then.fg && self.bg == then.bg, "changed the drawing colours");
        note(self.clip == then.clip, "changed the clip");
        note(self.draw_offset == then.draw_offset, "moved the island");
        out
    }
}

/// A decoded `.TTM` program. Shared by every instance of the same file.
pub struct Program {
    pub ops: Vec<Op>,
    /// The scenes that get an instance record when this file loads, in the
    /// order their `1111` markers appear in it.
    ///
    /// `seg11:0x0645` makes one record per `1111` and appends it to a single
    /// list shared by every loaded file, so that order **is** the draw order
    /// (`seg10:0x1C35` walks it head to tail and the tail lands on top).
    /// Records are never removed while the file is loaded and `seg10:0x0DB6`
    /// re-arms the one it finds, so replaying a scene does not move it: only
    /// `4000` and `4010` do.
    pub scenes: Vec<u16>,
}

impl Program {
    pub fn load(archive: &Archive, name: &str) -> Option<Program> {
        let body = archive.get(name)?;
        let tt3 = dgds::leaf(body, b"TT3:")?;
        let code = dgds::decompress(tt3.payload).ok()?;
        let (ops, _) = decode(&code);
        let scenes = ops
            .iter()
            .filter_map(|op| match op {
                Op::SceneMarker { scene, playable: true } => Some(*scene),
                _ => None,
            })
            .collect();
        Some(Program { ops, scenes })
    }

    /// A program with nothing in it, for an instance of a scene whose TTM is
    /// not in the archive. `seg11:0x074A` gives the same answer -- no TTM
    /// loaded, so the scene cannot be run -- and `seg10:0x1DBA` acts on it.
    pub fn empty() -> Program {
        Program {
            ops: Vec::new(),
            scenes: Vec::new(),
        }
    }

    /// The instruction index just after a scene marker.
    pub fn scene_start(&self, scene: u16) -> Option<usize> {
        self.ops
            .iter()
            .position(|op| matches!(op, Op::SceneMarker { scene: n, .. } if *n == scene))
            .map(|i| i + 1)
    }
}

/// Will the region family act on this rectangle at all?
///
/// `seg12:0x0318` walks off to its "done" label for a width or height that is
/// not positive, or a rectangle whose far edge is past the screen. There is
/// no clipping to fit: the op simply does not happen.
fn fits(stage: &Stage, x: i32, y: i32, w: i16, h: i16) -> bool {
    w > 0
        && h > 0
        && x + w as i32 <= stage.surface.width as i32
        && y + h as i32 <= stage.surface.height as i32
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
    /// The drawing colours, which belong to the instance and not to the
    /// screen.
    ///
    /// `seg12:0x06A9` writes `2002`'s two operands into the instance record
    /// at +0x1E and +0x1F as well as into the globals `ds:0x1EAE` and
    /// `ds:0x1EAD`, and `seg10:0x1C81` copies them back out of the record
    /// before every frame that instance runs -- the same arrangement as the
    /// clip below. `seg10:0x0B6E` fills both in with 15 when the record is
    /// made, so a script that draws a line before it has picked a colour
    /// draws it white, not in colour 0.
    fg: u8,
    bg: u8,
    /// The clip rectangle, which belongs to the instance and not to the
    /// screen.
    ///
    /// `seg12:0x0F40` writes `4004`'s rectangle into the instance record at
    /// +0x21 as well as into the globals, and `seg10:0x1C90` copies it back
    /// out of the record into those globals before every frame that instance
    /// runs. So two scenes on screen at once each draw under their own clip,
    /// however they interleave, and a clip one scene set is not inherited by
    /// the next. `seg10:0x1E2F` then puts the globals back to the whole
    /// screen once the list has been walked, so nothing drawn outside the
    /// runner -- Johnny's walk sprite, the island -- is ever clipped.
    ///
    /// `None` is the whole screen, which is what `seg10:0x0BA6` fills a new
    /// record's rectangle in with.
    clip: Option<Clip>,
    /// What the frame on screen drew, so it can be painted again.
    ///
    /// The engine leaves what a frame drew on screen until something paints
    /// over it: scripts save and restore the rectangles they disturb
    /// (`4204` and `A064`) rather than the whole picture. This port repaints
    /// the backdrop every tick, so it has to put the frames back on top of it
    /// every tick too, or a sprite would only be on screen for the one tick
    /// in seven its own script advanced on. See `Paint` for why this keeps
    /// the drawings rather than replaying the instructions.
    painted: Vec<Paint>,
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
            fg: 15,
            bg: 15,
            clip: None,
            painted: Vec::new(),
        }
    }

    /// Draw something, and keep it so the frame can be painted again.
    fn mark(&mut self, stage: &mut Stage, mark: Mark) {
        let p = Paint {
            clip: stage.surface.clip,
            mark,
        };
        stage.paint(&p);
        self.painted.push(p);
    }

    /// Put the frame that is already on screen back, without advancing.
    ///
    /// Nothing but drawing happens here: the frame's marks were resolved when
    /// it ran, so this cannot load a bitmap, make a noise, draw from the
    /// generator or move the instance on. It does not even need the archive.
    pub fn repaint(&mut self, stage: &mut Stage) {
        for p in &self.painted {
            stage.paint(p);
        }
    }

    /// What the frame on screen is made of, for the tools.
    pub fn marks(&self) -> usize {
        self.painted.len()
    }

    /// Where the instance stands: everything a repaint saves and puts back.
    ///
    /// Compared before and after a repaint, this catches the other half of
    /// the invariant -- a frame painted again must not advance its own
    /// program, change its delay, or leave a different slot selected.
    pub fn cursor(&self) -> (usize, usize, u16, usize, usize, Option<u16>, Option<u16>, bool) {
        (
            self.pc,
            self.painted.len(),
            self.delay,
            self.bitmap_slot,
            self.palette_slot,
            self.pending_goto,
            self.last_goto,
            self.ended,
        )
    }

    /// Leave the program pointer past the end, so the next frame asked for
    /// is `Finished`.
    ///
    /// This is the state `seg11:0x074A` reports by returning zero: the scene
    /// index is outside the TTM's scene table, or its entry in that table is
    /// null. The engine still makes the instance record -- `seg10:0x0DB6`
    /// never checks whether the scene exists -- and the runner puts it into
    /// state 4 on its first tick, so a block waiting on that scene to end
    /// fires instead of waiting for ever.
    pub fn exhaust(&mut self) {
        self.pc = self.program.ops.len();
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
        self.painted.clear();
        // `seg10:0x1C81`: the instance's own colours and clip, not whatever
        // the last scene to draw happened to leave behind.
        stage.fg = self.fg;
        stage.bg = self.bg;
        stage.surface.clip = self.clip;
        self.run_ops(stage, archive)
    }

    fn run_ops(&mut self, stage: &mut Stage, archive: &Archive) -> Step {
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
                    stage.loads += 1;
                    if let Some(f) = archive.screen(&name) {
                        stage.set_background(f);
                        stage.last_screen = Some(name);
                    }
                }
                Op::LoadBitmap(name) => {
                    stage.loads += 1;
                    if let Some(frames) = archive.bitmap(&name) {
                        // Shared, so a frame that drew from this slot keeps
                        // its artwork when the slot is loaded over.
                        stage.slots[self.bitmap_slot] =
                            frames.into_iter().map(Rc::new).collect();
                    }
                }
                Op::LoadPalette(name) => {
                    stage.loads += 1;
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
                    let (x, y) = stage.at(x, y);
                    self.mark(stage, Mark::Line { x1: x, y1: y, x2: x, y2: y, colour: stage.fg });
                }
                Op::SetColors { fg, bg } => {
                    // `seg12:0x06A9` writes both: the instance keeps them,
                    // and the screen uses them until the next frame puts its
                    // own back.
                    self.fg = fg;
                    self.bg = bg;
                    stage.fg = fg;
                    stage.bg = bg;
                }
                Op::SetClip { x1, y1, x2, y2 } => {
                    let (x1, y1) = stage.at(x1, y1);
                    let (x2, y2) = stage.at(x2, y2);
                    // `seg12:0x0F40` writes both: the instance keeps it, and
                    // the screen uses it until the next frame restores its
                    // own.
                    self.clip = Some(Clip { x1, y1, x2, y2 });
                    stage.surface.clip = self.clip;
                }
                Op::DrawLine { x1, y1, x2, y2 } => {
                    let (x1, y1) = stage.at(x1, y1);
                    let (x2, y2) = stage.at(x2, y2);
                    self.mark(stage, Mark::Line { x1, y1, x2, y2, colour: stage.fg });
                }
                Op::DrawRect { x, y, w, h, filled } => {
                    let (x, y) = stage.at(x, y);
                    let mark = Mark::Rect {
                        x,
                        y,
                        w: w as i32,
                        h: h as i32,
                        colour: stage.fg,
                        filled,
                    };
                    self.mark(stage, mark);
                }
                Op::DrawEllipse { x, y, w, h, filled } => {
                    let (x, y) = stage.at(x, y);
                    let mark = Mark::Ellipse {
                        x,
                        y,
                        w: w as i32,
                        h: h as i32,
                        colour: stage.fg,
                        filled,
                    };
                    self.mark(stage, mark);
                }
                Op::FreeShapes => {
                    stage.loads += 1;
                    stage.slots[self.bitmap_slot].clear();
                }
                Op::DrawSprite {
                    x,
                    y,
                    frame,
                    slot,
                    mirror,
                } => {
                    let slot = (slot as usize).min(15);
                    let (x, y) = stage.at(x, y);
                    if let Some(art) = stage.slots[slot].get(frame as usize).cloned() {
                        self.mark(stage, Mark::Sprite { art, x, y, mirror });
                    }
                }
                Op::GotoFrame(n) => self.pending_goto = Some(n),
                Op::EndScene => self.ended = true,
                // Markers, no-ops and the not-yet-implemented, all explicit.
                // `seg5:0x00BA`: one noise at a time, since `sndPlaySound`
                // replaces whatever is already going.
                Op::PlaySound(id) => {
                    stage.sounds.push(id);
                    if stage.sounds.len() > 8 {
                        stage.sounds.remove(0);
                    }
                }
                Op::SceneMarker { .. } | Op::SetBackground(_) | Op::NoOp(_) => {}
                // Known, named, and not yet drawn. Counted rather than
                // silently skipped so they stay visible.
                // The record half. Nothing to do: this port repaints the
                // whole background every tick, so what it saves is never lost.
                Op::RegionOp { opcode: 0xA054, .. } => {}
                // The restore half. Put that rectangle of the backdrop back,
                // the same as `B606`.
                Op::RegionOp {
                    opcode: 0xA064,
                    x,
                    y,
                    w,
                    h,
                } => {
                    let (x, y) = stage.at(x, y);
                    // `seg12:0x0318`: the whole family refuses a rectangle
                    // that is empty or that runs off the screen -- it is not
                    // clipped to fit, it is not done at all. The island's
                    // offset is already in `x` and `y` and is what can push a
                    // rectangle off the right-hand edge, so this has to be
                    // asked after the shift, not before it.
                    if fits(stage, x, y, w, h) {
                        let mark = Mark::Restore { x, y, w: w as i32, h: h as i32 };
                        self.mark(stage, mark);
                    }
                }
                // The other six have no use in any shipped script. Counted, so
                // that stays true rather than being assumed.
                Op::RegionOp { opcode, .. } => {
                    *stage.todo_seen.entry(opcode).or_insert(0) += 1;
                }
                Op::CopyRegion {
                    x,
                    y,
                    w,
                    h,
                    from,
                    to,
                } => {
                    if from == 2 && to == 1 {
                        let mark = Mark::Restore {
                            x: x as i32,
                            y: y as i32,
                            w: w as i32,
                            h: h as i32,
                        };
                        self.mark(stage, mark);
                    } else {
                        // A copy between two pages the port does not keep
                        // apart. No script does this; count it if one ever
                        // does rather than quietly doing the wrong thing.
                        *stage.todo_seen.entry(0xB606).or_insert(0) += 1;
                    }
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
            colour: self.colour.clone(),
        }
    }
}
