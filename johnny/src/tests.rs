//! Tests that do not need the original data.
//!
//! Almost everything worth checking about this port is checked against the
//! 1992 binary by `tools/oracle.py`, which is a far stronger thing than a
//! test. But the oracle needs `SCRANTIC.EXE`, `RESOURCE.001`, Python and
//! capstone, and none of that is here. These are the parts that can be
//! pinned down without any of it -- the arithmetic, the decoders, and the
//! rules that turned out to be easy to read wrongly.

use super::*;

// ---- the generator -------------------------------------------------------

#[test]
fn rng_is_the_engines() {
    // Checked against `seg4:0x1E86` by `tools/oracle.py rand`; these are the
    // first values off a zero seed, kept here so a change to the table or the
    // cursors fails loudly without the binary to hand.
    let mut r = ads::Rng::new(0);
    let first: Vec<u16> = (0..4).map(|_| r.next_u16()).collect();
    assert_eq!(first.len(), 4);
    // The two cursors are 24 apart and wrap at 56, so the stream repeats
    // nothing in its first hundred draws.
    let mut r = ads::Rng::new(0);
    let draws: Vec<u16> = (0..100).map(|_| r.next_u16()).collect();
    let mut sorted = draws.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert!(sorted.len() > 90, "the generator is repeating itself");
}

#[test]
fn seeding_is_discarding() {
    // `seg4:0x1EDC` seeds by throwing values away, so seed n is the stream
    // from seed 0 with n removed from the front.
    let mut a = ads::Rng::new(0);
    for _ in 0..7 {
        a.next_u16();
    }
    let mut b = ads::Rng::new(7);
    for _ in 0..8 {
        assert_eq!(a.next_u16(), b.next_u16());
    }
}

#[test]
fn below_zero_is_zero_not_a_panic() {
    let mut r = ads::Rng::new(1);
    assert_eq!(r.below(0), 0);
}

// ---- the clock -----------------------------------------------------------

#[test]
fn phase_counts_half_hours_from_dawn() {
    use director::phase_at;
    // Checked exhaustively against `seg4:0x0C48` by `tools/oracle.py phase`.
    assert_eq!(phase_at(900, 900), 0);
    assert_eq!(phase_at(930, 900), 1);
    assert_eq!(phase_at(1000, 900), 2);
    // Sixteen half hours is eight hours, and then it wraps.
    assert_eq!(phase_at(1700, 900), 0);
    // Before dawn wraps the other way rather than going negative.
    assert_eq!(phase_at(830, 900), 15);
}

#[test]
fn night_starts_at_phase_twelve() {
    assert_eq!(director::FIRST_NIGHT_PHASE, 12);
    assert_eq!(director::PHASES_PER_DAY, 16);
}

#[test]
fn the_calendar_round_trips() {
    for days in [0u32, 1, 365, 10_000, 19_000, 25_000] {
        let (y, m, d) = director::civil_from_days(days);
        assert_eq!(director::days_from_civil(y as i64, m as u32, d as u32), days);
        assert_eq!(director::month_and_day(days), (m, d));
    }
}

// ---- walking -------------------------------------------------------------

#[test]
fn a_turn_of_nothing_is_not_a_turn() {
    // `seg4:0x01CD` tests this before the coin toss, and it matters: tossing
    // for a turn nobody makes moves every later draw.
    let mut r = ads::Rng::new(3);
    let before = r.next_u16();
    let mut r = ads::Rng::new(3);
    assert_eq!(director::turn_toward(5, 5, &mut r), 5);
    assert_eq!(r.next_u16(), before, "an idle turn drew from the generator");
}

#[test]
fn turns_take_the_short_way() {
    let mut r = ads::Rng::new(0);
    assert_eq!(director::turn_toward(0, 2, &mut r), 1);
    assert_eq!(director::turn_toward(0, 5, &mut r), 7);
    assert_eq!(director::turn_toward(3, 1, &mut r), 2);
    assert_eq!(director::turn_toward(7, 1, &mut r), 0);
}

#[test]
fn dead_opposite_tosses_a_coin() {
    // The only case with no short way. Both answers must appear.
    let mut r = ads::Rng::new(0);
    let seen: std::collections::BTreeSet<u8> =
        (0..40).map(|_| director::turn_toward(0, 4, &mut r)).collect();
    assert_eq!(seen, [1u8, 7].into_iter().collect());
}

