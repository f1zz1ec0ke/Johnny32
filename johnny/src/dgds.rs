//! Reader for the Dynamix DGDS resource format (`RESOURCE.MAP` / `RESOURCE.001`).
//!
//! The traps in this format are documented in `docs/FILE_FORMATS.md`. The two
//! that matter, because both fail quietly rather than loudly:
//!
//! * the LZW stream is read in blocks of `code_size` bytes, and a code-width
//!   change or a CLEAR discards the rest of the current block;
//! * in the RLE stream, bit 7 set means *repeat*, not *literal*.
//!
//! Get either backwards and small inputs still decode perfectly.

use std::collections::HashMap;

#[derive(Debug)]
pub enum Error {
    Truncated(&'static str),
    BadCompression(u8),
    ShortDecode { expected: usize, got: usize },
    Missing(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Truncated(what) => write!(f, "truncated {what}"),
            Error::BadCompression(t) => write!(f, "unknown compression type {t}"),
            Error::ShortDecode { expected, got } => {
                write!(f, "decoded {got} bytes, expected {expected}")
            }
            Error::Missing(what) => write!(f, "missing {what}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

fn u16_at(d: &[u8], o: usize) -> Result<u16> {
    d.get(o..o + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or(Error::Truncated("u16"))
}

fn u32_at(d: &[u8], o: usize) -> Result<u32> {
    d.get(o..o + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(Error::Truncated("u32"))
}

// ---------------------------------------------------------------- containers

/// One record of `RESOURCE.001`: a 13-byte name, a length, then the body.
pub struct Record<'a> {
    pub name: String,
    pub body: &'a [u8],
    pub offset: usize,
}

/// Walk the archive. The records are laid end to end and consume it exactly.
pub fn read_archive(data: &[u8]) -> Result<Vec<Record<'_>>> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 17 <= data.len() {
        let raw = &data[off..off + 13];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(13);
        let name = String::from_utf8_lossy(&raw[..end]).into_owned();
        let size = u32_at(data, off + 13)? as usize;
        if name.is_empty() || off + 17 + size > data.len() {
            break;
        }
        out.push(Record {
            name,
            body: &data[off + 17..off + 17 + size],
            offset: off,
        });
        off += 17 + size;
    }
    Ok(out)
}

/// `RESOURCE.MAP`: a header, the archive name, then (hash, offset) pairs. The
/// offsets are redundant with a linear walk, but checking them is a cheap way
/// to notice a corrupt copy.
pub fn read_map(data: &[u8]) -> Result<(String, Vec<(u32, u32)>)> {
    let raw = data.get(6..19).ok_or(Error::Truncated("map header"))?;
    let end = raw.iter().position(|&b| b == 0).unwrap_or(13);
    let archive = String::from_utf8_lossy(&raw[..end]).into_owned();
    let count = u16_at(data, 19)? as usize;
    let mut entries = Vec::with_capacity(count);
    for i in 0..count {
        let o = 21 + i * 8;
        entries.push((u32_at(data, o)?, u32_at(data, o + 4)?));
    }
    Ok((archive, entries))
}

/// A leaf chunk: its four-character tag and its payload.
pub struct Leaf<'a> {
    pub tag: [u8; 4],
    pub payload: &'a [u8],
}

impl Leaf<'_> {
    pub fn tag_str(&self) -> &str {
        std::str::from_utf8(&self.tag).unwrap_or("????")
    }
}

/// Flatten a record body into its leaf chunks.
///
/// Each chunk is a four-character tag and a length whose top bit marks a
/// container, whose payload is itself a chunk list.
pub fn leaves(body: &[u8]) -> Vec<Leaf<'_>> {
    let mut out = Vec::new();
    collect(body, &mut out);
    out
}

fn collect<'a>(body: &'a [u8], out: &mut Vec<Leaf<'a>>) {
    let mut p = 0usize;
    while p + 8 <= body.len() {
        let tag = [body[p], body[p + 1], body[p + 2], body[p + 3]];
        if !tag.iter().all(|&c| (0x20..0x7f).contains(&c)) {
            break;
        }
        let raw = match u32_at(body, p + 4) {
            Ok(v) => v,
            Err(_) => break,
        };
        let container = raw & 0x8000_0000 != 0;
        let size = (raw & 0x7fff_ffff) as usize;
        let Some(payload) = body.get(p + 8..p + 8 + size) else {
            break;
        };
        if container {
            collect(payload, out);
        } else {
            out.push(Leaf { tag, payload });
        }
        p += 8 + size;
    }
}

