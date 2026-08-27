//! Run an ADS sequence with its scenes animating concurrently, and dump frames.
//!
//!     cargo run --release --bin jcplay -- MISCGAG.ADS 1 60
//!
//! Arguments: the `.ADS` record, the sequence number, an optional frame cap
//! and an optional RNG seed.
//!
//! This is the engine's actual shape. `PlayScene` does not replace whatever is
//! on screen — it adds an instance to a list that the runner advances every
//! tick (`seg10:0x0BF4` looks instances up by the `(ttm, scene)` pair, and
//! `+0x3B` chains them). Several scenes composite into one frame, which is how
//! the fire, a seagull and Johnny share the island.

use johnny::{
    ads::{self, Action},
    dgds::Archive,
    director::{advance_scenes, Active},
    png, ttm,
};
use std::collections::HashMap;
use std::process::ExitCode;
use std::rc::Rc;

/// One tick is 16ms; see `director::MS_PER_TICK`.
const MS_PER_TICK: u32 = johnny::director::MS_PER_TICK;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let record = args.next().unwrap_or_else(|| "MISCGAG.ADS".into());
    let sequence: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let cap: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);
    let seed: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0x5EED);

    let dir = std::env::var("JC_DATA").unwrap_or_else(|_| "original".into());
    let data = match std::fs::read(format!("{dir}/RESOURCE.001")) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{dir}/RESOURCE.001: {e}");
            return ExitCode::FAILURE;
        }
    };
    let map = std::fs::read(format!("{dir}/RESOURCE.MAP")).ok();
    let archive = match Archive::load(&data, map.as_deref()) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("archive: {e}");
            return ExitCode::FAILURE;
        }
    };

    let Some(mut seq) = ads::Sequencer::from_record(&archive, &record) else {
        eprintln!("no ADS record {record}");
        return ExitCode::FAILURE;
    };
    let label = seq
        .labels
        .get(&sequence)
        .cloned()
        .unwrap_or_else(|| "(unnamed)".into());
    println!("{record} sequence {sequence}: {label}");
    for (id, f) in &seq.files {
        println!("   res {id}: {f}");
    }
    println!();

    if !seq.start(sequence) {
        eprintln!("{record} has no sequence {sequence}");
        return ExitCode::FAILURE;
    }

    // Set JC_NOWRITE=1 to run the engine without dumping PNGs, which is what
    // the smoke test over every sequence wants.
    let write_frames = std::env::var("JC_NOWRITE").is_err();
    let out = std::path::Path::new("frames");
    if write_frames {
        if let Err(e) = std::fs::create_dir_all(out) {
            eprintln!("frames/: {e}");
            return ExitCode::FAILURE;
        }
    }

    // `JC_CHECK=1` holds the repaint model to its invariant every tick; see
    // `director::check_repaint`.
    let check = std::env::var("JC_CHECK").is_ok();
    let mut complaints = 0usize;

    let mut rng = ads::Rng::new(seed);
    let mut stage = ttm::Stage::new();
    let mut programs: HashMap<String, Rc<ttm::Program>> = HashMap::new();
    let mut active: Vec<Active> = Vec::new();
    // The instance list: one entry per `1111` marker, file by file in `RES:`
    // order, which is the order the engine loads them in. It is the draw
    // order, and a scene keeps its place in it whatever plays when.
    let mut order: Vec<(u16, u16)> = Vec::new();
    for (id, name) in seq.files.clone() {
        if let Some(p) = ttm::Program::load(&archive, &name) {
            for scene in &p.scenes {
                order.push((id, *scene));
            }
        }
    }
    let mut frame = 0usize;
    let mut tick = 0u32;
    let mut sequence_done = false;

    // Bound the tick count too. A sequence whose scenes never load a
    // background produces no frames, and without this the loop spins.
    let tick_limit = (cap as u32).saturating_mul(50).max(1000);
    while frame < cap && tick < tick_limit {
        // The instances move first, then the whole script is re-read from the
        // top -- the order `seg10:0x1ACB` uses, and the order that lets a
        // guard see the scene that ended this pass.
        stage.restore_background();
        for (_, scene) in advance_scenes(&mut active, &mut seq, &mut stage, &archive, tick) {
            println!("[tick {tick:5}] scene {scene} finished");
        }
        // Before the script runs: it can start scenes that have not been
        // painted yet, and those are not part of this tick's picture.
        if check && stage.has_background() {
            for what in johnny::director::check_repaint(&mut active, &mut stage) {
                println!("[tick {tick:5}] {what}");
                complaints += 1;
            }
        }

        seq.begin_pass();
        for _ in 0..johnny::ads::PASS_BUDGET {
            match seq.next_action(&mut rng) {
                Action::PassOver => break,
                Action::Reorder {
                    ttm,
                    scene,
                    to_front,
                } => {
                    // The engine moves the record inside the instance list
                    // itself, so the move outlasts the scene it was asked
                    // for.
                    if let Some(i) = order.iter().position(|e| *e == (ttm, scene)) {
                        let e = order.remove(i);
                        if to_front {
                            order.push(e);
                        } else {
                            order.insert(0, e);
                        }
                    }
                    if let Some(i) = active.iter().position(|a: &Active| (a.ttm, a.scene) == (ttm, scene)) {
                        let a = active.remove(i);
                        if to_front {
                            active.push(a);
                        } else {
                            active.insert(0, a);
                        }
                    }
                    continue;
                }
                Action::Play {
                    file,
                    scene,
                    repeat,
                } => {
                    let ttm_id = seq
                        .files
                        .iter()
                        .find(|(_, f)| **f == file)
                        .map(|(id, _)| *id)
                        .unwrap_or(0);
                    let program = match programs.get(&file) {
                        Some(p) => Rc::clone(p),
                        None => {
                            let Some(p) = ttm::Program::load(&archive, &file) else {
                                eprintln!("no TTM record {file}");
                                return ExitCode::FAILURE;
                            };
                            let p = Rc::new(p);
                            programs.insert(file.clone(), Rc::clone(&p));
                            // The header before the first scene marker holds
                            // the background, palette and bitmap loads. Run it
                            // once per file, into the shared stage.
                            let mut header = ttm::Instance::new(Rc::clone(&p));
                            let _ = header.run_frame(&mut stage, &archive);
                            p
                        }
                    };
                    println!("[tick {tick:5}] play {file} scene {scene} {repeat:?}");
                    // Already on screen: re-arm it where it stands.
                    // `seg10:0x0DB6` leaves the program pointer alone, and so
                    // does the director -- so this tool has to as well, or it
                    // is not watching the same animation the screensaver runs.
                    if let Some(a) = active
                        .iter_mut()
                        .find(|a: &&mut Active| (a.ttm, a.scene) == (ttm_id, scene))
                    {
                        a.rearm(repeat, tick);
                        continue;
                    }
                    let mut inst = ttm::Instance::new(program);
                    if !inst.seek_scene(scene) {
                        // `seg10:0x1DBA` ends it on its first tick rather
                        // than never starting it.
                        println!("   (no scene {scene} in {file})");
                        inst.exhaust();
                    }
                    // At the depth the instance list has always had it.
                    let rank = johnny::director::depth(&order, ttm_id, scene);
                    let at = active
                        .iter()
                        .position(|a: &Active| {
                            johnny::director::depth(&order, a.ttm, a.scene) > rank
                        })
                        .unwrap_or(active.len());
                    active.insert(at, Active::start(ttm_id, scene, inst, repeat, tick));
                }
            }
        }
        if !sequence_done && !seq.running() {
            sequence_done = true;
            println!("[tick {tick:5}] sequence finished");
        }
        if sequence_done && active.iter().all(|a| a.looping) {
            break;
        }

        if stage.has_background() {
            if write_frames {
                let path = out.join(format!("play_{frame:03}.png"));
                let _ = png::write_indexed(
                    &path,
                    stage.surface.width,
                    stage.surface.height,
                    &stage.surface.pixels,
                    stage.palette(),
                    None,
                );
            }
            frame += 1;
        }
        tick += 1;
    }

    println!(
        "\n{frame} frames over {tick} ticks, {:.1}s at {MS_PER_TICK}ms/tick",
        (tick * MS_PER_TICK) as f64 / 1000.0
    );
    if check {
        println!(
            "repaint check: {}",
            if complaints == 0 {
                "clean".to_string()
            } else {
                format!("{complaints} complaints")
            }
        );
    }
    if !stage.todo_seen.is_empty() {
        println!("TTM opcodes not implemented:");
        for (op, n) in &stage.todo_seen {
            println!("  {op:04X}  x{n}");
        }
    }
    if !seq.todo_seen.is_empty() {
        println!("ADS opcodes not implemented:");
        for (op, n) in &seq.todo_seen {
            println!("  {op:04X}  x{n}");
        }
    }
    ExitCode::SUCCESS
}
