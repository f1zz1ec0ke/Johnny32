//! Check an art pack against the archive it stands in for.
//!
//!     cargo run --release --bin jcart -- art
//!
//! Says what a pack covers, what it does not, and -- the one mistake worth
//! catching early -- which replacements are the wrong size. The scripts place
//! every sprite themselves in the original's coordinates, so a frame drawn a
//! few pixels larger does not land a few pixels larger: it is refused, and
//! the original is drawn instead.

use johnny::{artpack::ArtPack, dgds::Archive};
use std::process::ExitCode;

fn main() -> ExitCode {
    let root = std::env::args().nth(1).unwrap_or_else(|| "art".into());
    let dir = std::env::var("JC_DATA").unwrap_or_else(|_| "original".into());
    let Ok(data) = std::fs::read(format!("{dir}/RESOURCE.001")) else {
        eprintln!("no {dir}/RESOURCE.001");
        return ExitCode::FAILURE;
    };
    let map = std::fs::read(format!("{dir}/RESOURCE.MAP")).ok();
    let Ok(mut archive) = Archive::load(&data, map.as_deref()) else {
        eprintln!("could not read the archive");
        return ExitCode::FAILURE;
    };
    let pack = std::rc::Rc::new(ArtPack::load(std::path::Path::new(&root)));
    println!("{}: {} frames read", root, pack.loaded);
    for line in &pack.rejected {
        println!("  unreadable  {line}");
    }

    archive.with_art(std::rc::Rc::clone(&pack));

    let mut covered = 0usize;
    let mut total = 0usize;
    let mut wrong = Vec::new();
    let mut partial = Vec::new();
    for name in archive.names().to_vec() {
        if name.ends_with(".BMP") {
            let Some(frames) = archive.bitmap(&name) else { continue };
            total += frames.len();
            let n = frames.iter().filter(|f| f.colour.is_some()).count();
            covered += n;
            wrong.extend(pack.mismatches(&name, &frames));
            if n > 0 && n < frames.len() {
                partial.push(format!("{name}: {n} of {}", frames.len()));
            }
        } else if name.ends_with(".SCR") {
            let Some(frame) = archive.screen(&name) else { continue };
            total += 1;
            covered += frame.colour.is_some() as usize;
        }
    }
    for line in &wrong {
        println!("  wrong size  {line}");
    }
    if !partial.is_empty() {
        println!("\npart done:");
        for line in partial.iter().take(20) {
            println!("  {line}");
        }
    }
    println!(
        "\n{covered} of {total} images redrawn ({:.1}%){}",
        100.0 * covered as f32 / total.max(1) as f32,
        if wrong.is_empty() {
            String::new()
        } else {
            format!(", {} refused for size", wrong.len())
        }
    );
    if wrong.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