/// Find one leaf by tag.
pub fn leaf<'a>(body: &'a [u8], tag: &[u8; 4]) -> Option<Leaf<'a>> {
    leaves(body).into_iter().find(|l| &l.tag == tag)
}

/// Does this leaf carry a `[type][uncompressed size]` header?
///
/// `VGA:` is the awkward one: inside a `.BMP` it is compressed, inside a
/// `.PAL` the payload is the raw palette.
/// CRC-32 (IEEE), matching Python's `zlib.crc32`. The differential test and
/// the codec oracle both name chunks by it, so both sides have to agree on
/// the polynomial as well as on the bytes.
pub fn crc32(data: &[u8]) -> u32 {
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

pub fn is_compressed(record_name: &str, tag: &[u8; 4]) -> bool {
    match tag {
        b"BIN:" | b"SCR:" | b"TT3:" => true,
        b"VGA:" => record_name.to_ascii_uppercase().ends_with(".BMP"),
        _ => false,
    }
}

// ------------------------------------------------------------- decompression

/// LZW, ported from `SCRANTIC.EXE` seg6:0x0D63 / 0x0FC1.
///
/// Unix-`compress` lineage: LSB-first codes of 9 to 12 bits, 256 is CLEAR, the
/// first free entry is 257. The details that are not guessable from the data:
///
/// * the input is consumed in blocks of `code_size` bytes (eight codes each),
///   and a width increase or a CLEAR abandons the rest of the current block;
/// * at 12 bits the original stores a max-code of `0x1000` rather than
///   `0x0FFF`, so the table freezes instead of ever resetting itself;
/// * after a CLEAR the next free entry is 256, *not* 257, and `old` is
///   deliberately not reset — the encoder does the same, so it round-trips.
pub fn lzw_decompress(data: &[u8], usize_hint: usize) -> Vec<u8> {
    let mut r = LzwReader::new(data);
    let mut prefix = [0u16; 4096];
    let mut suffix = [0u8; 4096];
    for i in 0..256 {
        suffix[i] = i as u8;
    }
    let mut next_code: usize = 257;

    let mut out = Vec::with_capacity(usize_hint);
    let mut stack: Vec<u8> = Vec::with_capacity(64);

    let Some(first) = r.next_code(next_code) else {
        return out;
    };
    let mut old = first;
    let mut fin = first;
    out.push(first as u8);

    while out.len() < usize_hint {
        let Some(mut code) = r.next_code(next_code) else {
            break;
        };
        if code == 0x100 {
            // CLEAR. The next free entry becomes 256, not 257, and `old` is
            // deliberately left alone; the encoder does the same.
            prefix[..256].fill(0);
            r.reset_pending = true;
            next_code = 0x100;
            let Some(c) = r.next_code(next_code) else {
                break;
            };
            code = c;
        }
        let in_code = code;
        stack.clear();
        if code >= next_code {
            // KwKwK: the code names the entry we are about to build.
            stack.push(fin as u8);
            code = old;
        }
        while code >= 256 {
            stack.push(suffix[code]);
            code = prefix[code] as usize;
        }
        fin = suffix[code] as usize;
        stack.push(fin as u8);
        out.extend(stack.iter().rev());
        if next_code < 0x1000 {
            prefix[next_code] = old as u16;
            suffix[next_code] = fin as u8;
            next_code += 1;
        }
        old = in_code;
    }
    out.truncate(usize_hint);
    out
}

/// The block-oriented bit reader that makes the above work.
///
/// Codes are taken LSB-first from a block of exactly `code_size` bytes, which
/// holds eight of them. When the width grows or a CLEAR arrives, the rest of
/// the block is abandoned and a fresh one is read. Treat the input as one
/// continuous bitstream instead and small files still decode perfectly while
/// large ones come out a few percent short.
struct LzwReader<'a> {
    data: &'a [u8],
    src: usize,
    block_start: usize,
    block_len: usize,
    bit_off: usize,
    avail_bits: usize,
    code_size: u32,
    max_code: usize,
    reset_pending: bool,
}

