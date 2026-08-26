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
| `intr` | .VIN | newline-separated preload list |

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

Bytecode in `TT3:` (TTM) and `SCR:` (ADS) is a stream of 16-bit opcodes where
the low nibble is the operand count and `0x0F` means one inline
NUL-terminated string padded to a word boundary. Confirmed opcodes:

| Opcode | Operands | Meaning |
| --- | --- | --- |
| `F010` | string | load background `.SCR` |
| `F020` | string | load sprite sheet `.BMP` into the current slot |
| `F050` | string | load palette `.PAL` |
| `1050` | 1 | select bitmap slot |
| `4210` | 4 | rectangle (x, y, w, h) |
| `A500` | 4 | draw sprite (x, y, frame, slot) |
| `0FF0` | 0 | end of frame |

**This table is incomplete and the decoder loses sync partway into most
scripts** — see the "Open" section of `docs/ANALYSIS.md`.
