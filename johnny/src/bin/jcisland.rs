//! Let Johnny live on his island: the scheduler picking scenes, walking him
//! to them, and running them.
//!
//!     cargo run --release --bin jcisland -- 300
//!
//! Arguments: a tick count, an optional RNG seed. `JC_NOWRITE=1` skips the
//! PNG output; `JC_FAITHFUL=1` restores the 1992 walk-router bug; `JC_DAY=n`
//! starts on story day `n`, which is what decides how far the raft has got.

use johnny::{dgds::Archive, director::Director, png};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let ticks: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(300);
    let seed: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0x15_1A2D);

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

    let write_frames = std::env::var("JC_NOWRITE").is_err();
    let out = std::path::Path::new("frames");
    if write_frames {
        if let Err(e) = std::fs::create_dir_all(out) {
            eprintln!("frames/: {e}");
            return ExitCode::FAILURE;
        }
    }

    let mut d = Director::new(seed);
    d.faithful_router = std::env::var("JC_FAITHFUL").is_ok();
    // Start on a given story day, to see the island as it is by then --
    // mostly, to watch the raft get built.
    if let Some(day) = std::env::var("JC_DAY").ok().and_then(|v| v.parse().ok()) {
        d.day = day;
    }
    let mut frame = 0usize;
    let mut logged = 0usize;
    let mut waves_max = 0usize;
    let mut waves_total = 0usize;

    for _ in 0..ticks {
        let drew = d.tick(&archive);
        waves_max = waves_max.max(d.wave_count());
        waves_total += d.wave_count();
        while logged < d.log.len() {
            // Ticks and wall clock, so pacing can be read off the log.
            let t = d.ticks();
            let secs = t as f32 * johnny::director::MS_PER_TICK as f32 / 1000.0;
            println!("[{t:6} {secs:7.1}s] {}", d.log[logged]);
            logged += 1;
        }
        if drew && write_frames {
            let path = out.join(format!("island_{frame:04}.png"));
            let _ = png::write_indexed(
                &path,
                d.stage.surface.width,
                d.stage.surface.height,
                &d.stage.surface.pixels,
                d.stage.palette(),
                None,
            );
        }
        if drew {
            frame += 1;
        }
    }

    println!(
        "\nwaves: {waves_max} at most, {:.1} on average",
        waves_total as f32 / ticks.max(1) as f32
    );
    println!(
        "{frame} frames in {:.1}s, day {}, phase {} ({}), Johnny at {:?}",
        d.ticks() as f32 * johnny::director::MS_PER_TICK as f32 / 1000.0,
        d.day,
        d.phase,
        if d.is_night() { "night" } else { "day" },
        d.position
    );
    if !d.stage.todo_seen.is_empty() {
        println!("TTM opcodes not implemented:");
        for (op, n) in &d.stage.todo_seen {
            println!("  {op:04X}  x{n}");
        }
    }
    ExitCode::SUCCESS
}
