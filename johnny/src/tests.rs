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
    let s = settings::Settings {
        start_time: 1830,
        sounds: false,
        story_offered: 4,
        story_reached: 7,
        story_date: (1992, 12, 25),
        speed: 85,
        ..Default::default()
    };
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
    let s = settings::Settings {
        story_offered: 5,
        story_reached: 4,
        story_date: (1992, 6, 1),
        ..Default::default()
    };
    let mut d = director::Director::new(0);
    d.configure(s.clone());
    assert_eq!(d.day, 4, "the reached day");
    let (offered, reached, _) = d.story_snapshot();
    assert_eq!((offered, reached), (5, 4), "a round trip must not move it on");
}

#[test]
fn the_calendar_gates_the_story() {
    // The reached day catches up to the offer only when a real day has gone
    // by (`seg4:0x0BC9` compares the stored date with today's). Stored as
    // today, nothing moves; stored long ago, it moves by exactly one.
    let today = director::civil_from_days(director::today());
    for (date, want) in [(today, 5u8), ((2020, 1, 1), 6)] {
        let s = settings::Settings {
            story_offered: 6,
            story_reached: 5,
            story_date: date,
            ..Default::default()
        };
        let mut d = director::Director::new(0);
        d.configure(s);
        d.settle_story_for_test();
        assert_eq!(d.day, want, "stored {date:?}");
    }
}

#[test]
fn a_fresh_settings_file_does_not_start_the_story_mid_way() {
    let mut d = director::Director::new(0);
    d.configure(settings::Settings::default());
    assert_eq!(d.day, 1);
}

#[test]
fn a_broken_png_is_refused_rather_than_fatal() {
    // The reader is handed whatever is in an art pack, and a screensaver that
    // panics on a truncated file is worse than one that draws the original.
    // Every prefix of a real PNG, every single-byte corruption of its header,
    // and a spread of nonsense.
    let mut palette = [[0u8; 3]; 256];
    for (i, slot) in palette.iter_mut().enumerate() {
        *slot = [i as u8, 255 - i as u8, 0];
    }
    let dir = std::env::temp_dir().join("johnny-png-fuzz");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("seed.png");
    let px: Vec<u8> = (0..24 * 9).map(|i| (i % 251) as u8).collect();
    png::write_indexed(&path, 24, 9, &px, &palette, Some(0)).expect("write");
    let good = std::fs::read(&path).expect("read");
    let _ = std::fs::remove_file(&path);

    for n in 0..good.len() {
        let _ = png::read(&good[..n]);
    }
    for at in 0..good.len().min(120) {
        let mut bad = good.clone();
        bad[at] ^= 0xFF;
        let _ = png::read(&bad);
    }
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    for _ in 0..300 {
        let mut noise = good.clone();
        for _ in 0..8 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let at = (seed as usize) % noise.len();
            noise[at] = (seed >> 32) as u8;
        }
        let _ = png::read(&noise);
    }
    // And the degenerate cases, which are the ones a real tool produces.
    for junk in [
        &b""[..],
        &b"\x89PNG\r\n\x1a\n"[..],
        &b"\x89PNG\r\n\x1a\nIHDR"[..],
    ] {
        assert!(png::read(junk).is_err());
    }
}

#[test]
fn a_string_operand_always_lands_on_a_word() {
    // `seg11:0x04a8` counts the bytes before the NUL, steps over the NUL,
    // and steps one further when that count came out even. Both lengths and
    // both parities, so the rule is pinned rather than the one case the data
    // happens to hold.
    for text in ["A", "AB", "ABC", "ABCD", ""] {
        let mut bytes = 0xF02Fu16.to_le_bytes().to_vec();
        bytes.extend_from_slice(text.as_bytes());
        bytes.push(0);
        let consumed = 2 + text.len() + 1 + usize::from(text.len() % 2 == 0);
        bytes.resize(consumed + 2, 0);
        // A second instruction, so `decode` has to land on it exactly.
        bytes[consumed..consumed + 2].copy_from_slice(&0x0FF0u16.to_le_bytes());
        let (ops, used) = ttm::decode(&bytes);
        assert_eq!(used, bytes.len(), "{text:?} left {} bytes", bytes.len() - used);
        assert_eq!(ops.len(), 2, "{text:?} did not decode as two instructions");
        assert!(matches!(ops[1], ttm::Op::EndFrame), "{text:?} misread the second instruction");
    }
}

