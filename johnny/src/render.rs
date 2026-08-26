//! An 8-bit indexed surface and the sprite blit.
//!
//! The original ran at 640x350 in 16 colours with palette index 0 as the
//! transparency key. Everything here stays indexed; conversion to RGBA happens
//! once, at present time, so scaling and filtering are a display concern
//! rather than something baked into the engine.

use crate::dgds::Frame;

pub const SCREEN_W: u16 = 640;
/// 480, not the 350 the scene backdrops are drawn at. `seg2:0x1483` writes
/// 640 and 480 into `ds:0x2562`/`ds:0x2564` and `seg3:0x01D1` clears a
/// rectangle that size; the sea is 640x480 and the surf round the island
/// breaks as far down as row 384, so a 350-row screen would cut it off.
pub const SCREEN_H: u16 = 480;

/// The palette swaps black and magenta so that index 0 is free to be the
/// transparency key; black ends up at index 5.
pub const BLACK: u8 = 5;

/// Palette index 0 is transparent in every sprite. The palette puts magenta
/// there and moves black to index 5, which is what frees index 0 up.
pub const TRANSPARENT: u8 = 0;

/// How a sprite is mirrored as it is drawn.
///
/// From the blitter at `seg8:0x0AE4`: the draw mode is `(opcode & 0x00F0) >> 4`
/// and is a bitmask. Bit 0 negates the horizontal step, bit 1 the vertical.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct Mirror {
    pub x: bool,
    pub y: bool,
}

impl Mirror {
    pub fn from_mode(mode: u16) -> Self {
        Mirror {
            x: mode & 1 != 0,
            y: mode & 2 != 0,
        }
    }

    pub fn none() -> Self {
        Mirror::default()
    }
}

/// An inclusive clip rectangle in screen coordinates.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Clip {
    pub x1: i32,
    pub y1: i32,
    pub x2: i32,
    pub y2: i32,
}

/// An indexed surface.
#[derive(Clone)]
pub struct Surface {
    pub width: u16,
    pub height: u16,
    pub pixels: Vec<u8>,
    /// Set by TTM opcode `4004`, which stores the x pair and the y pair
    /// separately (`seg12:0x0F40`).
    pub clip: Option<Clip>,
}

impl Surface {
    pub fn new(width: u16, height: u16) -> Self {
        Surface {
            width,
            height,
            pixels: vec![0; width as usize * height as usize],
            clip: None,
        }
    }

    /// A screen-sized surface, cleared to black rather than to index 0.
    ///
    /// Index 0 is the transparency key, and the palette puts magenta there --
    /// fine as a key, alarming as a colour. Anything the backdrop does not
    /// reach (a scene loading one of the 640x350 screens, say) should be black.
    pub fn screen() -> Self {
        let mut s = Surface::new(SCREEN_W, SCREEN_H);
        s.fill(BLACK);
        s
    }

    pub fn fill(&mut self, index: u8) {
        self.pixels.fill(index);
    }

    /// Copy a full-screen background in, ignoring transparency.
    pub fn draw_background(&mut self, bg: &Frame) {
        let w = bg.width.min(self.width) as usize;
        let h = bg.height.min(self.height) as usize;
        for y in 0..h {
            let src = y * bg.width as usize;
            let dst = y * self.width as usize;
            self.pixels[dst..dst + w].copy_from_slice(&bg.pixels[src..src + w]);
        }
    }

    /// Is this pixel inside the surface and the current clip rectangle?
    #[inline]
    fn visible(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return false;
        }
        match self.clip {
            Some(c) => x >= c.x1 && x <= c.x2 && y >= c.y1 && y <= c.y2,
            None => true,
        }
    }

    #[inline]
    fn put(&mut self, x: i32, y: i32, index: u8) {
        if self.visible(x, y) {
            self.pixels[y as usize * self.width as usize + x as usize] = index;
        }
    }

    /// Draw a line. TTM `A0A4` passes the two endpoints.
    pub fn draw_line(&mut self, x1: i32, y1: i32, x2: i32, y2: i32, index: u8) {
        let (dx, dy) = ((x2 - x1).abs(), -(y2 - y1).abs());
        let (sx, sy) = (if x1 < x2 { 1 } else { -1 }, if y1 < y2 { 1 } else { -1 });
        let (mut x, mut y) = (x1, y1);
        let mut err = dx + dy;
        loop {
            self.put(x, y, index);
            if x == x2 && y == y2 {
                break;
            }
            let e2 = err * 2;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Draw a rectangle. TTM `A104` fills it, `A114` outlines it.
    pub fn draw_rect(&mut self, x: i32, y: i32, w: i32, h: i32, index: u8, filled: bool) {
        if w <= 0 || h <= 0 {
            return;
        }
        let (x2, y2) = (x + w - 1, y + h - 1);
        if filled {
            for row in y..=y2 {
                for col in x..=x2 {
                    self.put(col, row, index);
                }
            }
        } else {
            self.draw_line(x, y, x2, y, index);
            self.draw_line(x, y2, x2, y2, index);
            self.draw_line(x, y, x, y2, index);
            self.draw_line(x2, y, x2, y2, index);
        }
    }

    /// Draw an ellipse in a bounding box. TTM `A404` fills, `A424` outlines.
    pub fn draw_ellipse(&mut self, x: i32, y: i32, w: i32, h: i32, index: u8, filled: bool) {
        if w <= 0 || h <= 0 {
            return;
        }
        let (rx, ry) = (w / 2, h / 2);
        let (cx, cy) = (x + rx, y + ry);
        if rx == 0 || ry == 0 {
            self.draw_line(x, y, x + w - 1, y + h - 1, index);
            return;
        }
        for dy in -ry..=ry {
            // Half-width of the ellipse at this row.
            let t = 1.0 - (dy as f64 / ry as f64).powi(2);
            if t < 0.0 {
                continue;
            }
            let dx = (rx as f64 * t.sqrt()).round() as i32;
            if filled {
                for px in (cx - dx)..=(cx + dx) {
                    self.put(px, cy + dy, index);
                }
            } else {
                self.put(cx - dx, cy + dy, index);
                self.put(cx + dx, cy + dy, index);
            }
        }
    }

    /// Draw a sprite with index 0 masked out, clipped to the surface.
    pub fn draw_sprite(&mut self, frame: &Frame, x: i32, y: i32, mirror: Mirror) {
        let (fw, fh) = (frame.width as i32, frame.height as i32);
        for row in 0..fh {
            let dy = y + row;
            let src_row = if mirror.y { fh - 1 - row } else { row } as usize;
            for col in 0..fw {
                let dx = x + col;
                let src_col = if mirror.x { fw - 1 - col } else { col } as usize;
                let value = frame.pixels[src_row * frame.width as usize + src_col];
                if value != TRANSPARENT {
                    self.put(dx, dy, value);
                }
            }
        }
    }

    /// Expand to RGBA using a palette. This is the only place colour enters.
    pub fn to_rgba(&self, palette: &[[u8; 3]; 256]) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 4);
        for &i in &self.pixels {
            let c = palette[i as usize];
            out.extend_from_slice(&[c[0], c[1], c[2], 0xff]);
        }
        out
    }
}
