# What is inside `johnnycastaway.exe`

The download is a 32-bit installer wrapping a 32-bit launcher wrapping the
untouched 1992 16-bit original. Nothing was ported, which is why it fails on
64-bit Windows, and why the original data files come out intact.

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

`tools/extract_original.py` performs all three unwrappings into `original/`.

## The Delphi layer

A dropper: it writes the three originals to disk and `ShellExecute`s the
16-bit binary. Without NTVDM the launch fails with "Sorry, an error occurred
while launching Johnny saver...".

| Field | Value |
| --- | --- |
| CompanyName | `Sierra Online` |
| FileDescription | `A somewhat adapted version of the Screen Antics screensaver` |
| FileVersion | `2.0.1.0` |
| Comments | `Windows XP version of Screen Antics...` |
| `DESCRIPTION` resource | `CRNSAVE Screen Antics V2.0 unofficial` |

Imports: Delphi VCL plus `ShellExecuteExA`, `GetExitCodeProcess`,
`CreateFileA`/`WriteFile`/`DeleteFileA`. Resource language 1043 (Dutch).

## `scrantic.exe`

A Windows 3.x New Executable: linker 5.10, 14 segments, `exetype` 2, imports
`KERNEL`, `USER`, `GDI`, `MMSYSTEM` only. Resident name `SCRNATIC`, module
description `SCRNSAVE :Screen Antics`. Exports the standard screensaver entry
points plus a non-standard `AdsControlDialog` (a developer scene picker).

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

81,919 bytes of 16-bit code in total.

## `RESOURCE.001`

180 records; the linear walk consumes all 1,175,645 bytes exactly.

| Type | Count | What it is |
| --- | --- | --- |
| `.BMP` | 117 | sprite sheets, 2,392 frames total |
| `.TTM` | 41 | per-scene animation scripts |
| `.SCR` | 10 | 640x350 backgrounds |
| `.ADS` | 10 | scene sequencing scripts |
| `.PAL` | 1 | the 256-entry palette |
| `.VIN` | 1 | `FILES.VIN`, a preload list |

No audio: the 23 sound effects are `WAVE` resources inside `scrantic.exe`
(`docs/SCHEDULER.md`, "The sound"). Every `.TTM` carries a `TAG:` chunk naming
its scenes; every `.ADS` a `RES:` chunk listing the `.TTM` files it drives.

## Status

Solved and verified against the binary (`docs/FILE_FORMATS.md`,
`docs/SCHEDULER.md`):

* the three unwrapping layers and the container format
* both codecs (LZW from segment 6, RLE with bit 7 meaning repeat); all 180
  compressed chunks decode to their declared size
* 4bpp images, the palette, index 0 as the transparency key; all 2,392
  sprite frames and every background export
* the TTM bytecode from segment 12's three opcode tables: 41 scripts, 34,781
  instructions
* the ADS sequencer: length function `seg10:0x0303`, weighted random block
  `seg10:0x0CDA`
* the scheduler in segment 4: scene table `ds:0x1756`, the queue, the
  eleven-day story, the sixteen-phase day, the holiday calendar
* the walk: route matrix `ds:0x0A94`, 34 authored paths, eight facings
* the island: placement, surf, waves, glints, sky, raft, decorations

Open: the blitter (`seg8:0x0AE4` hands each sprite to GDI, so there is no
pixel loop of Sierra's to check against) and what the engine draws in the sea
screens' index-0 patches.
