# Dynamix DGDS formats as used by Johnny Castaway

Derived from the shipped data plus a disassembly of the original 16-bit
`SCRANTIC.EXE`. All integers are little-endian.

## `RESOURCE.MAP`

```
uint8   magic[6]        00 01 05 07 01 00
char    archive[13]     "RESOURCE.001", NUL-padded
uint16  count           180
struct { uint32 hash; uint32 offset; } entries[count]
```

`offset` is a byte offset into `RESOURCE.001`. The file is exactly
`21 + count * 8` bytes. `hash` is a filename hash; it is not needed in
practice, because the archive can be walked linearly.

## `RESOURCE.001`

A flat sequence of records:

```
char    name[13]        e.g. "JOHNWALK.BMP", NUL-padded
uint32  size
uint8   body[size]
```

Walking these back to back consumes the file exactly.

## Chunks

Each record body is a tree of chunks:

```
char    tag[4]          e.g. "BMP:", "INF:", "BIN:"
uint32  size            top bit set => container
uint8   payload[size & 0x7FFFFFFF]
```

A container's payload is itself a chunk list. Leaves are data.

Chunks seen in this build:

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
| `TT3:` | .TTM | animation bytecode |
| `TTI:` | .TTM | container |
| `TAG:` | .TTM/.ADS | scene-number to name table |
| `RES:` | .ADS | resource-number to `.TTM` filename table |

`FILES.VIN` is the exception: it has no chunk structure at all, just a
newline-separated preload list (`intro.scr`, `johncast.pal`, `ocean*.scr` and
so on). Its first four bytes spell `intr`, from `intro.scr`, which looks enough
like a tag that a parser slicing without a bounds check will happily invent a
chunk out of it.

**`BIN:`, `SCR:`, `TT3:` and `VGA:`-inside-a-`.BMP` carry a compression
header.** Everything else is raw, including the `VGA:` in `JOHNCAST.PAL`, whose
768 bytes are the palette directly. That makes 180 compressed chunks: 127
`BIN:`, 10 `SCR:`, 41 `TT3:` and 2 `VGA:`.

`TT3:` is easy to miss. It is RLE-compressed, and because RLE stores long
literal runs verbatim, a raw `TT3:` still shows its strings in the clear and
still disassembles for the first few dozen instructions. The giveaway is a
stray byte every ~128 bytes -- the literal-run markers.

Exactly two records, `JOHNWALK.BMP` and `MJTELE.BMP`, carry a `VGA:` chunk
alongside their `BIN:`. It decompresses to the same byte count as the `BIN:`
but is not a second image: in `JOHNWALK.BMP` it is 96,946 zero nibbles out of
96,952. It looks like an alternate-art slot the Dynamix toolchain reserved and
nobody ever filled. Ignore it and read `BIN:`.

## Compression

A compressed leaf payload is:

```
uint8   method          0 = none, 1 = RLE, 2 = LZW
uint32  uncompressed_size
uint8   data[]
```

### RLE (method 1)

Per run: `uint8 n`. **If bit 7 is set**, repeat the following single byte
`n & 0x7F` times. **If bit 7 is clear**, copy the next `n` bytes verbatim.

The inverted reading is the natural guess and it is wrong. It still decodes
short streams nearly correctly, so it fails quietly.

### LZW (method 2)

Ported from `SCRANTIC.EXE` segment 6 (init at `0x0D63`, `getcode` at `0x0FC1`).
It is of the Unix `compress` lineage, and the details matter:

* Codes are **LSB-first**, 9 to 12 bits wide.
* Entries 0..255 are the literals. Code **256 is CLEAR**. The first free entry
  is **257**.
* On CLEAR the decoder zeroes prefixes 0..255, drops back to 9-bit codes, and
  sets the next free entry to **256, not 257**. `oldcode` is deliberately *not*
  reset, so the entry added immediately after a CLEAR is built from a stale
  `oldcode`. The encoder does the same thing, so it round-trips.
* At 12 bits the original stores a max-code of `0x1000`, not `0x0FFF`. Since
  the next free entry also caps at `0x1000`, the comparison stops being true
  and the table simply **freezes** — it is never auto-reset when full.
* **The input is read in blocks of `codesize` bytes — eight codes per block.**
  A code-width increase or a CLEAR discards whatever is left of the current
  block and starts a fresh one.

That last point is the one that bites. Treat the input as a single continuous
bitstream and small files still decode perfectly, because they never reach the
first width change; large ones quietly come out a few percent short. All 139
compressed chunks in `RESOURCE.001` decode to exactly their declared size with
the block behaviour in place.

## Images

Both sprites and backgrounds are **4bpp, two pixels per byte, high nibble
first**, rows padded to a byte boundary. Backgrounds are 640x350.

`.BMP` `INF:` is:

```
uint16  count
uint16  widths[count]
uint16  heights[count]
```

and the decompressed `BIN:` is those frames concatenated, each
`((width + 1) / 2) * height` bytes.

## Palette

`JOHNCAST.PAL` -> `PAL:` -> `VGA:`, 768 raw bytes, 256 RGB triples of 6-bit
VGA values (scale by 255/63).

The first 16 entries are the EGA set **with entries 0 and 5 exchanged**, so
index 0 holds magenta and index 5 holds black. Index 0 is the transparency key
for sprites; the swap is what frees black up to be drawn normally.

