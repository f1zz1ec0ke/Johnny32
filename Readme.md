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

Still open: ADS operand counts, and the scene scheduler that lives in the
81,919 bytes of 16-bit code.

Details in [docs/ANALYSIS.md](docs/ANALYSIS.md),
[docs/FILE_FORMATS.md](docs/FILE_FORMATS.md) and
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

Inspect the 16-bit executable:

```sh
python3 tools/ne_info.py original/scrantic.exe
python3 tools/ne_info.py original/scrantic.exe --disasm 6 --start 0xfc1 --end 0x10d0
```

## Layout

```
tools/extract_original.py   installer -> scrantic.exe + RESOURCE.001 + RESOURCE.MAP
tools/dgds.py               DGDS archive, chunks, LZW/RLE, images, PNG writer
tools/export_assets.py      RESOURCE.001 -> PNGs and readable script dumps
tools/ne_info.py            16-bit NE header dump and disassembler
docs/                       what was found, and how the formats work
```

`original/` and `assets/` are generated and not committed — both are derived
from Sierra's copyrighted data, and both are one command away.
