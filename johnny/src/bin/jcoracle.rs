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
        // Which way Johnny pivots, including the coin toss he makes when the
        // two facings are dead opposite.
        "turn" => {
            let n: usize = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(8);
            let mut rng = Rng::new(0);
            for at in 0..8u8 {
                for to in 0..8u8 {
                    for i in 0..n {
                        println!("turn {at} {to} {i} {}", director::turn_toward(at, to, &mut rng));
                    }
                }
            }
        }
        // The (block, position) pairs the route matrix offers, so both sides
        // walk the same list without sharing the table.
        "routes" => {
            for (from, to, at, _) in tables::ROUTE_TABLE {
                println!("{from},{to},{at}");
            }
        }
        // Which decoration goes on the island on each day of the year.
        // Eleven days out of 365, so nothing would ever notice this being
        // wrong.
        "holiday" => {
            let days = [31u8, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
            for month in 1..=12u8 {
                for day in 1..=days[month as usize - 1] {
                    let id = director::Holiday::for_date(month, day)
                        .map_or(0, |h| h.index() as u32 + 1);
                    println!("holiday {month} {day} {id}");
                }
            }
        }
        // What a scene's backdrop bits ask for. Two of them share one
        // variable in the engine, so this is where that shows.
        "backdrop" => {
            let bits = [0x0010u32, 0x0200, 0x0800, 0x1000, 0x2000, 0x4000];
            for combo in 0..64u32 {
                let mut mask = 0u32;
                for (k, b) in bits.iter().enumerate() {
                    if combo & (1 << k) != 0 {
                        mask |= b;
                    }
                }
                let mut b = johnny::scenery::Backdrop::default();
                b.apply(mask);
                println!(
                    "backdrop {mask:04X} raft={} water={} light={} holiday={} layout={}",
                    b.no_raft as u8,
                    b.still_water as u8,
                    // `ds:0x30B4`: 0 plain, 1 night, 2 cutaway.
                    if b.cutaway { 2 } else { b.night as u8 },
                    !b.no_holiday as u8,
                    !b.no_rock as u8
                );
            }
        }
        // How long a scene stays: the run state, the repeat count and the
        // deadline the flags operand asks for.
        "repeat" => {
            const NOW: u32 = 1000;
            for flags in [0i32, 1, 2, 3, 7, 100, 0x7FFF, -1, -2, -60, -300, -0x7FFF] {
                let r = johnny::ads::Repeat::from_flags(flags as u16);
                let state = match r.state() {
                    johnny::ads::SceneState::Queued(n) => n as u32,
                    johnny::ads::SceneState::Over => 4,
                    johnny::ads::SceneState::Finished => 5,
                    johnny::ads::SceneState::Idle => 0,
                };
                let deadline = match r {
                    johnny::ads::Repeat::ForTicks(n) => NOW + n,
                    _ => 0,
                };
                println!("repeat {flags} {state} {} {deadline}", r.count());
            }
        }
        // Which parts of the water a scene needs free, and what claiming
        // them leaves behind. A bitmask machine, so exhaustively.
        "island" => {
            let mut d = director::Director::new(0);
            for state in (0..64u16).chain(std::iter::once(0xFFFF)) {
                for want in 0..64u32 {
                    d.island_state = state;
                    println!(
                        "island allow {state} {want} {}",
                        d.island_allows(want << 16) as u8
                    );
                }
            }
            for state in (0..64u16).chain(std::iter::once(0xFFFF)) {
                for want in 0..64u32 {
                    d.island_state = state;
                    d.take_wishes(want << 16);
                    println!("island take {state} {want} {}", d.island_state);
                }
            }
        }
        // Which way Johnny goes: one roll of the route picker for every
        // (block, position) the six-by-six matrix offers.
        "route" => {
            let n: usize = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(32);
            let mut d = director::Director::new(0);
            // The engine draws `rand() % 100` here whatever the weights sum
            // to; the port's default corrects that, so the comparison has to
            // be against the faithful mode.
            d.faithful_router = true;
            for (from, to, at, _) in tables::ROUTE_TABLE {
                for i in 0..n {
                    let step = d.route_roll(*from, *to, *at);
                    println!("route {from} {to} {at} {i} {}", step.map_or(-1, |s| s as i16));
                }
            }
        }
        // One pass of one ADS sequence, with the scene states handed in, so
        // the same pass can be run inside the original interpreter and the
        // two action lists compared. This is the sequencer's whole contract.
        "seqpass" => {
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
            let record = args.get(1).cloned().unwrap_or_default();
            let seq: u16 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(1);
            let Some(mut s) = johnny::ads::Sequencer::from_record(&archive, &record) else {
                eprintln!("no ADS record {record}");
                return ExitCode::FAILURE;
            };
            // `ttm:scene:played:state,...`
            for entry in args.get(3).map(String::as_str).unwrap_or("").split(',') {
                let f: Vec<u16> = entry.split(':').filter_map(|v| v.parse().ok()).collect();
                if f.len() == 4 {
                    s.preload(f[0], f[1], f[2] as u32, f[3]);
                }
            }
            let seed: u64 = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(0);
            s.tracing = true;
            if !s.start(seq) {
                eprintln!("{record} has no sequence {seq}");
                return ExitCode::FAILURE;
            }
            let mut rng = Rng::new(seed);
            s.begin_pass();
            for _ in 0..johnny::ads::PASS_BUDGET {
                if matches!(s.next_action(&mut rng), johnny::ads::Action::PassOver) {
                    break;
                }
            }
            for line in &s.trace {
                println!("{line}");
            }
            // What the pass left behind, in the same order the oracle reads
            // it out of the interpreter's context: the sequence states, then
            // each scene the script mentions.
            println!("|");
            for (id, state) in s.sequence_states() {
                println!("seq{id}={state}");
            }
            for entry in args.get(5).map(String::as_str).unwrap_or("").split(',') {
                let f: Vec<u16> = entry.split(':').filter_map(|v| v.parse().ok()).collect();
                if f.len() == 2 {
                    println!("{}/{}={}", f[0], f[1], s.raw_state(f[0], f[1]));
                }
            }
        }
        // Several passes of one sequence, its state moving on between them
        // by the same rule the oracle applies to the interpreter: the lowest
        // scene that is playing has just ended, and what had just ended is
        // now idle. That is the only way to see the parts of the sequencer
        // that live between passes -- the parked resume pointer, and the one
        // pass state 4 lasts.
        "seqrun" => {
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
            let record = args.get(1).cloned().unwrap_or_default();
            let seq: u16 = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(1);
            let pairs: Vec<(u16, u16)> = args
                .get(3)
                .map(String::as_str)
                .unwrap_or("")
                .split(',')
                .filter_map(|e| {
                    let f: Vec<u16> = e.split(':').filter_map(|v| v.parse().ok()).collect();
                    (f.len() == 2).then(|| (f[0], f[1]))
                })
                .collect();
            let passes: usize = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(8);
            let Some(mut s) = johnny::ads::Sequencer::from_record(&archive, &record) else {
                eprintln!("no ADS record {record}");
                return ExitCode::FAILURE;
            };
            s.tracing = true;
            if !s.start(seq) {
                eprintln!("{record} has no sequence {seq}");
                return ExitCode::FAILURE;
            }
            let mut rng = Rng::new(0);
            for turn in 0..passes {
                s.trace.clear();
                if s.sequence_states().iter().any(|(id, st)| *id == seq && *st == 1) {
                    s.begin_pass();
                    for _ in 0..johnny::ads::PASS_BUDGET {
                        if matches!(s.next_action(&mut rng), johnny::ads::Action::PassOver) {
                            break;
                        }
                    }
                }
                let state = s
                    .sequence_states()
                    .iter()
                    .find(|(id, _)| *id == seq)
                    .map_or(0, |(_, st)| *st);
                let scenes: Vec<String> = pairs
                    .iter()
                    .map(|(t, sc)| format!("{t}/{sc}={}", s.raw_state(*t, *sc)))
                    .collect();
                println!(
                    "pass {turn} {} | seq={state} {}",
                    s.trace.join(" ; "),
                    scenes.join(" ")
                );
                // Move the world on, exactly as the oracle does.
                let mut sorted = pairs.clone();
                sorted.sort();
                let mut ended: Option<(u16, u16)> = None;
                for (t, sc) in &sorted {
                    match s.raw_state(*t, *sc) {
                        4 => s.set_state(*t, *sc, johnny::ads::SceneState::Idle),
                        1..=3 if ended.is_none() => ended = Some((*t, *sc)),
                        _ => {}
                    }
                }
                if let Some((t, sc)) = ended {
                    s.set_state(t, sc, johnny::ads::SceneState::Over);
                }
            }
        }
        // Where each guarded block ends. `seg10:0x106B` answers this in the
        // engine, and it is the thing that decides which instructions a
        // refused guard swallows -- stopping short leaves the tail of the
        // block to run as if it were the next rule.
        "blocks" => {
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
                let Some(mut s) = johnny::ads::Sequencer::from_record(&archive, name) else {
                    continue;
                };
                let (ops, _) = johnny::ads::decode(&code);
                for (i, (off, _)) in ops.iter().enumerate() {
                    println!(
                        "blocks {name} {off} {}",
                        s.block_end(i).map_or(-1, |e| e as i64)
                    );
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