## Scripts

`.TTM` files are per-scene animation programs, `.ADS` files sequence them.

`TAG:` and `RES:` share a layout:

```
uint16  count
count * { uint16 id; char name[]; }   // NUL-terminated
```

`TT3:` (TTM) and `SCR:` (ADS) both hold a stream of 16-bit opcodes, and both
are compressed — `TT3:` in particular is RLE, which is easy to miss because
long literal runs leave most of the bytecode readable.

### TTM

The low nibble of the opcode is the operand count, and `0x0F` means one inline
NUL-terminated string padded to a word boundary. `seg12:0x0461` is the fetch
loop. Opcodes are looked up in three tables — 47 general (`seg12:0x12BD`), 28
draw (`seg12:0x03BC`) and 18 in the `Cxxx` family (`seg12:0x00FC`) — each with
a parallel table of handlers; anything absent is ignored rather than fatal.

All 41 scripts disassemble end to end, 34,781 instructions. The full list is
in `johnny/src/ttm.rs`, where the opcode enum is exhaustive so a missed one is
a compile error rather than a silent no-op. The ones worth knowing here:

| Opcode | Operands | Meaning |
| --- | --- | --- |
| `0x0FF0` | 0 | end of frame — present what has been drawn and wait |
| `0x0110` | 0 | end of scene: set `ds:0x4644` and carry on (`seg12:0x0F73`) |
| `0x1021` | 1 | frame delay, in 16ms ticks |
| `0x2022` | 2 | frame delay, random in a range |
| `0x1111` | 1 | scene marker — the id an ADS `PlayScene` names |
| `0x1201` | 1 | jump to a scene, taken at the next frame boundary |
| `0xF02F` | string | load a sprite sheet into the current slot |
| `0xA5x4` | 4 | draw sprite (x, y, frame, slot); `x` is the mirror mode |
| `0xB606` | 6 | copy a rectangle between pages: (x, y, w, h, from, to) |
| `0xA054` / `0xA064` | 4 | save and restore one rectangle of screen |

`0xB606` is worth spelling out, because it is the only one that reaches
outside the scene. `seg12:0x0EB4` resolves its last two operands through
`seg12:0x14E0` — page 0 is the working page, 1 the composite, 2 the island
backdrop segment 3 paints — and hands the rectangle to the copier at
`seg8:0x06CE`. All six uses in the data are `(.., 2, 1)`: put a piece of the
island back over whatever has been drawn on it. That is how a scene rubs out
what it drew, and it is why a port that repaints the whole background every
frame gets it for free.

`0xA054` and `0xA064` are the same idea the long way round, through the second
draw table at `seg12:0x1900`. `A054` writes the rectangle into the structure
at `ds:0x2638`; `A064` walks it and draws it back. Six of that table's eight
opcodes are never used at all, and these two are used once each, in
`GJGULIVR.TTM`, with the same rectangle.

Three opcodes that look like they should do something do not, in this build:
`0xC051` is `PLAY_SOUND` (535 call sites, and the screensaver ships no audio),
and `0xA601` and `0x4214` have empty handlers despite 6,874 call sites between
them.

### ADS

ADS does **not** use the low-nibble rule. Each handler advances the script
pointer itself, and the lengths come from a dedicated function at
`seg10:0x0303` with its own 35-entry table. The opcodes are at `seg10:0x1875`
with handlers at `0x18CD`.

An `.ADS` record holds numbered sequences. A sequence is a run of guarded
blocks — a condition, some instructions, a `Step` — and the runner
(`seg10:0x1925`) interprets until a `Step` and then returns, so one block
happens per pass. A `Step` is a yield, not a join; what holds a block back is
the `If` in front of it.

| Opcode | Meaning |
| --- | --- |
| `0x2005` | play scene: ttm id, scene id, repeat operand, weight |
| `0x1510` / `0x1520` | step (yield); `0x1520` also returns from the interpreter |
| `0x1010`-`0x1080`, `0x1310`-`0x13C1` | conditions on a scene's run state |
| `0x1420` / `0x1430` | chain the previous condition with OR |
| `0x3010` ... `0x30FF` | weighted random choice of one enclosed entry |
| `0x2010` / `0x2015` | stop a scene (state 0) / finish it (state 5) |
| `0x4000` / `0x4010` | move a scene in the draw order (`seg10:0x15FB`) |
| `0xF010`, `0xFFFF` | end of sequence |

The conditions all read one field — the scene's run state at +0x2F of its
instance record — and their predicates are at `seg10:0x1351`-`0x13C7`:

| Opcode | True when |
| --- | --- |
| `0x1010` / `0x1310` | state 5 (finished) |
| `0x1020` / `0x1320` | not state 5 |
| `0x1030` / `0x1330` | never started (+0x2D is zero) |
| `0x1040` / `0x1340` | started |
| `0x1050` / `0x1350` | state 4 (running) |
| `0x1060` / `0x1360` | state 4 or 0 — not waiting its turn |
| `0x1070` / `0x1370` | state 1, 2 or 3 — queued |

What those states mean, and how the repeat operand drives them, is in
`docs/SCHEDULER.md` under "How long a scene lasts". It is the part that
decides pacing, and it is not obvious from the bytecode alone.