#[test]
fn a_scene_that_cannot_run_ends_rather_than_hanging() {
    // `seg11:0x074A` refuses a scene index the TTM's table has not got, and
    // `seg10:0x1DBA` puts that instance straight into state 4 -- so the
    // block waiting on it fires on the next pass. Dropping the play instead,
    // which is the obvious thing to do, leaves that block waiting for ever.
    let program = std::rc::Rc::new(ttm::Program::empty());
    let mut inst = ttm::Instance::new(program);
    assert!(!inst.seek_scene(7), "an empty program has no scenes");
    inst.exhaust();
    let mut stage = ttm::Stage::new();
    let archive = dgds::Archive::load(&[], None).expect("an empty archive");
    assert_eq!(inst.run_frame(&mut stage, &archive), ttm::Step::Finished);

    // And a count or a deadline does not resurrect it: `seg10:0x1DBA` is on
    // the other side of the branch from `seg10:0x1D21`'s restarts.
    for repeat in [ads::Repeat::Once, ads::Repeat::Times(4), ads::Repeat::ForTicks(600)] {
        let mut inst = ttm::Instance::new(std::rc::Rc::new(ttm::Program::empty()));
        inst.exhaust();
        let mut a = director::Active::start(1, 7, inst, repeat, 0);
        a.cannot_run();
        assert_eq!(a.state, ads::SceneState::Over, "{repeat:?} kept a dead scene alive");
    }
}

#[test]
fn the_instance_list_is_marker_order_not_play_order() {
    // `seg11:0x0645` makes one instance record per `1111` marker as a TTM
    // loads, appended to a single list, and `seg10:0x1C35` walks that list
    // head to tail -- so the tail lands on top and a scene's depth is fixed
    // by where its marker sits in the file. `seg10:0x0DB6` re-arms the
    // record it finds rather than making a new one, so replaying a scene
    // does not lift it.
    //
    // `MJAMBWLK.TTM` is why this matters: scene 42 is the first marker in
    // the file and scenes 33 to 61 are the forty-fourth onward, but 42 is
    // played last, by the sequence `F200` wakes. Appending on play put the
    // standing backdrop on top of Johnny.
    let order = [(1u16, 42u16), (1, 33), (1, 61), (2, 5)];
    assert_eq!(director::depth(&order, 1, 42), 0);
    assert_eq!(director::depth(&order, 1, 61), 2);
    assert_eq!(director::depth(&order, 2, 5), 3);
    // A scene with no record at all sorts last, which is the only place the
    // engine cannot put it: `seg10:0x0DB6` never plays one.
    assert_eq!(director::depth(&order, 9, 9), order.len());
}

#[test]
fn only_the_playable_marker_makes_a_record() {
    // Both `1101` and `1111` name a frame -- `seg12:0x13AA` accepts either
    // when it resolves a goto -- but only `1111` gets an instance record,
    // and without one `seg10:0x0DB6` does nothing at all.
    let mut code = Vec::new();
    for (op, scene) in [(0x1101u16, 5u16), (0x0FF0, 0), (0x1111, 7), (0x0FF0, 0)] {
        code.extend_from_slice(&op.to_le_bytes());
        if op & 0x0F != 0 {
            code.extend_from_slice(&scene.to_le_bytes());
        }
    }
    let (ops, used) = ttm::decode(&code);
    assert_eq!(used, code.len());
    let playable: Vec<u16> = ops
        .iter()
        .filter_map(|op| match op {
            ttm::Op::SceneMarker { scene, playable: true } => Some(*scene),
            _ => None,
        })
        .collect();
    assert_eq!(playable, vec![7], "1101 must not make a record");
    // Both are still findable, because a goto can name either.
    let program = ttm::Program { name: "T.TTM".into(), ops, scenes: playable };
    assert!(program.scene_start(5).is_some(), "1101 still names a frame");
    assert!(program.scene_start(7).is_some());
}

#[test]
fn the_clip_and_the_colours_belong_to_the_instance() {
    // `seg12:0x0F40` and `seg12:0x06A9` write `4004`'s rectangle and `2002`'s
    // two colours into the instance record as well as into the globals, and
    // `seg10:0x1C81` copies them back out of the record before every frame
    // that instance runs. So a scene that sets a clip does not hand it to the
    // next scene to draw -- which is what left the raft in `SJLEAVES.TTM`
    // clipped down to a few pixels.
    let archive = dgds::Archive::load(&[], None).expect("an empty archive");
    let mut stage = ttm::Stage::new();

    let setter = std::rc::Rc::new(ttm::Program {
        name: "SETTER.TTM".into(),
        ops: vec![
            ttm::Op::SceneMarker { scene: 0, playable: true },
            ttm::Op::SetClip { x1: 10, y1: 20, x2: 30, y2: 40 },
            ttm::Op::SetColors { fg: 3, bg: 4 },
            ttm::Op::EndFrame,
        ],
        scenes: vec![0],
    });
    let plain = std::rc::Rc::new(ttm::Program {
        name: "PLAIN.TTM".into(),
        ops: vec![ttm::Op::EndFrame],
        scenes: Vec::new(),
    });

    let mut a = ttm::Instance::new(std::rc::Rc::clone(&setter));
    let mut b = ttm::Instance::new(std::rc::Rc::clone(&plain));

    assert!(matches!(a.run_frame(&mut stage, &archive), ttm::Step::Frame(_)));
    assert!(stage.surface.clip.is_some(), "4004 sets the screen's clip");
    assert_eq!((stage.fg, stage.bg), (3, 4));

    // The next instance to draw brings its own, and its own is the whole
    // screen and colour 15 (`seg10:0x0B6E`).
    assert!(matches!(b.run_frame(&mut stage, &archive), ttm::Step::Frame(_)));
    assert!(stage.surface.clip.is_none(), "the clip followed the wrong scene");
    assert_eq!((stage.fg, stage.bg), (15, 15), "the colours followed the wrong scene");

    // And the one that set them still has them next time round.
    assert!(a.seek_scene(0));
    assert!(matches!(a.run_frame(&mut stage, &archive), ttm::Step::Frame(_)));
    assert!(stage.surface.clip.is_some(), "the instance lost its own clip");
}