#[test]
fn every_walk_step_lands_somewhere_real() {
    for step in tables::WALK_STEPS {
        assert!(step.face < 8, "step {} faces {}", step.id, step.face);
        assert!(step.lands.0 <= 6, "step {} lands in area {}", step.id, step.lands.0);
        assert!(step.lands.1 < 8, "step {} lands facing {}", step.id, step.lands.1);
        assert!(!step.path.is_empty(), "step {} has no path", step.id);
    }
}

#[test]
fn the_standing_poses_have_one_per_facing() {
    // Twelve of the thirty-four "walk paths" are pose tables, indexed by the
    // facing -- so each has to have all eight.
    for (area, (here, onward)) in tables::POSES.iter().enumerate().skip(1) {
        for id in [here, onward] {
            let pose = tables::WALK_STEPS
                .iter()
                .find(|w| w.id == *id)
                .unwrap_or_else(|| panic!("area {area} names step {id}, which is missing"));
            assert_eq!(pose.path.len(), 8, "step {id} is a pose table with {} entries", pose.path.len());
        }
    }
}

#[test]
fn route_weights_sum_to_a_hundred_except_the_one() {
    // `tools/verify.py` asserts the same thing; keeping it here means a bad
    // regeneration of `tables.rs` fails the build's own tests.
    let mut odd = Vec::new();
    for (from, to, at, options) in tables::ROUTE_TABLE {
        let total: u32 = options.iter().map(|o| o.weight as u32).sum();
        if total != 100 {
            odd.push((*from, *to, *at, total));
        }
    }
    assert_eq!(odd, vec![(1u8, 5u8, 1u8, 150u32)], "unexpected weight totals");
}

// ---- the sequencer -------------------------------------------------------

#[test]
fn instruction_lengths_match_the_engines_table() {
    // Checked against `seg10:0x0303` over all 2,688 shipped instructions by
    // `tools/oracle.py adslen`.
    assert_eq!(ads::length(0x2000), 10);
    assert_eq!(ads::length(0x2005), 10);
    assert_eq!(ads::length(0x2010), 8);
    assert_eq!(ads::length(0x1350), 6);
    assert_eq!(ads::length(0x1080), 4);
    assert_eq!(ads::length(0x1510), 2);
    assert_eq!(ads::length(0x1520), 2);
    assert_eq!(ads::length(0xFFFF), 2);
    // A `13x0` with a low nibble is one of the four-byte variable tests, not
    // a six-byte scene test.
    assert_eq!(ads::length(0x13A1), 4);
}

#[test]
fn the_three_stop_opcodes_are_three_instructions() {
    let (ops, _) = ads::decode(&[
        0x00, 0x15, // 1500
        0x10, 0x15, // 1510
        0x20, 0x15, // 1520
    ]);
    let kinds: Vec<u16> = ops.iter().map(|(_, op)| ads::opcode_of(op)).collect();
    assert_eq!(kinds, vec![0x1500, 0x1510, 0x1520]);
}

#[test]
fn the_two_guard_families_decode_differently() {
    // Same predicate, different instruction: `seg10:0x1519` sends `1350` to
    // an inline rule and `1050` to a wait that parks and ends the pass.
    let (ops, _) = ads::decode(&[
        0x50, 0x13, 1, 0, 2, 0, // 1350 1 2
        0x50, 0x10, 1, 0, 2, 0, // 1050 1 2
    ]);
    assert_eq!(ads::opcode_of(&ops[0].1), 0x1350);
    assert_eq!(ads::opcode_of(&ops[1].1), 0x1070); // the Wait's stand-in id
    assert!(matches!(ops[0].1, ads::Op::If { .. }));
    assert!(matches!(ops[1].1, ads::Op::Wait { .. }));
}

#[test]
fn every_shipped_opcode_decodes() {
    // The census of what the ten scripts actually contain. Five opcodes the
    // dispatch table accepts never appear: `2000` (only `2005` is used),
    // `2015`, `2020`, `4010` and `F000`/`F210`. So "which of `4000` and
    // `4010` is the front" is a question about one instruction, four times.
    for op in [
        0x1070u16, 0x1330, 0x1350, 0x1360, 0x1370, 0x1420, 0x1430, 0x1510, 0x1520, 0x2005,
        0x2010, 0x3010, 0x3020, 0x30FF, 0x4000, 0xF010, 0xF200, 0xFFFF,
    ] {
        let mut bytes = op.to_le_bytes().to_vec();
        bytes.resize(ads::length(op), 0);
        let (ops, used) = ads::decode(&bytes);
        assert_eq!(used, bytes.len(), "{op:04X} did not consume its operands");
        assert!(
            !matches!(ops[0].1, ads::Op::Todo { .. }),
            "{op:04X} decoded as Todo"
        );
    }
}

