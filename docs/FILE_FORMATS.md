# Dynamix DGDS formats as used by Johnny Castaway

Derived from the shipped data and a disassembly of the 16-bit `SCRANTIC.EXE`.
All integers are little-endian.

## `RESOURCE.MAP`

```
uint8   magic[6]        00 01 05 07 01 00
char    archive[13]     "RESOURCE.001", NUL-padded
uint16  count           180
struct { uint32 hash; uint32 offset; } entries[count]
```

`offset` is a byte offset into `RESOURCE.001`; the file is `21 + count * 8`
bytes. `hash` is a filename hash, not needed since the archive walks linearly.

## `RESOURCE.001`

```
char    name[13]        e.g. "JOHNWALK.BMP", NUL-padded
uint32  size
uint8   body[size]
```

Records run back to back and consume the file exactly.

## Chunks

Each record body is a tree of chunks:

```
char    tag[4]          e.g. "BMP:", "INF:", "BIN:"
uint32  size            top bit set => container
uint8   payload[size & 0x7FFFFFFF]
```

A container's payload is a chunk list; leaves are data.

| Tag | In | Meaning |
| --- | --- | --- |
| `BMP:` | .BMP | container |
| `INF:` | .BMP | frame count and dimensions |
| `BIN:` | .BMP/.SCR | compressed pixel data |
| `DIM:` | .SCR | `uint16 width, height` |
| `SCR:` | .SCR/.ADS | compressed screen / ADS bytecode |
| `PAL:` | .PAL | container |
| `VGA:` | .PAL | 256 raw RGB triples |
| `VGA:` | .BMP | vestigial alternate art, see below |
| `VER:` | .TTM/.ADS | version string, e.g. `4.09` |
| `PAG:` | .TTM | `uint16` page count |
| `TT3:` | .TTM | animation bytecode, RLE-compressed |
| `TTI:` | .TTM | container |
| `TAG:` | .TTM/.ADS | scene-number to name table |
| `RES:` | .ADS | resource-number to `.TTM` filename table |

`FILES.VIN` has no chunk structure: a newline-separated preload list whose
first bytes `intr` look like a tag. Bounds-check before slicing.

Compressed chunks (with the header below): `BIN:`, `SCR:`, `TT3:`, and `VGA:`
inside a `.BMP`; 180 in total (127, 10, 41, 2). The `VGA:` in `JOHNCAST.PAL`
is raw. `TT3:` shows its strings in the clear because RLE stores long literal
runs verbatim. The `VGA:` in `JOHNWALK.BMP` and `MJTELE.BMP` is almost all
zero nibbles (96,946 of 96,952); read `BIN:`.

## Compression

```
uint8   method          0 = none, 1 = RLE, 2 = LZW
uint32  uncompressed_size
uint8   data[]
```

### RLE (method 1)

Per run: `uint8 n`. Bit 7 set: repeat the next byte `n & 0x7F` times. Bit 7
clear: copy the next `n` bytes verbatim.

### LZW (method 2)

Ported from segment 6 (init `0x0D63`, `getcode` `0x0FC1`). Unix `compress`
lineage with these specifics:

* Codes are LSB-first, 9 to 12 bits wide.
* 0..255 are literals, 256 is CLEAR, the first free entry is 257.
* On CLEAR: zero prefixes 0..255, drop to 9-bit codes, set the next free
  entry to 256 (not 257). `oldcode` is not reset, so the entry added after a
  CLEAR is built from a stale `oldcode`. The encoder matches.
* At 12 bits the max-code is `0x1000`, not `0x0FFF`; the table freezes when
  full and is never auto-reset.
* Input is read in blocks of `codesize` bytes, eight codes per block. A width
  increase or a CLEAR discards the rest of the current block. Treating the
  input as one continuous bitstream decodes small files and quietly
  truncates large ones.

`tools/oracle.py codec` diffs the original decompressor against the port over
every compressed leaf (`JC_DEEP=1`: all 180, 5,225,880 bytes). `seg6:0x0185`
picks the input mode on bit 5 of `ds:0x4030` (clear: far memcpy from
`ds:0x4040`); `seg6:0x0C92(handle)` opens a stream via the vtable at
`ds:0x1DC6 + method*14 + 6`; `seg6:0x0A0D(handle, off, seg, n)` yields `n`
bytes.

## Images

Sprites and backgrounds are 4bpp, two pixels per byte, high nibble first, rows
padded to a byte boundary. Backgrounds are 640x350 (sea screens 640x480).

