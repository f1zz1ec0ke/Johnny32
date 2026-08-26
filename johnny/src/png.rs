//! A minimal PNG writer, so the engine can dump frames without a dependency.
//!
//! Uses stored (uncompressed) deflate blocks. The files are larger than a real
//! encoder would produce, which does not matter for debug output and keeps the
//! crate dependency-free.

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, entry) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *entry = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// zlib stream made of stored deflate blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut chunks = data.chunks(0xFFFF).peekable();
    if data.is_empty() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xff, 0xff]);
    }
    while let Some(chunk) = chunks.next() {
        let last = chunks.peek().is_none();
        out.push(if last { 1 } else { 0 });
        out.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
        out.extend_from_slice(&(!(chunk.len() as u16)).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(tag);
    out.extend_from_slice(payload);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Write an 8-bit palettised PNG. `transparent` marks one index fully clear.
pub fn write_indexed(
    path: &std::path::Path,
    width: u16,
    height: u16,
    pixels: &[u8],
    palette: &[[u8; 3]; 256],
    transparent: Option<u8>,
) -> std::io::Result<()> {
    let mut raw = Vec::with_capacity(pixels.len() + height as usize);
    for y in 0..height as usize {
        raw.push(0); // filter: none
        let row = y * width as usize;
        raw.extend_from_slice(&pixels[row..row + width as usize]);
    }

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 3, 0, 0, 0]); // 8-bit, palette
    chunk(&mut png, b"IHDR", &ihdr);

    let plte: Vec<u8> = palette.iter().flatten().copied().collect();
    chunk(&mut png, b"PLTE", &plte);

    if let Some(t) = transparent {
        let mut trns = vec![0xffu8; 256];
        trns[t as usize] = 0;
        chunk(&mut png, b"tRNS", &trns);
    }

    chunk(&mut png, b"IDAT", &zlib_stored(&raw));
    chunk(&mut png, b"IEND", &[]);
    std::fs::write(path, png)
}

/// Write a 24-bit PNG from a colour buffer of `0x00RRGGBB`, for dumping what
/// the screen actually shows once artwork with its own colours is in play.
pub fn write_rgb(
    path: &std::path::Path,
    width: u16,
    height: u16,
    colour: &[u32],
) -> std::io::Result<()> {
    let (w, h) = (width as usize, height as usize);
    let mut raw = Vec::with_capacity(h * (1 + w * 3));
    for y in 0..h {
        raw.push(0);
        for x in 0..w {
            let c = colour.get(y * w + x).copied().unwrap_or(0);
            raw.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8]);
        }
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    chunk(&mut out, b"IEND", &[]);
    std::fs::write(path, out)
}

// ---- reading -------------------------------------------------------------

/// A decoded image: straight RGBA, eight bits a channel.
pub struct Image {
    pub width: u16,
    pub height: u16,
    /// `0xAARRGGBB`, one per pixel, row major.
    pub pixels: Vec<u32>,
}