#[test]
fn a_region_that_runs_off_the_screen_is_not_drawn_at_all() {
    // `seg12:0x0318` refuses the whole `A0x4` family a rectangle that is
    // empty or whose far edge is past the screen -- it does not clip it to
    // fit. Nothing in the shipped scripts reaches it, but the difference
    // between "clipped" and "not drawn" is a real one.
    let archive = dgds::Archive::load(&[], None).expect("an empty archive");
    let mut stage = ttm::Stage::new();
    let w = stage.surface.width as i16;
    let cases = [
        (0i16, 0i16, w, 4i16, true),
        (1, 0, w, 4, false),   // one past the right edge
        (0, 0, 0, 4, false),   // empty
        (0, 0, 4, 0, false),
    ];
    for (x, y, rw, rh, want) in cases {
        let program = std::rc::Rc::new(ttm::Program {
            name: "REGION.TTM".into(),
            ops: vec![
                ttm::Op::RegionOp { opcode: 0xA064, x, y, w: rw, h: rh },
                ttm::Op::EndFrame,
            ],
            scenes: Vec::new(),
        });
        let mut inst = ttm::Instance::new(program);
        let _ = inst.run_frame(&mut stage, &archive);
        assert_eq!(
            inst.marks() > 0,
            want,
            "A064 at {x},{y} {rw}x{rh} on a {w}-wide screen"
        );
    }
}

#[test]
fn a_pass_with_nothing_to_run_runs_nothing() {
    // `seg10:0x1C0B` hands the interpreter an explicit pointer and only when
    // the sequence is in state 1, so a pass in which no sequence wants to run
    // interprets nothing at all. Here the pointer is a field, and a field
    // keeps its value: once the last sequence stopped itself, every later
    // pass carried on from where that one left off -- the instruction after
    // its `F010`, then the `FFFF`, then the *next sequence's header*, and
    // then that sequence's body, which plays its own scenes.
    //
    // That is a second animation starting on its own a few frames after the
    // first one ends, with no rule anywhere asking for it.
    let mut code: Vec<u8> = Vec::new();
    let mut w = |v: u16| code.extend_from_slice(&v.to_le_bytes());
    w(0x0001); // sequence 1
    w(0xF010);
    w(0xFFFF); // stop this sequence
    w(0x1510);
    w(0xFFFF);
    w(0x0002); // sequence 2, which nothing starts
    w(0x2005);
    w(1);
    w(7);
    w(0);
    w(1);
    w(0x1510);
    w(0xFFFF);

    let mut s = ads::Sequencer::from_script(&code);
    s.tracing = true;
    assert!(s.start(1));
    let mut rng = ads::Rng::new(0);
    for pass in 0..8 {
        s.trace.clear();
        s.begin_pass();
        for _ in 0..1000 {
            if matches!(s.next_action(&mut rng), ads::Action::PassOver) {
                break;
            }
        }
        assert!(
            s.trace.is_empty(),
            "pass {pass} ran {:?} with no sequence in state 1",
            s.trace
        );
    }
    assert_eq!(s.raw_state(1, 7), 0, "sequence 2 played on its own");
}