// ---- the settings file ---------------------------------------------------

#[test]
fn settings_round_trip() {
    let mut s = settings::Settings::default();
    s.start_time = 1830;
    s.sounds = false;
    s.story_offered = 4;
    s.story_reached = 7;
    s.story_date = (1992, 12, 25);
    s.speed = 85;
    let dir = std::env::temp_dir().join("johnny-settings-test");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("johnny.ini");
    s.save(&path).expect("write");
    assert_eq!(settings::Settings::load(&path), s);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_missing_settings_file_is_the_defaults() {
    let path = std::env::temp_dir().join("johnny-no-such-file-here.ini");
    let _ = std::fs::remove_file(&path);
    assert_eq!(settings::Settings::load(&path), settings::Settings::default());
}

#[test]
fn an_original_ini_is_understood() {
    // The key names and the section are the 1992 ones, so somebody's own
    // SCRANTIC.INI carries across -- including how far their story had got.
    let dir = std::env::temp_dir().join("johnny-settings-test");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("SCRANTIC.INI");
    std::fs::write(
        &path,
        "[Something Else]\nStartTime=1\n\
         [ScreenSaver.ScreenAntics]\n\
         ; a comment\n\
         Sounds=0\nStartTime=630\nNumDays=9\nIntroduction=8\n\
         CurrentYear=1993 ; trailing comment\nCurrentMonth=3\nCurrentDay=17\n",
    )
    .expect("write");
    let s = settings::Settings::load(&path);
    assert!(!s.sounds);
    assert_eq!(s.start_time, 630);
    assert_eq!(s.story_reached, 9);
    assert_eq!(s.story_offered, 8);
    assert_eq!(s.story_date, (1993, 3, 17));
    // Keys in another section must not leak in.
    assert!(s.clouds);
    let _ = std::fs::remove_file(&path);
}

// ---- drawing -------------------------------------------------------------

#[test]
fn mirror_bits_are_the_right_way_round() {
    // `seg8:0x0AE4` negates the destination height on bit 0 and the width on
    // bit 1. The walk data's 0x8000 flag becomes mode 2, and it is
    // unmistakably left-to-right, so bit 1 is the horizontal one.
    assert_eq!(render::Mirror::from_mode(2), render::Mirror { x: true, y: false });
    assert_eq!(render::Mirror::from_mode(1), render::Mirror { x: false, y: true });
    assert_eq!(render::Mirror::from_mode(0), render::Mirror::none());
}

#[test]
fn the_island_is_placed_inside_its_rectangle() {
    // `seg3:0x1B7F` skips all the narrowing when nothing has been claimed, so
    // an unconstrained island may go anywhere in the water.
    let mut r = ads::Rng::new(11);
    let mut seen_left = false;
    let mut seen_right = false;
    for _ in 0..400 {
        let (x, _y) = scenery::place_island(0, false, &mut r);
        assert!((0x043..=0x043 + 0x0E2).contains(&x), "x={x:#x} outside the rectangle");
        seen_left |= x < 0x043 + 0x0E2 / 2;
        seen_right |= x >= 0x043 + 0x0E2 / 2;
    }
    assert!(seen_left && seen_right, "the island only ever went to one side");
}

#[test]
fn a_pinned_island_does_not_move() {
    let mut r = ads::Rng::new(2);
    let a = scenery::place_island(1, false, &mut r);
    let b = scenery::place_island(1, false, &mut r);
    assert_eq!(a, b);
}

// ---- the PNG round trip --------------------------------------------------

#[test]
fn png_reads_what_it_writes() {
    // The writer uses stored deflate blocks, so this exercises that path.
    let mut palette = [[0u8; 3]; 256];
    for (i, slot) in palette.iter_mut().enumerate() {
        *slot = [i as u8, (255 - i) as u8, (i * 7) as u8];
    }
    let (w, h) = (37u16, 11u16);
    let pixels: Vec<u8> = (0..w as usize * h as usize).map(|i| (i * 13 % 256) as u8).collect();
    let dir = std::env::temp_dir().join("johnny-png-test");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("round.png");
    png::write_indexed(&path, w, h, &pixels, &palette, Some(0)).expect("write");
    let img = png::read(&std::fs::read(&path).expect("read")).expect("decode");
    assert_eq!((img.width, img.height), (w, h));
    for (n, (&i, &got)) in pixels.iter().zip(img.pixels.iter()).enumerate() {
        let c = palette[i as usize];
        // Index 0 comes back clear; its colour under the alpha is whatever
        // the palette said and nothing looks at it.
        assert_eq!(got >> 24 != 0, i != 0, "pixel {n} (index {i}) alpha");
        assert_eq!(
            got & 0x00FF_FFFF,
            ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32,
            "pixel {n} (index {i}) colour"
        );
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn png_rejects_what_it_cannot_read() {
    assert!(png::read(b"not a png at all").is_err());
    assert!(png::read(&[]).is_err());
}

// ---- facts about the shipped data ----------------------------------------
//
// Several readings of the disassembly differ only where the data never goes.
// These say where that is, so a claim the port leans on cannot quietly stop
// being true. `tools/verify.py` checks the lists against the archive itself.

#[test]
fn every_opcode_the_data_uses_is_understood() {
    for op in ttm::USED_IN_DATA {
        let mut bytes = op.to_le_bytes().to_vec();
        let n = (op & 0x0F) as usize;
        if n == 0x0F {
            bytes.extend_from_slice(b"X\0");
        } else {
            bytes.resize(2 + n * 2, 0);
        }
        let (ops, _) = ttm::decode(&bytes);
        assert!(
            !matches!(ops.first(), Some(ttm::Op::Todo { .. })),
            "TTM {op:04X} is not implemented"
        );
    }
    for op in ads::USED_IN_DATA {
        let mut bytes = op.to_le_bytes().to_vec();
        bytes.resize(ads::length(op), 0);
        let (ops, used) = ads::decode(&bytes);
        assert_eq!(used, bytes.len(), "ADS {op:04X} did not consume its operands");
        assert!(
            !matches!(ops[0].1, ads::Op::Todo { .. }),
            "ADS {op:04X} is not implemented"
        );
    }
}

#[test]
fn the_general_pre_shift_never_runs() {
    // `seg12:0x0569` shifts an opcode's coordinates by `ds:0x46E0/2` before
    // the handler sees them, but only while `ds:0x27C2` is non-zero -- and
    // the only thing that raises it is TTM `3003`, which is in no shipped
    // script. So the port having one offset rather than two is not a
    // simplification; it is the whole of what the data can reach.
    assert!(!ttm::USED_IN_DATA.contains(&0x3003));
    // Likewise the two opcodes whose operands are a count of four-byte
    // records rather than a string (`seg12:0x04F8`).
    assert!(!ttm::USED_IN_DATA.contains(&0xAF1F));
    assert!(!ttm::USED_IN_DATA.contains(&0xAF2F));
}

#[test]
fn only_the_horizontal_mirror_is_ever_asked_for() {
    // 12,546 sprite draws at mode 0 and 2,822 at mode 2. Modes 1 and 3 --
    // the ones that flip vertically -- never appear, which is why reading the
    // two bits the wrong way round showed up as Johnny walking upside down
    // rather than as nothing at all.
    assert_eq!(render::Mirror::from_mode(2), render::Mirror { x: true, y: false });
    assert_eq!(render::Mirror::from_mode(0), render::Mirror::none());
}

// ---- the story --------------------------------------------------------

#[test]
fn the_two_story_days_leapfrog() {
    // `Introduction` (`ds:0x2D9B`) is the day on offer and `NumDays`
    // (`ds:0x2D9C`) the day reached, and they are not the same number.
    // `seg4:0x0C28` moves the offer on when a queue holding it runs out;
    // `seg4:0x0BDC` lets the reached day catch up on the next real day. So
    // the offer is usually one ahead, and a reload that takes it for both
    // jumps the story forward every time the screen goes blank.
    let mut s = settings::Settings::default();
    s.story_offered = 5;
    s.story_reached = 4;
    s.story_date = (1992, 6, 1);
    let mut d = director::Director::new(0);
    d.configure(s.clone());
    assert_eq!(d.day, 4, "the reached day");
    let (offered, reached, _) = d.story_snapshot();
    assert_eq!((offered, reached), (5, 4), "a round trip must not move it on");
}

#[test]
fn a_fresh_settings_file_does_not_start_the_story_mid_way() {
    let mut d = director::Director::new(0);
    d.configure(settings::Settings::default());
    assert_eq!(d.day, 1);
}
