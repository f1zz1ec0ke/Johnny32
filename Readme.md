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

And Johnny lives on his island — the scheduler picking scenes by weight and
time of day, walking him to them, and running them:

```sh
cargo run --release --manifest-path johnny/Cargo.toml --bin jcisland -- 600
```

`JC_NOWRITE=1` skips the PNG output. `JC_FAITHFUL=1` restores the 1992
walk-router bug, for comparing against the original's behaviour.

Timing is settled: the engine registers one 4-unit software timer
(`seg2:0x1522` into `seg9:0x07E6`) against `USER.15`, which is Win16's
`GetCurrentTime` and returns milliseconds. **One tick is 4 ms**, so a TTM
`SET_DELAY n` waits 4n ms.

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
docs/                       what was found, and how the formats work
```

`original/` and `assets/` are generated and not committed — both are derived
from Sierra's copyrighted data, and both are one command away.
