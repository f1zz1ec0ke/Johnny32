//! Let Johnny live on his island: the scheduler picking scenes, walking him
//! to them, and running them.
//!
//!     cargo run --release --bin jcisland -- 300
//!
//! Arguments: a tick count, an optional RNG seed. `JC_NOWRITE=1` skips the
//! PNG output; `JC_FAITHFUL=1` restores the 1992 walk-router bug; `JC_DAY=n`
//! starts on story day `n`, which is what decides how far the raft has got;
//! `JC_HHMM=hhmm` pins the wall clock instead of stepping a phase per queue;
//! `JC_FROM`/`JC_TO` bound which ticks get written as PNGs;
//! `JC_DATE=YYYY-MM-DD` pins the calendar, for the holiday decorations.

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
    // Write only part of a long run: `JC_FROM`/`JC_TO` are tick numbers.
    let from: u32 = std::env::var("JC_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let to: u32 = std::env::var("JC_TO").ok().and_then(|v| v.parse().ok()).unwrap_or(u32::MAX);
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
        d.date = 0;
    }
    // `JC_DATE=YYYY-MM-DD` pins the calendar, which is how you get a holiday
    // decoration without waiting for October.
    if let Some(date) = std::env::var("JC_DATE").ok() {
        let parts: Vec<i64> = date.split('-').filter_map(|p| p.parse().ok()).collect();
        if let [y, m, dd] = parts[..] {
            d.date = johnny::director::days_from_civil(y, m as u32, dd as u32);
            d.story_date_reset();
        }
    }
    // The shipped clock is the wall clock: half hours since the dawn the user
    // set. A test run is minutes long, so step the phase per queue instead and
    // let a long one walk through a whole day.
    d.clock = match std::env::var("JC_HHMM").ok().and_then(|v| v.parse().ok()) {
        Some(hhmm) => {
            d.hhmm = Some(hhmm);
            johnny::director::Clock::Wall {
                dawn: johnny::director::DEFAULT_DAWN,
            }
        }
        None => johnny::director::Clock::PerQueue,
    };
    // The sound effects live in the original's executable, not the archive.
    let sounds = ["SCRANTIC.EXE", "scrantic.exe"]
        .iter()
        .find_map(|name| std::fs::read(format!("{dir}/{name}")).ok())
        .and_then(|exe| johnny::sound::Sounds::from_exe(&exe));
    match sounds.as_ref() {
        Some(s) => println!("sounds: {} of {} found", s.count(), johnny::sound::LAST),
        None => println!("sounds: no SCRANTIC.EXE in {dir}"),
    }
    let mut heard: std::collections::BTreeMap<u16, usize> = Default::default();
    let mut frame = 0usize;
    let mut logged = 0usize;
    let mut waves_max = 0usize;
    let mut waves_total = 0usize;
    let mut clouds_max = 0usize;
    let mut glints_max = 0usize;
    let mut active_max = 0usize;

    for _ in 0..ticks {
        let drew = d.tick(&archive);
        for id in d.take_sounds() {
            *heard.entry(id).or_insert(0) += 1;
        }
        waves_max = waves_max.max(d.wave_count());
        waves_total += d.wave_count();
        clouds_max = clouds_max.max(d.cloud_count());
        glints_max = glints_max.max(d.glint_count());
        active_max = active_max.max(d.active_count());
        while logged < d.log.len() {
            // Ticks and wall clock, so pacing can be read off the log.
            let t = d.ticks();
            let secs = t as f32 * johnny::director::ms_per_tick() as f32 / 1000.0;
            println!("[{t:6} {secs:7.1}s] {}", d.log[logged]);
            logged += 1;
        }
        if drew && write_frames && (from..=to).contains(&d.ticks()) {
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
        "\nwaves: {waves_max} at most, {:.1} on average; clouds: {clouds_max}, glints: {glints_max}, scenes at once: {active_max}",
        waves_total as f32 / ticks.max(1) as f32
    );
    let played: usize = heard.values().sum();
    let silent: Vec<u16> = heard
        .keys()
        .copied()
        .filter(|id| sounds.as_ref().map_or(true, |s| s.wave(*id).is_none()))
        .collect();
    println!(
        "sounds: {played} played, {} distinct{}",
        heard.len(),
        if silent.is_empty() {
            String::new()
        } else {
            format!(" ({silent:?} have no resource)")
        }
    );
    println!(
        "{frame} frames in {:.1}s, day {}, phase {} ({}), Johnny at {:?}",
        d.ticks() as f32 * johnny::director::ms_per_tick() as f32 / 1000.0,
        d.day,
        d.phase,
        if d.is_night() { "night" } else { "day" },
        d.position
    );
    if std::env::var_os("JC_CHECK").is_some() {
        if d.complaints.is_empty() {
            println!("repaint check: clean");
        } else {
            println!("repaint check: {} complaints", d.complaints.len());
            for line in d.complaints.iter().take(20) {
                println!("  {line}");
            }
        }
    }
    if !d.ads_todo.is_empty() {
        println!("ADS opcodes not implemented:");
        for (op, n) in &d.ads_todo {
            println!("  {op:04X}  x{n}");
        }
    }
    if !d.stage.todo_seen.is_empty() {
        println!("TTM opcodes not implemented:");
        for (op, n) in &d.stage.todo_seen {
            println!("  {op:04X}  x{n}");
        }
    }
    ExitCode::SUCCESS
}
