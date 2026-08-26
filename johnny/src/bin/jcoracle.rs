//! Answer the questions `tools/oracle.py` puts to the original binary.
//!
//! Each query prints one line per result in a fixed format, and the Python
//! side runs the same question through `SCRANTIC.EXE`'s own code in the 16-bit
//! interpreter and diffs the two. Anything that comes out here is either the
//! same as the 1992 code or a bug, with no third possibility and nobody's
//! opinion involved.
//!
//!     jcoracle rand 64
//!     jcoracle weighted 10,20,30,20,10,10 64
//!     jcoracle phase
//!     jcoracle choose 1 0 300 64

use johnny::ads::Rng;
use johnny::director::{self, category};
use johnny::tables;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let query = args.first().map(String::as_str).unwrap_or("");
    match query {
        "rand" => {
            let n: usize = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(64);
            let mut rng = Rng::new(0);
            for i in 0..n {
                println!("rand {i} {}", rng.next_u16());
            }
        }
        "weighted" => {
            let weights: Vec<u32> = args
                .get(1)
                .map(|v| v.split(',').filter_map(|w| w.parse().ok()).collect())
                .unwrap_or_default();
            let n: usize = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(64);
            let mut rng = Rng::new(0);
            for i in 0..n {
                println!("weighted {i} {}", johnny::scenery::weighted(&mut rng, &weights));
            }
        }
        // Every half hour of the day against every half hour of dawn is 2,304
        // answers, which is small enough to check exhaustively.
        "phase" => {
            for h in 0..24u16 {
                for m in [0u16, 30] {
                    for dh in 0..24u16 {
                        for dm in [0u16, 30] {
                            let now = h * 100 + m;
                            let dawn = dh * 100 + dm;
                            println!("phase {now} {dawn} {}", director::phase_at(now, dawn));
                        }
                    }
                }
            }
        }
        // The scene chooser, reported as row numbers in the table so the two
        // sides can be compared without sharing an address space.
        "choose" => {
            let cat: u32 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(category::PRIMARY);
            let phase: u8 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(0);
            let budget: i32 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(300);
            let n: usize = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(64);
            let mut d = director::Director::new(0);
            d.phase = phase;
            for i in 0..n {
                let row = d.choose(cat, budget).map(|s| {
                    let base = tables::SCENES.as_ptr() as usize;
                    (s as *const _ as usize - base) / std::mem::size_of::<tables::Scene>()
                });
                println!("choose {i} {}", row.map_or(-1, |r| r as i64));
            }
        }
        // The whole outer loop: how a queue of scenes gets built.
        "queue" => {
            let phase: u8 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
            let n: usize = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(16);
            let mut d = director::Director::new(0);
            d.phase = phase;
            for i in 0..n {
                let rows: Vec<String> =
                    d.queue_rows().iter().map(|r| r.to_string()).collect();
                println!("queue {i} {}", rows.join(","));
            }
        }
        // Where the island is dropped, for every state that means anything.
        "place" => {
            let n: usize = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(64);
            let mut rng = Rng::new(0);
            for state in [0xFFFFu16, 0, 1, 2, 4, 8, 0x10, 0x20, 0x0C, 0x30, 0x14, 0x28] {
                for layout in [false, true] {
                    for i in 0..n {
                        let (x, y) = johnny::scenery::place_island(state, layout, &mut rng);
                        println!("place {state} {} {i} {x} {y}", layout as u8);
                    }
                }
            }
        }
        // Where every instruction of every shipped script begins and ends,
        // which is the foundation the rest of the ADS work stands on.
        "adslen" => {
            let dir = std::env::var("JC_DATA").unwrap_or_else(|_| "original".into());
            let Ok(data) = std::fs::read(format!("{dir}/RESOURCE.001")) else {
                eprintln!("no {dir}/RESOURCE.001");
                return ExitCode::FAILURE;
            };
            let map = std::fs::read(format!("{dir}/RESOURCE.MAP")).ok();
            let Ok(archive) = johnny::dgds::Archive::load(&data, map.as_deref()) else {
                eprintln!("archive");
                return ExitCode::FAILURE;
            };
            for (_, name) in tables::ADS_FILES {
                let Some(body) = archive.get(name) else { continue };
                let Some(scr) = johnny::dgds::leaf(body, b"SCR:") else { continue };
                let Ok(code) = johnny::dgds::decompress(scr.payload) else { continue };
                let (ops, _) = johnny::ads::decode(&code);
                for (off, _) in &ops {
                    let raw = u16::from_le_bytes([code[*off], code[*off + 1]]);
                    println!("adslen {name} {off} {raw:04X} {}", johnny::ads::length(raw));
                }
            }
        }
        _ => {
            eprintln!("usage: jcoracle rand|weighted|phase|choose ...");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
