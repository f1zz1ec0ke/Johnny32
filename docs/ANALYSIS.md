# What is actually inside `johnnycastaway.exe`

Short version: the download is a 32-bit installer wrapping a 32-bit launcher
wrapping **the untouched 1992 16-bit original**. Nothing in it was ever ported.
That is why it cannot work on 64-bit Windows, and it is also good news — the
original data files come out intact.

## The layers

```
johnnycastaway.exe                     1,357,166 bytes
  PE32, NSIS 2.00 self-extracting installer
  │
  └── Screen Antics.scr                1,350,656 bytes packed
        PE32, UPX-packed, Borland Delphi (linker 2.25, CODE/DATA/BSS/.tls)
        1,893,888 bytes unpacked, of which 1.5 MB is one .rsrc section
        │
        ├── RCDATA SCREENANTICSEXECUTABLE   295,952 bytes -> scrantic.exe (16-bit NE)
        ├── RCDATA SCREENANTICSDATAFILE   1,175,645 bytes -> RESOURCE.001
        └── RCDATA SCREENANTICSMAPFILE        1,461 bytes -> RESOURCE.MAP
```

`tools/extract_original.py` performs all three unwrappings and writes the
originals to `original/`.

## The Delphi layer is a dropper, not a port

Its version resource is candid about what it is:

| Field | Value |
| --- | --- |
| CompanyName | `Sierra Online` |
| FileDescription | `A somewhat adapted version of the Screen Antics screensaver` |
| FileVersion | `2.0.1.0` |
| Comments | `Windows XP version of Screen Antics...` |
| `DESCRIPTION` resource | `CRNSAVE Screen Antics V2.0 unofficial` |

Its imports are ordinary Delphi VCL plus `ShellExecuteExA`,
`GetExitCodeProcess`, `CreateFileA`/`WriteFile`/`DeleteFileA`. Its strings give
away the whole runtime behaviour:

```
\scrantic.exe        \RESOURCE.001        \RESOURCE.MAP        \scrantic.ini
Sorry, an error occurred while launching Johnny saver...
```

So at run time it writes the three originals to disk and `ShellExecute`s the
16-bit binary. On 64-bit Windows there is no NTVDM to load an NE image, the
launch fails, and you get that apology. The Delphi wrapper renders nothing
itself; every pixel still comes from 1992 code.

Resource language is 1043 (Dutch), consistent with the site it came from.

## The real program: `scrantic.exe`

A Windows 3.x New Executable — the genuine Dynamix screensaver.

* Linker 5.10, 14 segments, `exetype` 2 (Windows)
* Imports only `KERNEL`, `USER`, `GDI`, `MMSYSTEM`
* Resident name `SCRNATIC`; module description `SCRNSAVE :Screen Antics`
* Exports the standard Windows screensaver entry points —
  `ScreenSaverProc`, `ScreenSaverConfigureDialog`, `PasswordDialog`,
  `ChangePasswordDialog` — plus a non-standard **`AdsControlDialog`**, which
  looks like a leftover developer scene-picker.

| Segment | Kind | Bytes |
| --- | --- | --- |
| 1 | CODE | 10,409 |
| 2 | CODE | 5,917 |
| 3 | CODE | 8,047 |
| 4 | CODE | 8,069 |
| 5 | CODE | 2,108 |
| 6 | CODE | 6,942 |
| 7 | CODE | 7,092 |
| 8 | CODE | 7,880 |
| 9 | CODE | 4,571 |
| 10 | CODE | 7,967 |
| 11 | CODE | 3,015 |
| 12 | CODE | 7,216 |
| 13 | CODE | 2,686 |
| 14 | DATA | 18,368 |

**81,919 bytes of 16-bit code.** That is small. For comparison, it is about a
third the size of the Delphi wrapper that fails to launch it. A complete,
faithful disassembly is a realistic goal rather than an aspiration.

## Data: `RESOURCE.001`

180 records, and the linear walk consumes all 1,175,645 bytes exactly, so the
container format is fully understood.

| Type | Count | What it is |
| --- | --- | --- |
| `.BMP` | 117 | sprite sheets — 2,392 frames total |
| `.TTM` | 41 | per-scene animation scripts |
| `.SCR` | 10 | 640x350 backgrounds |
| `.ADS` | 10 | scene sequencing scripts |
| `.PAL` | 1 | the 256-entry palette |
| `.VIN` | 1 | `FILES.VIN`, a preload list |

There is **no audio anywhere in this build** — no `.WAV`, no `SNG:` chunk.
The MMSYSTEM import is there, but this release ships silent.

The scripts are self-documenting. Every `.TTM` carries a `TAG:` chunk naming
its scenes: `LOAD FIRE`, `RUB STICKS`, `john gets boards`, `johnny building`,
`johnny kicks castle`, `UNDRESS JOHNNY`, `johnny gets an idea`. Every `.ADS`
carries a `RES:` chunk listing the `.TTM` files it drives.

## Status

**Solved and verified:**

* all three unwrapping layers, end to end
* the `RESOURCE.MAP` / `RESOURCE.001` container format
* both codecs — LZW ported from the disassembly, and RLE once the disassembly
  showed the literal/repeat convention is the opposite of the obvious reading.
  All 180 compressed chunks decode to exactly their declared size
* 4bpp image decoding, the palette, and index 0 as the transparency key
* every background and all 2,392 sprite frames export to PNG correctly

* **the TTM bytecode**, read out of the interpreter in segment 12 rather than
  inferred. All 41 animation scripts now disassemble end to end — 34,781
  instructions — against the three opcode tables the interpreter itself uses.

The TTM breakthrough was not the opcode table; it was noticing that `TT3:` is
RLE-compressed at all. Raw `TT3:` looks like plain bytecode — the strings are
readable and the first few dozen instructions decode — because RLE stores long
literal runs verbatim. What gives it away is one stray byte every ~128 bytes,
which is a literal-run marker. Two wrong assumptions (raw chunk, inverted RLE
convention) had been cancelling out just well enough to look like progress.

* **the ADS sequencer.** ADS turned out not to use the TTM length rule at all;
  it has its own length function at `seg10:0x0303`. With that, all ten scripts
  consume exactly, and they read as what they are — small state machines that
  play a scene, wait for it, and branch on what happened.

  The find worth having is `seg10:0x0CDA`: `3010 ... 30FF` is a **weighted
  random choice**. It sums a weight per entry, draws `rand() % total`, and runs
  the one it lands on. That is the mechanism behind the screensaver's variety,
  and the weights are sitting in the data as operand 4 of every `2005`.

**Open:**

* **The scheduler.** Which scene plays when, the story progression, the
  day/night and tide cycles, and the walking transitions all live in those
  81,919 bytes of 16-bit code. This is the part no existing project has
  recovered from the binary (see `docs/PRIOR_ART.md`).