#[test]
fn two_files_do_not_share_bitmap_slots() {
    // `seg12:0x0773` reads a bitmap slot out of the *loaded TTM* record at
    // `ds:0x2638 + 0x0C + slot*2`, and `seg10:0x1C71` selects that record
    // from the running instance's own file id before every frame. So the
    // slots are per file, not global: two scenes of one file share them, and
    // two files on screen at once cannot tread on each other's however many
    // of them are live and whatever numbers they pick.
    //
    // The port had one set of sixteen for everything, which the `ttm` oracle
    // caught: `GJVIS5.TTM` draws from a slot it never loads, and with shared
    // slots the port drew whatever the file before it had left there.
    let archive = dgds::Archive::load(&[], None).expect("an empty archive");
    let mut stage = ttm::Stage::new();
    let one = std::rc::Rc::new(ttm::Program {
        name: "ONE.TTM".into(),
        ops: vec![
            ttm::Op::SelectBitmapSlot(3),
            ttm::Op::FreeShapes,
            ttm::Op::EndFrame,
        ],
        scenes: Vec::new(),
    });
    let two = std::rc::Rc::new(ttm::Program {
        name: "TWO.TTM".into(),
        ops: vec![
            ttm::Op::SelectBitmapSlot(3),
            ttm::Op::FreeShapes,
            ttm::Op::EndFrame,
        ],
        scenes: Vec::new(),
    });
    stage.trace = Some(Vec::new());
    for p in [&one, &two] {
        let mut inst = ttm::Instance::new(std::rc::Rc::clone(p));
        let _ = inst.run_frame(&mut stage, &archive);
    }
    // Two files, two slot threes. The trace names the slot each frame
    // touched; what matters is that asking for one made a second set rather
    // than finding the first.
    assert_eq!(stage.slot_sets(), 2, "each file gets its own sixteen");
}

#[test]
fn the_ellipse_opcode_draws_a_circle_the_height_decides() {
    // `seg12:0x01C9` turns `A404`'s four operands into three before anything
    // is drawn: a radius of `h >> 1` and a centre at `(x + r, y + r)`. The
    // width operand is read by nobody. All 437 in `RESOURCE.001` are square,
    // so this has never shown -- but a port that inscribed an ellipse in the
    // rectangle would put the far edge one pixel out even on those.
    let archive = dgds::Archive::load(&[], None).expect("an empty archive");
    let mut stage = ttm::Stage::new();
    stage.trace = Some(Vec::new());
    let program = std::rc::Rc::new(ttm::Program {
        name: "E.TTM".into(),
        ops: vec![
            ttm::Op::DrawEllipse { x: 100, y: 200, w: 999, h: 21, filled: true },
            ttm::Op::EndFrame,
        ],
        scenes: Vec::new(),
    });
    let mut inst = ttm::Instance::new(program);
    let _ = inst.run_frame(&mut stage, &archive);
    assert_eq!(
        stage.trace.as_deref().unwrap_or_default(),
        ["circle 10 110 210 15 1"],
        "radius h>>1, centre offset by it, width ignored"
    );
}


#[test]
fn a_comment_needs_whitespace_in_front_of_it() {
    // Two things pull against each other here. The file this port writes
    // puts comments after values, so `StartTime=900  ; ...` has to lose the
    // tail -- but a path may contain `#` or `;`, and cutting at every one of
    // them sends the loader somewhere else entirely. The line is drawn where
    // every other INI reader draws it: a marker starts a comment when it
    // starts the line or follows a space, and is part of the value otherwise.
    let dir = std::env::temp_dir().join("johnny_ini_comment_test");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("johnny.ini");
    std::fs::write(
        &path,
        "[ScreenSaver.ScreenAntics]\n\
         ; a whole-line comment\n\
         SourceDir=/games/Johnny#1;odd\n\
         StartTime=1030    ; when his day begins\n\
         Speed=75\n",
    )
    .unwrap();
    let s = crate::settings::Settings::load(&path);
    assert_eq!(s.source_dir, "/games/Johnny#1;odd", "no space, so not a comment");
    assert_eq!(s.start_time, 1030, "space then `;`, so a comment");
    assert_eq!(s.speed, 75);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_sprite_clipped_away_does_not_tint_the_surface() {
    // `tinted` decides whether a composed frame keeps its own colours, and a
    // frame that does is no longer recoloured by a later palette change. So
    // it has to mean "something non-palette was drawn", not "something was
    // attempted" -- otherwise an art-pack sprite that missed the clip
    // rectangle entirely costs the whole composition its palette art.
    use crate::dgds::Frame;
    use crate::render::{Clip, Mirror, Surface};
    let mut s = Surface::new(64, 64);
    let sprite = Frame {
        width: 4,
        height: 4,
        pixels: vec![1; 16],
        colour: Some(vec![0xFF00_FF00; 16]),
    };
    s.clip = Some(Clip { x1: 0, y1: 0, x2: 8, y2: 8 });
    s.draw_sprite(&sprite, 40, 40, Mirror::none());
    assert!(!s.tinted, "nothing landed inside the clip");
    s.draw_sprite(&sprite, 2, 2, Mirror::none());
    assert!(s.tinted, "this one did");
}