impl<'a> LzwReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        LzwReader {
            data,
            src: 0,
            block_start: 0,
            block_len: 0,
            bit_off: 0,
            avail_bits: 0,
            code_size: 9,
            max_code: (1 << 9) - 1,
            reset_pending: false,
        }
    }

    fn next_code(&mut self, next_free: usize) -> Option<usize> {
        if self.reset_pending || self.bit_off >= self.avail_bits || next_free > self.max_code {
            if next_free > self.max_code {
                self.code_size += 1;
                // At 12 bits the original stores 0x1000, not 0x0FFF. Since the
                // free entry also caps at 0x1000 the comparison stops firing,
                // so the table freezes rather than resetting.
                self.max_code = if self.code_size == 12 {
                    0x1000
                } else {
                    (1 << self.code_size) - 1
                };
            }
            if self.reset_pending {
                self.code_size = 9;
                self.max_code = (1 << 9) - 1;
                self.reset_pending = false;
            }
            let n = (self.code_size as usize).min(self.data.len() - self.src);
            if n == 0 {
                return None;
            }
            self.block_start = self.src;
            self.block_len = n;
            self.src += n;
            self.bit_off = 0;
            self.avail_bits = n * 8 - (self.code_size as usize - 1);
        }
        let block = &self.data[self.block_start..self.block_start + self.block_len];
        let mut code = 0usize;
        for k in 0..self.code_size as usize {
            let bi = self.bit_off + k;
            if let Some(&byte) = block.get(bi >> 3) {
                code |= (((byte >> (bi & 7)) & 1) as usize) << k;
            }
        }
        self.bit_off += self.code_size as usize;
        Some(code)
    }
}

/// RLE. Bit 7 set means repeat the next byte `n & 0x7F` times; bit 7 clear
/// means copy the next `n` bytes verbatim. The inverted reading is the natural
/// guess, and it decodes short streams almost correctly.
pub fn rle_decompress(data: &[u8], usize_hint: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(usize_hint);
    let mut p = 0usize;
    while p < data.len() && out.len() < usize_hint {
        let n = data[p];
        p += 1;
        if n & 0x80 != 0 {
            let Some(&b) = data.get(p) else { break };
            out.extend(std::iter::repeat(b).take((n & 0x7f) as usize));
            p += 1;
        } else {
            let n = n as usize;
            let end = (p + n).min(data.len());
            out.extend_from_slice(&data[p..end]);
            p += n;
        }
    }
    out.truncate(usize_hint);
    out
}

/// Decode a leaf payload of the form `[type: u8][uncompressed size: u32][data]`.
pub fn decompress(payload: &[u8]) -> Result<Vec<u8>> {
    let ctype = *payload.first().ok_or(Error::Truncated("compression header"))?;
    let want = u32_at(payload, 1)? as usize;
    let body = &payload[5..];
    let out = match ctype {
        0 => body.get(..want).unwrap_or(body).to_vec(),
        1 => rle_decompress(body, want),
        2 => lzw_decompress(body, want),
        other => return Err(Error::BadCompression(other)),
    };
    if out.len() != want {
        return Err(Error::ShortDecode {
            expected: want,
            got: out.len(),
        });
    }
    Ok(out)
}

// -------------------------------------------------------------------- images

/// A single frame: 8-bit palette indices, one byte per pixel. Index 0 is the
/// transparency key.
pub struct Frame {
    pub width: u16,
    pub height: u16,
    /// Palette indices. Everything in the archive is 0..15; index 0 is the
    /// transparency key.
    pub pixels: Vec<u8>,
    /// Colours of its own, as `0xAARRGGBB`, for artwork that did not come out
    /// of the archive. `None` for everything that did, which is then drawn
    /// through the live palette exactly as before.
    pub colour: Option<Vec<u32>>,
}

/// 4bpp, two pixels per byte, high nibble first, rows padded to a byte.
fn unpack4(raw: &[u8], w: u16, h: u16) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let stride = w.div_ceil(2);
    let mut px = vec![0u8; w * h];
    for y in 0..h {
        let row = y * stride;
        for x in 0..w {
            let Some(&b) = raw.get(row + (x >> 1)) else {
                continue;
            };
            px[y * w + x] = if x & 1 == 0 { b >> 4 } else { b & 0x0f };
        }
    }
    px
}