`.BMP` `INF:`:

```
uint16  count
uint16  widths[count]
uint16  heights[count]
```

The decompressed `BIN:` is the frames concatenated, each
`((width + 1) / 2) * height` bytes.

## Palette

`JOHNCAST.PAL` -> `PAL:` -> `VGA:`: 768 bytes, 256 RGB triples of 6-bit VGA
values (scale by 255/63). The first 16 entries are the EGA set with entries 0
and 5 exchanged: index 0 is magenta, index 5 black. Index 0 is the sprite
transparency key.

## Scripts

`.TTM` files are per-scene animation programs; `.ADS` files sequence them.
`TAG:` and `RES:` share a layout:

```
uint16  count
count * { uint16 id; char name[]; }   // NUL-terminated
```

`TT3:` (TTM) and `SCR:` (ADS) hold streams of 16-bit opcodes, both compressed.

### TTM

The low nibble of the opcode is the operand count; `0x0F` means one inline
NUL-terminated string padded to a word. Fetch loop `seg12:0x0461`; opcode
tables with parallel handlers: 47 general (`seg12:0x12BD`), 28 draw
(`seg12:0x03BC`), 18 `Cxxx` (`seg12:0x00FC`); unknown opcodes are ignored.
41 scripts, 34,781 instructions. Enum in `johnny/src/ttm.rs`; full audit in
`docs/SCHEDULER.md`.

| Opcode | Operands | Meaning |
| --- | --- | --- |
| `0x0FF0` | 0 | end of frame: present and wait |
| `0x0110` | 0 | end of scene: set `ds:0x4644` and carry on (`seg12:0x0F73`) |
| `0x1021` | 1 | frame delay, in 16 ms ticks |
| `0x2022` | 2 | frame delay, random in a range |
| `0x1111` | 1 | scene marker, the id an ADS `PlayScene` names |
| `0x1201` | 1 | jump to a scene, taken at the next frame boundary |
| `0xF02F` | string | load a sprite sheet into the current slot |
| `0xA5x4` | 4 | draw sprite (x, y, frame, slot); `x` is the mirror mode |
| `0xB606` | 6 | copy a rectangle between pages: (x, y, w, h, from, to) |
| `0xA054` / `0xA064` | 4 | save and restore one rectangle of screen |
| `0xC051` | 1 | play sound; the resources live in `SCRANTIC.EXE` (535 uses) |
| `0xA601`, `0x4214` | | empty handlers (6,874 uses between them) |

`0xB606`: `seg12:0x0EB4` resolves pages through `seg12:0x14E0` (0 working, 1
composite, 2 the island backdrop) and calls `seg8:0x06CE`. All six uses are
`(.., 2, 1)`: island back over the drawing. `0xA054`/`0xA064` use the second
draw table at `seg12:0x1900`, storing the rectangle in `ds:0x2638`; once each,
in `GJGULIVR.TTM`.

### ADS

ADS does not use the low-nibble rule: lengths come from `seg10:0x0303` (35
entries). Opcodes at `seg10:0x1875`, handlers at `0x18CD`, 44 words; an opcode
outside the table does nothing (`seg10:0x152C`). The shipped scripts hold 77
such words; `tools/reach.py` sets them aside.

An `.ADS` record holds numbered sequences that run concurrently. Each has a
state word: 1 runs this pass, 2 stopped, 4 starts on the next pass.
`seg10:0x1ACB` runs once per dispatcher pass (about every 55 ms): every
sequence in state 1 is interpreted first, then every TTM instance is stepped.

A sequence is a list of guarded rules: a condition, instructions, and a
`Step` (`1510`). It is re-read from the top every pass (`seg10:0x1C18` discards
the stopping point), so every condition is re-tested and every true one fires.
A `Step` inside a guarded block is swallowed (`seg10:0x194A`) and the pass
continues with the next rule; only a top-level `Step` ends the pass.

