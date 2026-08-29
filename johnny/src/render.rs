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
/// The draw mode is `(opcode & 0x00F0) >> 4` and it is a bitmask, and the two
/// bits are the other way round from the obvious reading. In the blitter at
/// `seg8:0x0AE4`, `di` is the destination width -- `seg8:0x0C34` compares it
/// against twice the sprite's own width and `seg8:0x0C5B` against half of it
/// -- and `[bp-0x12]` is the destination height. Bit 0 negates the height
/// (`seg8:0x0CD1`) and bit 1 negates the width (`seg8:0x0CE9`). So **bit 0 is
/// the vertical flip and bit 1 the horizontal one**.
///
/// The walk settles it independently: `seg4:0x02C0` turns the `0x8000` flag on
/// a waypoint into mode **2**, and that is unmistakably a left-right mirror --
/// the same sprites carry Johnny both ways along the beach, and he does not
/// walk on his head.
///
/// Getting this backwards is quiet rather than loud. Only two of the four
/// modes are used in the shipped scripts: `A504` (no mirror, 12,546 draws) and
/// `A524` (2,822 draws across 31 of the 41 scripts). Reading bit 1 as vertical
/// draws every one of those 2,822 upside down.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct Mirror {
    pub x: bool,
    pub y: bool,
}

impl Mirror {
    pub fn from_mode(mode: u16) -> Self {
        Mirror {
            x: mode & 2 != 0,
            y: mode & 1 != 0,
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
    /// The picture as the engine draws it: one palette index per pixel.
    ///
    /// Everything in `RESOURCE.001` is sixteen colours -- all 2,392 sprite
    /// frames and all ten backdrops use indices 0 to 15 and nothing else --
    /// so this is the whole of the original's visual budget, and it stays
    /// authoritative: the repaint check, the PNG dumps and the digests all
    /// compare it.
    pub pixels: Vec<u8>,
    /// The same picture in colour, as `0x00RRGGBB`.
    ///
    /// Kept in step with `pixels` write for write. An indexed draw resolves
    /// through the palette; artwork that has its own colours -- which is the
    /// point of the whole buffer -- writes here and leaves its nearest
    /// palette index behind in `pixels`. Byte order in memory is B, G, R, 0,
    /// which is what a 32bpp `BI_RGB` DIB wants.
    pub colour: Vec<u32>,
    /// Set by TTM opcode `4004`, which stores the x pair and the y pair
    /// separately (`seg12:0x0F40`).
    pub clip: Option<Clip>,
    /// The palette every indexed write resolves through.
    lut: [u32; 256],
    /// Whether anything on this surface came from artwork with colours of its
    /// own. While nothing has, the colour buffer is just the palette applied
    /// to `pixels` and can be thrown away and rebuilt -- which is what keeps
    /// a palette change recolouring the island, the way the original's DIB
    /// does. Once something has, the colours have to be carried.
    pub tinted: bool,
}

/// Pack a palette entry the way the colour buffer holds it.
#[inline]
pub fn pack(rgb: [u8; 3]) -> u32 {
    ((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[2] as u32
}

impl Surface {
    pub fn new(width: u16, height: u16) -> Self {
        Surface {
            width,
            height,
            pixels: vec![0; width as usize * height as usize],
            colour: vec![0; width as usize * height as usize],
            clip: None,
            lut: [0; 256],
            tinted: false,
        }
    }

    /// Take a new palette on, and recolour what is already drawn.
    ///
    /// The original hands its 8bpp DIB a palette, so changing it recolours
    /// the whole screen at once. Forty-one `F05F` loads in the scripts, and
    /// this is what makes them behave the same way here.
    pub fn set_palette(&mut self, palette: &[[u8; 3]; 256]) {
        for (slot, rgb) in self.lut.iter_mut().zip(palette.iter()) {
            *slot = pack(*rgb);
        }
        for (c, i) in self.colour.iter_mut().zip(self.pixels.iter()) {
            *c = self.lut[*i as usize];
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
        self.colour.fill(self.lut[index as usize]);
    }

    /// Copy a full-screen background in, ignoring transparency.
    pub fn draw_background(&mut self, bg: &Frame) {
        let w = bg.width.min(self.width) as usize;
        let h = bg.height.min(self.height) as usize;
        for y in 0..h {
            let src = y * bg.width as usize;
            let dst = y * self.width as usize;
            self.pixels[dst..dst + w].copy_from_slice(&bg.pixels[src..src + w]);
            match &bg.colour {
                Some(c) => {
                    self.tinted = true;
                    self.colour[dst..dst + w].copy_from_slice(&c[src..src + w]);
                }
                None => {
                    for k in 0..w {
                        self.colour[dst + k] = self.lut[bg.pixels[src + k] as usize];
                    }
                }
            }
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
            let at = y as usize * self.width as usize + x as usize;
            self.pixels[at] = index;
            self.colour[at] = self.lut[index as usize];
        }
    }

    /// Put down a colour that did not come from the palette, keeping the
    /// nearest index for the indexed buffer so everything built on that --
    /// the repaint check, the frame dumps -- still sees a picture.
    fn put_colour(&mut self, x: i32, y: i32, rgb: u32, index: u8) {
        if self.visible(x, y) {
            // Only once something is actually down. `tinted` is what decides
            // whether a composed frame carries its own colours, and a frame
            // that does is no longer recoloured by a later palette change --
            // so a sprite clipped entirely away must not cost the surface
            // its palette art.
            self.tinted = true;
            let at = y as usize * self.width as usize + x as usize;
            self.pixels[at] = index;
            self.colour[at] = rgb;
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
    /// A circle, centre and radius, the way the engine asks for one.
    ///
    /// `seg12:0x01C9` turns the opcode's four operands into three: a radius
    /// of `h >> 1`, and a centre at `(x + r, y + r)`. The width operand is
    /// read by nobody -- all 437 in `RESOURCE.001` are square, so this has
    /// never shown, but it is the rule. `seg9:0x0688` then hands GDI
    /// `Ellipse(cx - r, cy - r, cx + r, cy + r)`, whose right and bottom
    /// edges are excluded, so what lands on screen is the 2r-by-2r box
    /// `[cx - r, cx + r - 1]`.
    pub fn draw_circle(&mut self, cx: i32, cy: i32, r: i32, index: u8, filled: bool) {
        if r <= 0 {
            return;
        }
        // The box is even-sided, so the true centre falls between two
        // pixels: half a pixel left of `cx` and half a pixel above `cy`.
        let half = r as f64 - 0.5;
        for row in 0..2 * r {
            let dy = (cy - r + row) as f64 - (cy as f64 - 0.5);
            let t = 1.0 - (dy / half).powi(2);
            if t < 0.0 {
                continue;
            }
            let dx = (half * t.sqrt()).round() as i32;
            let (a, b) = (cx - dx, cx + dx - 1);
            let y = cy - r + row;
            if filled {
                for px in a..=b {
                    self.put(px, y, index);
                }
            } else {
                self.put(a, y, index);
                self.put(b, y, index);
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
                let at = src_row * frame.width as usize + src_col;
                let value = frame.pixels[at];
                match &frame.colour {
                    // Artwork with colours of its own. Its alpha decides what
                    // shows rather than index 0 -- redrawn art has no reason
                    // to keep magenta as a key.
                    Some(c) => {
                        let argb = c[at];
                        if argb >> 24 != 0 {
                            self.put_colour(dx, dy, argb & 0x00FF_FFFF, value);
                        }
                    }
                    None if value != TRANSPARENT => self.put(dx, dy, value),
                    None => {}
                }
            }
        }
    }

}
