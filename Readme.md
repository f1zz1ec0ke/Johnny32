# Johnny32

Reverse engineering the 1992 Dynamix/Sierra screensaver **Johnny Castaway**
(internally *Screen Antics* / `SCRANTIC`) with the goal of a native 64-bit
version — no DOSBox, no NTVDM, no 16-bit anything.

## Where things stand

`johnnycastaway.exe` in this repo, as downloaded from screensaverplanet.com,
turned out to be an NSIS installer wrapping a UPX-packed Delphi launcher
wrapping **the untouched 1992 16-bit binary**. The Delphi layer only drops the
original files and `ShellExecute`s them, which is exactly why it dies on
64-bit Windows. Nothing in that download was ever ported.

The upside is that the original data comes out intact, and it is now fully
decoded:

* all three unwrapping layers, automated end to end
* the `RESOURCE.001` container format
* both codecs, ported from the original's disassembly — all 180 compressed
  chunks decode to exactly their declared size
* 4bpp images, palette and transparency — 10 backgrounds and 2,392 sprite
  frames export to PNG

* the TTM animation bytecode — all 41 scripts disassemble end to end against
  the opcode tables read out of the interpreter
* the ADS sequencer — all 10 scripts consume exactly, including the weighted
  random-choice blocks that drive the screensaver's variety

* the scene scheduler — the 80-entry scene table, the weighted-random picker,
  the sixteen-phase day, the holiday calendar, and the eleven-day story arc

* the walking system — the route matrix, the weighted route blocks and the 34
  hand-authored walk paths in real screen coordinates

Still open: what the position pairs mean geometrically, and the cadence of the
outer scene loop.

Details in [docs/ANALYSIS.md](docs/ANALYSIS.md),
[docs/FILE_FORMATS.md](docs/FILE_FORMATS.md),
[docs/SCHEDULER.md](docs/SCHEDULER.md) and
[docs/PRIOR_ART.md](docs/PRIOR_ART.md).

## Usage

Recover the 1992 originals (needs `7z` and `upx` on PATH):

```sh
python3 tools/extract_original.py
# -> original/scrantic.exe, original/RESOURCE.001, original/RESOURCE.MAP
```

Check the decoder against every record and chunk:

```sh
python3 tools/verify.py
```

Export every asset to PNG and dump the scripts:

```sh
python3 tools/export_assets.py --sheets
# -> assets/backgrounds, assets/sprites, assets/sheets, assets/scripts
```

Dump the scheduler tables:

```sh
python3 tools/scheduler.py          # ADS files, scenes, story arc, holidays
python3 tools/scheduler.py --walk   # route matrix and the 34 walk paths
```

Inspect the 16-bit executable:

```sh
python3 tools/ne_info.py original/scrantic.exe
python3 tools/ne_info.py original/scrantic.exe --disasm 6 --start 0xfc1 --end 0x10d0
```

## The port

`johnny/` is a Rust reimplementation, written against the disassembly rather
than against the running screensaver. It reads a copy of `RESOURCE.001` you
supply; no Sierra data is redistributed here.

The decoder is differential-tested against the Python reference — both emit the
same canonical digest of every chunk, sprite frame, background and palette:

```sh
tools/difftest.sh
```

That harness paid for itself immediately: on its first run the Rust reader
refused a chunk the Python one had been inventing out of `FILES.VIN`, which is
plain text whose first four bytes happen to spell `intr`.

The TTM interpreter and the ADS sequencer both run:

```sh
cargo run --release --manifest-path johnny/Cargo.toml --bin jcrender -- MJRAFT.TTM
cargo run --release --manifest-path johnny/Cargo.toml --bin jcplay -- MISCGAG.ADS 1
```

The island is the real one now: a 640x480 sea (`OCEAN00/01/02.SCR` by day,
`NIGHT.SCR` after phase 12), the island assembled from `BACKGRND.BMP` — sand,
scrub, palm trunk, fronds, and a rock and sandbar that come and go on a coin
flip — and seven patches of surf breaking round the shore, three frames each,
on three counters that chase one another so the shoreline never moves in
lockstep. `docs/SCHEDULER.md` has the addresses and the frame table.

And Johnny lives on his island — the scheduler picking scenes by weight and
time of day, walking him to them, and running them:

```sh
cargo run --release --manifest-path johnny/Cargo.toml --bin jcisland -- 600
```

`JC_NOWRITE=1` skips the PNG output. `JC_FAITHFUL=1` restores the 1992
walk-router bug, for comparing against the original's behaviour.

Timing is settled, and it is not what it first looks like. The engine's
software clock has a 4 ms unit — `seg2:0x1516` sets it to `1000 / (13 * 18)` —
but the single timer it registers asks for four of those, so the callback at
`seg2:0x16CE` runs every **16 ms**, and that callback is the whole clock: it
bumps the tick counter at `ds:0x2E72` and counts down the five delay slots.
Every delay in the data is in those 16 ms ticks. The authors thought in 60ths
of a second — the `SET_DELAY` values are 60, 120, 180, 240, 360 — and at
62.5 Hz a 60 is 0.96 seconds, close enough that nobody would have noticed.

How long a scene lasts comes out of the ADS run-state machine rather than any
duration field; `docs/SCHEDULER.md` has the write-up. The short version is
that reaching the end of a program does not stop a scene, and a scene played
with a `flags` of zero runs its program through twice — which is exactly what
makes `IF_RUNNING` on the scene you just started ever come true.

## Building the screensaver

`johnny_scr` is the Windows shell — `/s` full screen, `/p <HWND>` for the
settings-dialog preview, `/c` for the config box. It talks to Win32 directly,
so the crate still has no dependencies.

On more than one monitor it covers the whole virtual desktop with a single
window but draws **one island per screen**, each centred and in proportion,
rather than stretching a single 640x350 surface across the lot.

On Windows:

```sh
cargo build --release --bin johnny_scr
```

Cross-compiling from Linux needs a mingw-w64 linker:

```sh
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu --bin johnny_scr
```

Type-checking the Windows-only code needs no linker at all, which is worth
knowing when you are developing on Linux — this catches everything short of
actually running it:

```sh
rustup target add x86_64-pc-windows-msvc
cargo check --manifest-path johnny/Cargo.toml --target x86_64-pc-windows-msvc
```

Rename the result to `Johnny.scr` and put it in `C:\Windows\System32`, with
`RESOURCE.001` and `RESOURCE.MAP` from your own copy of the original beside it
— or point `JC_DATA` at the folder holding them.

It has been run, on a three-monitor Windows desktop: it comes up, draws the
island, plays scenes and exits on input. Development happens on a headless
Linux container, so anything that only shows up on screen is found by running
it there and reporting back.

## Layout

```
tools/extract_original.py   installer -> scrantic.exe + RESOURCE.001 + RESOURCE.MAP
tools/dgds.py               DGDS archive, chunks, LZW/RLE, images, PNG writer
tools/export_assets.py      RESOURCE.001 -> PNGs and readable script dumps
tools/ne_info.py            16-bit NE header dump, relocations and disassembler
tools/scheduler.py          the scene/story tables from the executable
tools/dgds_digest.py        canonical digest, for differential testing
tools/difftest.sh           run the Rust decoder against the Python reference
johnny/                     the Rust port
johnny/src/bin/johnny_scr.rs   the Windows .scr shell
docs/                       what was found, and how the formats work
```

`original/` and `assets/` are generated and not committed — both are derived
from Sierra's copyrighted data, and both are one command away.
