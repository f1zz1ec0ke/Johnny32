//! Run an ADS sequence, letting it drive the TTM scenes, and dump frames.
//!
//!     cargo run --release --bin jcplay -- MISCGAG.ADS 1 40
//!
//! Arguments: the `.ADS` record, the sequence number, an optional frame cap
//! and an optional RNG seed. This is the sequencer and interpreter working
//! together — what the screensaver actually does between scheduler decisions.

use johnny::{
    ads::{self, Action},
    dgds::Archive,
    png, ttm,
};
use std::process::ExitCode;

/// One tick is 4ms. The engine registers a single 4-unit software timer
/// (`seg2:0x1522` into `seg9:0x07E6`) against `USER.15`, which is
/// `GetCurrentTime` — milliseconds. `SetDelay n` therefore waits 4n ms.
const MS_PER_TICK: u32 = 4;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let record = args.next().unwrap_or_else(|| "MISCGAG.ADS".into());
    let sequence: u16 = args.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let cap: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(40);
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

    let out = std::path::Path::new("frames");
    if let Err(e) = std::fs::create_dir_all(out) {
        eprintln!("frames/: {e}");
        return ExitCode::FAILURE;
    }

    let mut rng = ads::Rng::new(seed);
    let mut machine: Option<ttm::Machine> = None;
    let mut loaded: Option<String> = None;
    let mut playing: Option<(u16, u16)> = None;
    let mut frame = 0usize;
    let mut elapsed_ms = 0u32;

    while frame < cap {
        match seq.next_action(&mut rng) {
            Action::Done => {
                println!("sequence finished");
                break;
            }
            Action::Play { file, scene } => {
                let ttm_id = seq
                    .files
                    .iter()
                    .find(|(_, f)| **f == file)
                    .map(|(id, _)| *id)
                    .unwrap_or(0);
                println!("play {file} scene {scene}");
                // Keep the machine across scenes of the same file so its
                // loaded slots, palette and background survive; a different
                // file needs its own program and its header run first.
                let reuse = matches!(&loaded, Some(f) if *f == file);
                if !reuse {
                    let Some(mut m) = ttm::Machine::from_record(&archive, &file) else {
                        eprintln!("no TTM record {file}");
                        return ExitCode::FAILURE;
                    };
                    // The header before the first scene marker does the
                    // background, palette and bitmap loads.
                    let _ = m.run_frame(&archive);
                    machine = Some(m);
                    loaded = Some(file.clone());
                }
                if let Some(m) = machine.as_mut() {
                    if !m.seek_scene(scene) {
                        println!("   (no scene {scene} in {file})");
                    }
                }
                playing = Some((ttm_id, scene));
            }
            Action::Wait => {
                // Step ends the block and yields. The runner advances the
                // active scene by one frame, which is also what promotes a
                // queued scene to running -- the next block's IF_RUNNING
                // depends on that having happened during this yield.
                if let Some((ttm_id, scene)) = playing {
                    seq.scene_running(ttm_id, scene);
                }
                let Some(m) = machine.as_mut() else { continue };
                match m.run_frame(&archive) {
                    ttm::Step::Finished => {
                        if let Some((ttm_id, scene)) = playing {
                            seq.scene_finished(ttm_id, scene);
                        }
                    }
                    ttm::Step::Frame(delay) => {
                        let path = out.join(format!("play_{frame:03}.png"));
                        let _ = png::write_indexed(
                            &path,
                            m.surface.width,
                            m.surface.height,
                            &m.surface.pixels,
                            m.palette(),
                            None,
                        );
                        elapsed_ms += delay as u32 * MS_PER_TICK;
                        frame += 1;
                        m.restore_background();
                    }
                }
            }
        }
    }

    println!(
        "\n{frame} frames, {:.1}s of animation at {MS_PER_TICK}ms/tick",
        elapsed_ms as f64 / 1000.0
    );
    if !seq.todo_seen.is_empty() {
        println!("ADS opcodes not implemented:");
        for (op, n) in &seq.todo_seen {
            println!("  {op:04X}  x{n}");
        }
    }
    ExitCode::SUCCESS
}
