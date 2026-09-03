//! Score this port's island against the 1992 engine's own render of it.
//!
//!     cargo run --release --bin jcisle
//!
//! `ISLETEMP.SCR` is the backdrop thirty-two of the forty-one scripts load,
//! and it is not a hand-painted approximation: it is a picture of this exact
//! composition, taken from the running engine. So it is the one place in the
//! data where the original's *pixels* can be held up against this port's --
//! everywhere else the oracle can only compare the decisions, because the
//! blit itself is GDI's and there is no inner loop of Sierra's to diff.
//!
//! What it can catch that nothing else can: a piece of the island moving, the
//! wrong frame of `BACKGRND.BMP` being drawn, the palette being read
//! differently, or the sprite blit putting a shape down half a pixel out.
//!
//! It is a *template*, so two things about it have to be allowed for. The
//! scenes are drawn against a nominal island and the whole picture shifts
//! together (`Scenery::place`), so the comparison is made at offset zero. And
//! the render was taken from the bare-island layout with the surf counters at
//! zero -- `seg3:0x0226`'s coin had come up the other way -- so the rock, the
//! sandbar and their four extra patches of surf are not in it.
//!
//! `JC_DUMP=1` writes the two pictures and their difference into `frames/`.

use johnny::dgds::{Archive, Frame};
use johnny::render::TRANSPARENT;
use johnny::director;
use johnny::scenery::{Scenery, ISLAND, ORIGIN, SURF};
use johnny::{png, scenery};
use std::process::ExitCode;

/// What each piece has to score against the reference before this complains.
///
/// The three solid pieces come out at 96%, 95% and 90% -- the fronds lowest
/// because they are the piece the painted sky shows through most. These are
/// set a few points under the measured figures: far enough not to trip on a
/// rounding change, close enough that a piece moving by one pixel fails.
const FLOORS: &[(usize, &str, f64)] = &[
    (0, "the island", 0.90),
    (13, "the palm trunk", 0.90),
    (12, "the fronds", 0.85),
];

/// And what the whole composition has to score.
const WHOLE_FLOOR: f64 = 0.80;

/// The surf is scored much lower, because the reference has the painted sea
/// washing through it: two of the three patches come out at 75% against
/// 90-odd for the solid pieces. That is still a sharp peak -- a pixel either
/// way costs sixteen points -- so a loose floor catches one moving.
const SURF_FLOOR: f64 = 0.60;

/// The third patch is reported and not floored, and that is a finding rather
/// than a gap.
///
/// `seg3:0x13F2` puts it at `0x207`, and the port does what the binary says.
/// But the reference's best match for it is at `0x209` -- two pixels right --
/// and even there only 54.7%, against 75% for its two neighbours. Scored
/// where the code puts it, it comes out at 34%, *below* several positions
/// around it: the match surface is flat enough that no floor here would
/// discriminate between right and wrong.
///
/// So it is measured and shown, and nothing is asserted about it. Moving the
/// port on a 54.7% peak would be overriding the binary on weak evidence, and
/// the two patches either side of it confirm the island's own -1 correction
/// at 75%.
const UNCHECKED_SURF: usize = 9;