/// Read a PNG. Enough of the format for what an art tool exports: eight-bit
/// greyscale, palettised (with `tRNS`), RGB and RGBA, all five filters, no
/// interlacing.
///
/// Written out rather than pulled in because the whole point of this crate is
/// that it has no dependencies -- and a decoder that can read what the
/// exporter writes is a round trip worth having.
pub fn read(data: &[u8]) -> Result<Image, String> {
    if data.len() < 8 || &data[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("not a PNG".into());
    }
    let (mut w, mut h, mut depth, mut kind, mut interlace) = (0u32, 0u32, 0u8, 0u8, 0u8);
    let mut idat: Vec<u8> = Vec::new();
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut alpha: Vec<u8> = Vec::new();
    let mut p = 8usize;
    while p + 8 <= data.len() {
        let len = u32::from_be_bytes(data[p..p + 4].try_into().unwrap()) as usize;
        let tag = &data[p + 4..p + 8];
        let body = data
            .get(p + 8..p + 8 + len)
            .ok_or_else(|| "truncated chunk".to_string())?;
        match tag {
            b"IHDR" => {
                if body.len() < 13 {
                    return Err("short IHDR".into());
                }
                w = u32::from_be_bytes(body[0..4].try_into().unwrap());
                h = u32::from_be_bytes(body[4..8].try_into().unwrap());
                depth = body[8];
                kind = body[9];
                interlace = body[12];
            }
            b"PLTE" => palette = body.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
            b"tRNS" => alpha = body.to_vec(),
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        p += 12 + len;
    }
    if depth != 8 {
        return Err(format!("{depth}-bit samples; this reader wants 8"));
    }
    if interlace != 0 {
        return Err("interlaced".into());
    }
    if w == 0 || h == 0 || w > 8192 || h > 8192 {
        return Err(format!("{w}x{h} is not a size"));
    }
    let channels = match kind {
        0 => 1usize,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        other => return Err(format!("colour type {other}")),
    };
    let raw = inflate(&idat)?;
    let stride = w as usize * channels;
    let mut rows = vec![0u8; stride * h as usize];
    let mut at = 0usize;
    for y in 0..h as usize {
        let filter = *raw.get(at).ok_or_else(|| "truncated image".to_string())?;
        at += 1;
        let line = raw
            .get(at..at + stride)
            .ok_or_else(|| "truncated image".to_string())?;
        at += stride;
        let (before, here) = rows.split_at_mut(y * stride);
        let prev = if y == 0 { None } else { Some(&before[(y - 1) * stride..]) };
        unfilter(filter, line, &mut here[..stride], prev, channels)?;
    }
    let mut pixels = Vec::with_capacity(w as usize * h as usize);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let s = &rows[y * stride + x * channels..];
            let (r, g, b, a) = match kind {
                0 => (s[0], s[0], s[0], 0xFF),
                2 => (s[0], s[1], s[2], 0xFF),
                3 => {
                    let i = s[0] as usize;
                    let c = palette.get(i).copied().unwrap_or([0, 0, 0]);
                    (c[0], c[1], c[2], alpha.get(i).copied().unwrap_or(0xFF))
                }
                4 => (s[0], s[0], s[0], s[1]),
                _ => (s[0], s[1], s[2], s[3]),
            };
            pixels.push(
                ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32,
            );
        }
    }
    Ok(Image {
        width: w as u16,
        height: h as u16,
        pixels,
    })
}

fn unfilter(
    filter: u8,
    line: &[u8],
    out: &mut [u8],
    prev: Option<&[u8]>,
    bpp: usize,
) -> Result<(), String> {
    for i in 0..line.len() {
        let a = if i >= bpp { out[i - bpp] as i32 } else { 0 };
        let b = prev.map_or(0, |p| p[i] as i32);
        let c = match prev {
            Some(p) if i >= bpp => p[i - bpp] as i32,
            _ => 0,
        };
        let add = match filter {
            0 => 0,
            1 => a,
            2 => b,
            3 => (a + b) / 2,
            4 => {
                let q = a + b - c;
                let (pa, pb, pc) = ((q - a).abs(), (q - b).abs(), (q - c).abs());
                if pa <= pb && pa <= pc {
                    a
                } else if pb <= pc {
                    b
                } else {
                    c
                }
            }
            other => return Err(format!("filter {other}")),
        };
        out[i] = (line[i] as i32 + add) as u8;
    }
    Ok(())
}

/// A zlib stream, inflated. Stored, fixed-Huffman and dynamic-Huffman blocks.
fn inflate(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() < 2 {
        return Err("empty stream".into());
    }
    let mut bits = Bits {
        data: &data[2..],
        at: 0,
    };
    let mut out = Vec::new();
    loop {
        let last = bits.take(1)?;
        match bits.take(2)? {
            0 => {
                bits.align();
                let len = bits.take(16)? as usize;
                let _nlen = bits.take(16)?;
                for _ in 0..len {
                    out.push(bits.take(8)? as u8);
                }
            }
            1 => {
                let (lit, dist) = fixed_tables();
                block(&mut bits, &lit, &dist, &mut out)?;
            }
            2 => {
                let (lit, dist) = dynamic_tables(&mut bits)?;
                block(&mut bits, &lit, &dist, &mut out)?;
            }
            _ => return Err("reserved block type".into()),
        }
        if last == 1 {
            return Ok(out);
        }
    }
}

struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn take(&mut self, n: u32) -> Result<u32, String> {
        let mut v = 0u32;
        for k in 0..n {
            let byte = *self
                .data
                .get(self.at >> 3)
                .ok_or_else(|| "ran off the end of the stream".to_string())?;
            v |= (((byte >> (self.at & 7)) & 1) as u32) << k;
            self.at += 1;
        }
        Ok(v)
    }

    fn align(&mut self) {
        self.at = (self.at + 7) & !7;
    }
}

/// A canonical Huffman table: for each code length, the first code and the
/// symbols that use it. Decoding walks bit by bit, which is slow and short.
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

fn build(lengths: &[u8]) -> Huffman {
    let mut counts = [0u16; 16];
    for &l in lengths {
        counts[l as usize] += 1;
    }
    counts[0] = 0;
    let mut offsets = [0u16; 16];
    for i in 1..16 {
        offsets[i] = offsets[i - 1] + counts[i - 1];
    }
    let mut symbols = vec![0u16; lengths.len()];
    for (sym, &l) in lengths.iter().enumerate() {
        if l != 0 {
            symbols[offsets[l as usize] as usize] = sym as u16;
            offsets[l as usize] += 1;
        }
    }
    Huffman { counts, symbols }
}

fn decode(bits: &mut Bits, h: &Huffman) -> Result<u16, String> {
    let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
    for len in 1..16 {
        code |= bits.take(1)? as i32;
        let count = h.counts[len] as i32;
        if code - first < count {
            return Ok(h.symbols[(index + (code - first)) as usize]);
        }
        index += count;
        first = (first + count) << 1;
        code <<= 1;
    }
    Err("bad Huffman code".into())
}

fn fixed_tables() -> (Huffman, Huffman) {
    let mut lit = vec![8u8; 288];
    lit[144..256].fill(9);
    lit[256..280].fill(7);
    (build(&lit), build(&[5u8; 30]))
}

fn dynamic_tables(bits: &mut Bits) -> Result<(Huffman, Huffman), String> {
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let hlit = bits.take(5)? as usize + 257;
    let hdist = bits.take(5)? as usize + 1;
    let hclen = bits.take(4)? as usize + 4;
    let mut code_lengths = [0u8; 19];
    for &slot in ORDER.iter().take(hclen) {
        code_lengths[slot] = bits.take(3)? as u8;
    }
    let codes = build(&code_lengths);
    let mut lengths = vec![0u8; hlit + hdist];
    let mut i = 0usize;
    while i < lengths.len() {
        let sym = decode(bits, &codes)?;
        match sym {
            0..=15 => {
                lengths[i] = sym as u8;
                i += 1;
            }
            16 => {
                let prev = *lengths.get(i.wrapping_sub(1)).unwrap_or(&0);
                for _ in 0..3 + bits.take(2)? {
                    if i < lengths.len() {
                        lengths[i] = prev;
                        i += 1;
                    }
                }
            }
            17 => i += 3 + bits.take(3)? as usize,
            _ => i += 11 + bits.take(7)? as usize,
        }
    }
    Ok((build(&lengths[..hlit]), build(&lengths[hlit..])))
}

fn block(bits: &mut Bits, lit: &Huffman, dist: &Huffman, out: &mut Vec<u8>) -> Result<(), String> {
    const LEN_BASE: [u16; 29] = [
        3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
        131, 163, 195, 227, 258,
    ];
    const LEN_EXTRA: [u32; 29] = [
        0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
    ];
    const DIST_BASE: [u16; 30] = [
        1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
        2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
    ];
    const DIST_EXTRA: [u32; 30] = [
        0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
        13, 13,
    ];
    loop {
        let sym = decode(bits, lit)?;
        match sym {
            0..=255 => out.push(sym as u8),
            256 => return Ok(()),
            _ => {
                let i = sym as usize - 257;
                if i >= LEN_BASE.len() {
                    return Err("length symbol out of range".into());
                }
                let len = LEN_BASE[i] as usize + bits.take(LEN_EXTRA[i])? as usize;
                let d = decode(bits, dist)? as usize;
                if d >= DIST_BASE.len() {
                    return Err("distance symbol out of range".into());
                }
                let back = DIST_BASE[d] as usize + bits.take(DIST_EXTRA[d])? as usize;
                if back > out.len() {
                    return Err("distance before the start of the output".into());
                }
                let from = out.len() - back;
                for k in 0..len {
                    out.push(out[from + k]);
                }
            }
        }
    }
}
