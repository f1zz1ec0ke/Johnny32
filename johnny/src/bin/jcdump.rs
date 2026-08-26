//! Emit a canonical digest of everything decoded out of `RESOURCE.001`.
//!
//! The point is differential testing. `tools/dgds_digest.py` prints the same
//! lines from the Python decoder that was validated against the original
//! disassembly, so `diff` answers "did the Rust port get this right" for every
//! chunk and every sprite frame rather than for a sample.
//!
//!     cargo run --release --bin jcdump -- original/RESOURCE.001 > rust.txt
//!     python3 tools/dgds_digest.py original/RESOURCE.001 > py.txt
//!     diff rust.txt py.txt

use johnny::dgds;
use std::process::ExitCode;

/// CRC-32 (IEEE), matching Python's `zlib.crc32`.
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

fn main() -> ExitCode {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "original/RESOURCE.001".to_string());
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let records = match dgds::read_archive(&data) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut failed = false;
    println!("records\t{}", records.len());
    for rec in &records {
        println!("record\t{}\t{}", rec.name, rec.body.len());

        for leaf in dgds::leaves(rec.body) {
            let tag = leaf.tag_str();
            if dgds::is_compressed(&rec.name, &leaf.tag) {
                match dgds::decompress(leaf.payload) {
                    Ok(out) => println!(
                        "  chunk\t{}\t{}\t{:08x}",
                        tag,
                        out.len(),
                        crc32(&out)
                    ),
                    Err(e) => {
                        println!("  chunk\t{tag}\tERROR\t{e}");
                        failed = true;
                    }
                }
            } else {
                println!(
                    "  raw\t{}\t{}\t{:08x}",
                    tag,
                    leaf.payload.len(),
                    crc32(leaf.payload)
                );
            }
        }

        let upper = rec.name.to_ascii_uppercase();
        if upper.ends_with(".BMP") {
            match dgds::read_bitmap(rec.body) {
                Ok(frames) => {
                    for (i, f) in frames.iter().enumerate() {
                        println!(
                            "  frame\t{}\t{}x{}\t{:08x}",
                            i,
                            f.width,
                            f.height,
                            crc32(&f.pixels)
                        );
                    }
                }
                Err(e) => {
                    println!("  frame\tERROR\t{e}");
                    failed = true;
                }
            }
        } else if upper.ends_with(".SCR") {
            match dgds::read_screen(rec.body) {
                Ok(f) => println!(
                    "  screen\t{}x{}\t{:08x}",
                    f.width,
                    f.height,
                    crc32(&f.pixels)
                ),
                Err(e) => {
                    println!("  screen\tERROR\t{e}");
                    failed = true;
                }
            }
        } else if upper.ends_with(".PAL") {
            match dgds::read_palette(rec.body) {
                Ok(pal) => {
                    let flat: Vec<u8> = pal.iter().flatten().copied().collect();
                    println!("  palette\t{}\t{:08x}", flat.len(), crc32(&flat));
                }
                Err(e) => {
                    println!("  palette\tERROR\t{e}");
                    failed = true;
                }
            }
        }
    }

    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