/// A `.BMP` record: a frame count and dimensions in `INF:`, the frames
/// concatenated in `BIN:`.
pub fn read_bitmap(body: &[u8]) -> Result<Vec<Frame>> {
    let inf = leaf(body, b"INF:").ok_or(Error::Missing("INF: chunk"))?;
    let bin = leaf(body, b"BIN:").ok_or(Error::Missing("BIN: chunk"))?;
    let n = u16_at(inf.payload, 0)? as usize;
    let mut sizes = Vec::with_capacity(n);
    for i in 0..n {
        let w = u16_at(inf.payload, 2 + i * 2)?;
        let h = u16_at(inf.payload, 2 + n * 2 + i * 2)?;
        sizes.push((w, h));
    }
    let raw = decompress(bin.payload)?;
    let mut out = Vec::with_capacity(n);
    let mut off = 0usize;
    for (w, h) in sizes {
        let len = (w as usize).div_ceil(2) * h as usize;
        let slice = raw.get(off..off + len).unwrap_or(&[]);
        out.push(Frame {
            width: w,
            height: h,
            pixels: unpack4(slice, w, h),
                     colour: None,
                 });
        off += len;
    }
    Ok(out)
}

/// A `.SCR` record: a full-screen background, 640x350.
pub fn read_screen(body: &[u8]) -> Result<Frame> {
    let dim = leaf(body, b"DIM:").ok_or(Error::Missing("DIM: chunk"))?;
    let bin = leaf(body, b"BIN:").ok_or(Error::Missing("BIN: chunk"))?;
    let w = u16_at(dim.payload, 0)?;
    let h = u16_at(dim.payload, 2)?;
    let raw = decompress(bin.payload)?;
    Ok(Frame {
        width: w,
        height: h,
        pixels: unpack4(&raw, w, h),
           colour: None,
       })
}

/// `JOHNCAST.PAL`: 256 RGB triples of 6-bit VGA values.
///
/// Only the first 16 entries are used. They are the EGA set with entries 0 and
/// 5 exchanged, so index 0 holds magenta and can serve as the transparency
/// key while black stays drawable at index 5.
pub fn read_palette(body: &[u8]) -> Result<[[u8; 3]; 256]> {
    let vga = leaf(body, b"VGA:").ok_or(Error::Missing("VGA: chunk"))?;
    let mut pal = [[0u8; 3]; 256];
    for (i, entry) in pal.iter_mut().enumerate() {
        for c in 0..3 {
            let v = vga.payload.get(i * 3 + c).copied().unwrap_or(0) as u32;
            entry[c] = ((v * 255) / 63).min(255) as u8;
        }
    }
    Ok(pal)
}

// --------------------------------------------------------------------- entry

/// Everything the engine needs out of one copy of the original data.
pub struct Archive {
    records: HashMap<String, Vec<u8>>,
    order: Vec<String>,
    /// Artwork that stands in for what is in here. See `crate::artpack`.
    pack: Option<std::rc::Rc<crate::artpack::ArtPack>>,
}

impl Archive {
    /// Lay an art pack over this archive. Every `bitmap` and `screen` from
    /// here on hands back the replacement where there is one.
    pub fn with_art(&mut self, pack: std::rc::Rc<crate::artpack::ArtPack>) {
        self.pack = Some(pack);
    }

    /// A record's sprite frames, with any replacements applied.
    pub fn bitmap(&self, name: &str) -> Option<Vec<Frame>> {
        let mut frames = read_bitmap(self.get(name)?).ok()?;
        if let Some(pack) = &self.pack {
            pack.apply(name, &mut frames);
        }
        Some(frames)
    }

    /// A record's full-screen image, with any replacement applied.
    pub fn screen(&self, name: &str) -> Option<Frame> {
        let mut frame = read_screen(self.get(name)?).ok()?;
        if let Some(pack) = &self.pack {
            pack.apply_one(name, &mut frame);
        }
        Some(frame)
    }

    /// Parse `RESOURCE.001`. Pass `RESOURCE.MAP` too and its offsets are
    /// checked against the linear walk.
    pub fn load(resource: &[u8], map: Option<&[u8]>) -> Result<Self> {
        let records = read_archive(resource)?;
        if let Some(map) = map {
            let (_, entries) = read_map(map)?;
            if entries.len() == records.len() {
                for (rec, (_, off)) in records.iter().zip(entries.iter()) {
                    if rec.offset != *off as usize {
                        return Err(Error::Truncated("archive/map offset mismatch"));
                    }
                }
            }
        }
        let mut order = Vec::with_capacity(records.len());
        let mut map_out = HashMap::with_capacity(records.len());
        for rec in records {
            order.push(rec.name.clone());
            map_out.insert(rec.name.to_ascii_uppercase(), rec.body.to_vec());
        }
        Ok(Archive {
            records: map_out,
            order,
            pack: None,
        })
    }

    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.records.get(&name.to_ascii_uppercase()).map(|v| &v[..])
    }

    pub fn names(&self) -> &[String] {
        &self.order
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}
