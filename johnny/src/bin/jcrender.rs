//! Run one `.TTM` scene and write its frames out as PNGs.
//!
//!     cargo run --release --bin jcrender -- MJRAFT.TTM 3 12
//!
//! Arguments: the record name, an optional scene number, an optional frame
//! count. Reads `original/RESOURCE.001` unless `JC_DATA` says otherwise.

use johnny::{dgds::Archive, png, ttm};
use std::process::ExitCode;
use std::rc::Rc;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let record = args.next().unwrap_or_else(|| "MJRAFT.TTM".into());
    let scene: Option<u16> = args.next().and_then(|s| s.parse().ok());
    let frames: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(8);

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

    let Some(program) = ttm::Program::load(&archive, &record) else {
        eprintln!("no TTM record {record}");
        return ExitCode::FAILURE;
    };
    let mut stage = ttm::Stage::new();
    let mut inst = ttm::Instance::new(Rc::new(program));
    if let Some(s) = scene {
        // The header before the first scene marker does the loads, so run it
        // once before seeking, or the scene paints onto nothing.
        let _ = inst.run_frame(&mut stage, &archive);
        if !inst.seek_scene(s) {
            eprintln!("{record} has no scene {s}");
            return ExitCode::FAILURE;
        }
    }

    let out = std::path::Path::new("frames");
    if let Err(e) = std::fs::create_dir_all(out) {
        eprintln!("frames/: {e}");
        return ExitCode::FAILURE;
    }

    let stem = record.split('.').next().unwrap_or("scene").to_string();
    for i in 0..frames {
        match inst.run_frame(&mut stage, &archive) {
            ttm::Step::Finished => {
                println!("finished after {i} frames");
                break;
            }
            ttm::Step::Frame { asked, had } => {
                let delay = asked.unwrap_or(had);
                let path = out.join(format!("{stem}_{i:03}.png"));
                if let Err(e) = png::write_indexed(
                    &path,
                    stage.surface.width,
                    stage.surface.height,
                    &stage.surface.pixels,
                    stage.palette(),
                    None,
                ) {
                    eprintln!("{}: {e}", path.display());
                    return ExitCode::FAILURE;
                }
                println!("{} (delay {delay})", path.display());
                stage.restore_background();
            }
        }
    }

    if !stage.todo_seen.is_empty() {
        println!("\nopcodes decoded but not implemented:");
        for (op, n) in &stage.todo_seen {
            println!("  {op:04X}  x{n}");
        }
    }
    ExitCode::SUCCESS
}