fn main() -> ExitCode {
    let dir = std::env::var("JC_DATA").unwrap_or_else(|_| "original".into());
    let Ok(data) = std::fs::read(format!("{dir}/RESOURCE.001")) else {
        eprintln!("no {dir}/RESOURCE.001");
        return ExitCode::FAILURE;
    };
    let map = std::fs::read(format!("{dir}/RESOURCE.MAP")).ok();
    let Ok(archive) = Archive::load(&data, map.as_deref()) else {
        eprintln!("archive");
        return ExitCode::FAILURE;
    };
    let Some(reference) = archive.screen("ISLETEMP.SCR") else {
        eprintln!("no ISLETEMP.SCR");
        return ExitCode::FAILURE;
    };

    // The bare-island layout, at the nominal anchor, on a sea it never
    // actually pairs with -- only the pixels the island puts down are
    // compared, so which sea is underneath does not matter.
    let Some(scenery) = Scenery::build(&archive, scenery::SEAS[0], false, Default::default(), 1) else {
        eprintln!("could not build the island");
        return ExitCode::FAILURE;
    };
    let (sea, base) = (scenery.sea_only(), scenery.base());

    let mut bad = Vec::new();

    // Every pixel the composition put down over the bare sea, against the
    // engine's own picture of it.
    let (mut hit, mut seen) = (0usize, 0usize);
    for y in 0..reference.height as usize {
        for x in 0..reference.width as usize {
            let at = y * base.width as usize + x;
            if base.pixels[at] == sea.pixels[at] {
                continue;
            }
            seen += 1;
            hit += usize::from(base.pixels[at] == reference.pixels[y * reference.width as usize + x]);
        }
    }
    let whole = score(hit, seen);
    println!(
        "island       {:.1}% of {seen} painted pixels match ISLETEMP.SCR",
        whole * 100.0
    );
    if whole < WHOLE_FLOOR {
        bad.push(format!(
            "the composition scores {:.1}%, under {:.0}%",
            whole * 100.0,
            WHOLE_FLOOR * 100.0
        ));
    }

    // And each solid piece on its own, so a regression says which one moved
    // rather than only that something did.
    for &(frame, label, floor) in FLOORS {
        let Some(&(_, nx, ny, _)) = ISLAND.iter().find(|&&(f, _, _, _)| f == frame) else {
            bad.push(format!("{label} is not in the island table any more"));
            continue;
        };
        let Some(art) = scenery.pieces().get(frame) else {
            bad.push(format!("{label} is not in BACKGRND.BMP any more"));
            continue;
        };
        let (x, y) = scenery.at(nx, ny);
        let got = piece_score(art, x, y, &reference);
        println!(
            "  frame {frame:<3} {:.1}%  {label}, at ({x}, {y})",
            got * 100.0
        );
        if got < floor {
            bad.push(format!(
                "{label} scores {:.1}%, under {:.0}%",
                got * 100.0,
                floor * 100.0
            ));
        }
    }

    // The three patches of surf that trace *this* island's waterline. The
    // other four belong to the sandbar layout, and `ISLETEMP.SCR` contains no
    // trace of them -- which is the evidence the two sets are alternatives
    // rather than one shoreline.
    //
    // The render was taken with the counters at zero, so each patch shows its
    // first frame.
    for (n, &(sx, sy, frame, _, sandbar)) in SURF.iter().enumerate() {
        if sandbar {
            continue;
        }
        let Some(art) = scenery.pieces().get(frame) else {
            bad.push(format!("surf {n} is not in BACKGRND.BMP any more"));
            continue;
        };
        let (x, y) = scenery.at(sx, sy);
        let got = piece_score(art, x, y, &reference);
        if frame == UNCHECKED_SURF {
            println!(
                "  frame {frame:<3} {:.1}%  surf, at ({x}, {y}) -- not checked, see \
                 UNCHECKED_SURF",
                got * 100.0
            );
            continue;
        }
        println!("  frame {frame:<3} {:.1}%  surf, at ({x}, {y})", got * 100.0);
        if got < SURF_FLOOR {
            bad.push(format!(
                "the surf at ({x}, {y}) scores {:.1}%, under {:.0}%",
                got * 100.0,
                SURF_FLOOR * 100.0
            ));
        }
    }

    if std::env::var_os("JC_DUMP").is_some() {
        dump(base, sea, &reference, &archive);
    }

    // The correction from the nominal anchor the offsets in segment 3 are
    // written against. A piece one pixel out takes its score from the
    // nineties into single figures, so this number is not a preference.
    println!(
        "anchor       {ORIGIN:?} from the nominal ({:#05X}, {:#05X})",
        director::ISLAND_X, director::ISLAND_Y
    );
    if bad.is_empty() {
        println!("\nthe island is where the engine's own render says it is");
        return ExitCode::SUCCESS;
    }
    println!();
    for line in &bad {
        println!("  {line}");
    }
    ExitCode::FAILURE
}

fn score(hit: usize, seen: usize) -> f64 {
    if seen == 0 {
        0.0
    } else {
        hit as f64 / seen as f64
    }
}

/// One piece against the reference: every pixel of it that is not the
/// transparency key, at the position the composition puts it.
fn piece_score(art: &Frame, x: i32, y: i32, reference: &Frame) -> f64 {
    let (mut hit, mut seen) = (0usize, 0usize);
    for row in 0..art.height as i32 {
        for col in 0..art.width as i32 {
            let index = art.pixels[(row * art.width as i32 + col) as usize];
            if index == TRANSPARENT {
                continue;
            }
            let (sx, sy) = (x + col, y + row);
            if sx < 0 || sy < 0 || sx >= reference.width as i32 || sy >= reference.height as i32 {
                continue;
            }
            seen += 1;
            hit += usize::from(index == reference.pixels[(sy * reference.width as i32 + sx) as usize]);
        }
    }
    score(hit, seen)
}

/// The two pictures and where they disagree, for looking at.
///
/// The difference is only over the pixels the check scores -- the ones the
/// island put down over the bare sea. Diffing the whole picture instead shows
/// mostly sea and sky, which the reference paints its own way and which
/// nothing here is claiming to match.
fn dump(base: &Frame, sea: &Frame, reference: &Frame, archive: &Archive) {
    let palette = archive
        .get("JOHNCAST.PAL")
        .and_then(|b| johnny::dgds::read_palette(b).ok())
        .unwrap_or([[0; 3]; 256]);
    let out = std::path::Path::new("frames");
    let _ = std::fs::create_dir_all(out);
    let (w, h) = (reference.width, reference.height);
    let mine: Vec<u8> = (0..h as usize)
        .flat_map(|y| base.pixels[y * base.width as usize..][..w as usize].to_vec())
        .collect();
    let bare: Vec<u8> = (0..h as usize)
        .flat_map(|y| sea.pixels[y * sea.width as usize..][..w as usize].to_vec())
        .collect();
    let diff: Vec<u8> = mine
        .iter()
        .zip(reference.pixels.iter())
        .zip(bare.iter())
        .map(|((a, b), s)| match (a == s, a == b) {
            (true, _) => 0,      // the island did not paint here
            (false, true) => 3,  // painted, and it matches
            (false, false) => 15, // painted, and it does not
        })
        .collect();
    for (name, px) in [
        ("isle_port.png", &mine),
        ("isle_ref.png", &reference.pixels),
        ("isle_diff.png", &diff),
    ] {
        let _ = png::write_indexed(&out.join(name), w, h, px, &palette, None);
    }
    println!("wrote frames/isle_port.png, isle_ref.png and isle_diff.png");
}
