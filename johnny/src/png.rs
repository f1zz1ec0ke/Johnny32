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
