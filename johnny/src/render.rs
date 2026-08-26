//! An 8-bit indexed surface and the sprite blit.
//!
//! The original ran at 640x350 in 16 colours with palette index 0 as the
//! transparency key. Everything here stays indexed; conversion to RGBA happens
//! once, at present time, so scaling and filtering are a display concern
//! rather than something baked into the engine.

use crate::dgds::Frame;

pub const SCREEN_W: u16 = 640;
pub const SCREEN_H: u16 = 350;

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

/// An indexed surface.
#[derive(Clone)]
pub struct Surface {
    pub width: u16,
    pub height: u16,
    pub pixels: Vec<u8>,
}

impl Surface {
    pub fn new(width: u16, height: u16) -> Self {
        Surface {
            width,
            height,
            pixels: vec![0; width as usize * height as usize],
        }
    }

    pub fn screen() -> Self {
        Surface::new(SCREEN_W, SCREEN_H)
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

    /// Draw a sprite with index 0 masked out, clipped to the surface.
    pub fn draw_sprite(&mut self, frame: &Frame, x: i32, y: i32, mirror: Mirror) {
        let (fw, fh) = (frame.width as i32, frame.height as i32);
        for row in 0..fh {
            let dy = y + row;
            if dy < 0 || dy >= self.height as i32 {
                continue;
            }
            let src_row = if mirror.y { fh - 1 - row } else { row } as usize;
            for col in 0..fw {
                let dx = x + col;
                if dx < 0 || dx >= self.width as i32 {
                    continue;
                }
                let src_col = if mirror.x { fw - 1 - col } else { col } as usize;
                let value = frame.pixels[src_row * frame.width as usize + src_col];
                if value != TRANSPARENT {
                    self.pixels[dy as usize * self.width as usize + dx as usize] = value;
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