| Opcode | Meaning |
| --- | --- |
| `0x2005` | play scene: ttm id, scene id, repeat operand, weight |
| `0x1510` / `0x1520` | end of a guarded block (see below) |
| `0x1010`-`0x1080`, `0x1310`-`0x13C1` | conditions on a scene's run state |
| `0x1420` / `0x1430` | join the previous condition to the next with AND / OR |
| `0x3010` ... `0x30FF` | weighted random choice of one enclosed entry |
| `0x2010` / `0x2015` | stop a scene (state 0) / finish it (state 5) |
| `0x4000` / `0x4010` | draw order: `0x4000` (`seg10:0x15FB`, `seg10:0x0F8C`) appends the record to the instance list tail; `0x4010` (`seg10:0x160C`, `seg10:0x0EF2`) makes it the head. `seg10:0x1C35` draws head to tail, so `0x4000` brings a scene to the front and `0x4010` sends it behind |
| `0xF000` / `0xF200` / `0xF210` | set another sequence of this script running |
| `0xF010` | put a sequence into state 2; operand -1 means this one |
| `0xFFFF` | end of a sequence body |

`0x1420` (AND, 47 uses) and `0x1430` (OR, 30 uses) are different links.
`seg10:0x13E7` reads them left to right and short-circuits; `0x1430` also
sets a sticky flag that wins at the end (`seg10:0x14A2`).

Conditions read the instance's play count at `+0x2D` and run state at
`+0x2F`; predicates at `seg10:0x1351`-`0x13C7`:

| Opcode | True when |
| --- | --- |
| `0x1010` / `0x1310` | state 5 (finished) |
| `0x1020` / `0x1320` | not state 5 |
| `0x1030` / `0x1330` | never played (`+0x2D` is zero) |
| `0x1040` / `0x1340` | played at least once |
| `0x1050` / `0x1350` | state 4: has just ended |
| `0x1060` / `0x1360` | state 4 or 0: not waiting its turn |
| `0x1070` / `0x1370` | state 1, 2 or 3: queued |

`0x1350` is 500 of the 662 shipped conditions. State 4 is reached on the pass
a program ends and cleared when the instance is next stepped
(`seg10:0x1DE2`), so it lasts one pass; instances are stepped after the
scripts for that reason. "`0x1350` on A" means "A has just finished".

### Nesting

A rule's block runs to its `Step`; a rule inside a block is nested.
`seg10:0x1758` runs a true block through a nested interpreter. The skipper for
a false block, `seg10:0x106B`, uses eighteen entries at `cs:0x1142`:

| In the table | Scanner action |
| --- | --- |
| `1510`, `1500` | stop: the block ends here |
| `FFFF` | return null: the script ended first |
| the fifteen `13x0` guards | recurse: a nested rule's block belongs to it |
| everything else | advance one instruction |

`1520` and the `10x0` guards are not in the table and are skipped like any
instruction.

```
IF RUNNING[3,142] {
    RANDOM { play 38 x15 / play 38 x8 / play 40 x3 / nothing }
    IF (IDLE[3,38] AND IDLE[3,40]) {
        play 39, 80, 81
    }                              <- Step, closes the inner rule
}                                  <- Step, closes the outer
IF RUNNING[3,40] { ... }           <- top level, reached every pass
```

### `10x0` against `13x0`

Same predicates (table `cs:0x14BD`), different instructions (`seg10:0x1519`).
A `13x0` (`seg10:0x1758`) runs its block when true and the pass carries on. A
`10x0` (`seg10:0x17A0`) waits: when true it parks a resume pointer at the guard
(`ctx+0x292+seq*4`), counts the hit (`ctx+0xB2+seq*2`, tested by `1080`), runs
the block and returns null, ending the pass at any depth; the next pass
restarts at the guard. When false it clears both and skips to past the `1520`
(`seg10:0x118A`). One `10x0` is shipped: a `1070` in `ACTIVITY.ADS`.

### The three stop opcodes

| | |
| --- | --- |
| `1510` | ends the block it is in; at top level, ends the sequence's pass. 584 uses |
| `1520` | returns null: ends the pass at any nesting. 1 use |
| `1500` | scan a block, then stop (`seg10:0x1567`). Unused |

`seg10:0x1925` is the interpreter loop and `ds:0x460C` its stop flag; `1510`
and `1520` differ only in whether the returned pointer is live or null
(`seg10:0x1588` against `seg10:0x158A`).

### Random blocks

`seg10:0x0CDA` sums the weight of every instruction between `3010` and
`30FF`, draws `rand() % total`, and hands the winner to `seg10:0x0D9C`. Shipped
scripts use `2005` (play), `2010` (stop) and `3020` (a bare weight: nothing
this time). Weight operand (`seg10:0x0C8D`): fourth for `2000`/`2005`, first
for `3020`, third otherwise. The twelve standing sequences end through a
weighted `2010`. Run states and the repeat operand: `docs/SCHEDULER.md`, "How
long a scene lasts".
