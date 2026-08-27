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

Or check everything at once — the tests, the decoder, the 65 scenes, three
long soaks of the whole island, every story day and holiday and both halves of
the clock, what the scenes actually draw, and the twenty-two questions the
oracle puts to the 1992 binary:

```sh
bash tools/check.sh
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
lockstep. Out on the open water, wave crests swell and subside at random
spots in three bands of distance, up to seven at a time. And the raft grows:
five stages of `MRAFT.BMP` keyed to the story day, hidden on the one scene
that sails away in it. `JC_DAY=n` on `jcisland` starts on a given day, which
is the quick way to watch it get built.

Overhead, clouds drift across the sky at their own speeds and are replaced as
they leave; on the water, thirty two-pixel glints come and go — the waves and
the glints share one array of thirty slots, so a scene that asks for a flat
calm takes both away. Everything that drifts drifts the same way, because the
wind is one variable rolled once when the sky is set up. In twenty-six minutes
of simulated island — 54,000 frames — nothing is reached that the port does
not implement: no TTM opcode, no ADS opcode. That claim is now stronger than a
soak test: every one of the thirty TTM opcodes and eighteen ADS opcodes the
shipped scripts use has been walked back to its handler, and the ones the port
treats as no-ops are ones the disassembly shows reaching their "done" label
whichever way they branch. The audit table is in `docs/SCHEDULER.md`.

## Checking against the binary instead of against myself

`tools/difftest.sh` compares the Rust decoder with the Python one, and both
came out of the same reading of the same disassembly — a shared
misunderstanding passes it clean. So there is a second harness that does not
have that problem.

`tools/x86.py` is a 16-bit x86 interpreter: enough of an 8086 to load the NE
segments, apply their relocations and *run the original's own functions*. It is
not a PC emulator — no BIOS, no video, no DOS — and anything it cannot execute
raises rather than guessing, so a gap shows up as a failure and never as a
false pass. `tools/oracle.py` puts the same questions to that and to the port
and diffs the answers:

```sh
python3 tools/oracle.py
```

```
  rand        256 answers, all identical
  weighted    256 answers, all identical
  phase      2304 answers, all identical
  choose      256 answers, all identical
  choose2     256 answers, all identical
  choose4     256 answers, all identical
  queue        16 answers, all identical
  queue6       16 answers, all identical
  place      1536 answers, all identical
  adslen     2688 answers, all identical
  route      4736 answers, all identical
  turn        512 answers, all identical
  blocks     2688 answers, all identical
  seqpass    1143 answers, all identical
  seqrun      528 answers, all identical
  island     8320 answers, all identical
  repeat       12 answers, all identical
  backdrop     64 answers, all identical
  holiday     366 answers, all identical
  codec       181 answers, all identical
  ttm       51475 answers, all identical
  tick       1327 answers, all identical
```

That covers the generator, the weighted chooser every decision is built on,
the time-of-day arithmetic exhaustively, the scene chooser at three
categories, the entire queue builder at two times of day, where the island
gets dropped, which way Johnny walks and which way he turns, and — driving
`seg10:0x0303`, the engine's own instruction-length function, along every
shipped script — where all 2,688 ADS instructions begin and end. That last one matters more than its size suggests:
the whole sequencer is built on a hand-written length table, and a table that
happens to consume each file exactly is not the same as a correct one.

A disagreement here is a bug in the port with no interpretation involved —
which is how the first run found that the island's placement rectangle was
being narrowed in the one case `seg3:0x1B7F` skips the narrowing entirely,
pinning the island to a quarter of the water it should have had all of.

`codec` is the widest of them. It hands the original's own decompressor a
stream descriptor built by hand — see "What is still not checked" for why
that is possible at all — and runs its LZW and RLE over every compressed leaf
in `RESOURCE.001`, naming each by a CRC of what came out. It checks more than
the two codecs: both sides have to agree on *which* leaves are compressed (the
`VGA:`-inside-a-`.BMP` rule), on the order they come out of the archive, and
on the size each one declares.

`JC_DEEP=1` decodes every byte — all 180 leaves, 5,225,880 bytes, twenty-five
minutes of interpreter — and all of it matches. The default stops at 8KB a
leaf, 1,188,698 bytes, which is every RLE leaf whole and past the 9-to-10 and
10-to-11 code widths on every LZW leaf. Both sides end on a line saying how
much they covered, so a run that quietly compared a prefix cannot be mistaken
for one that compared the lot.

The last two are the whole ADS sequencer. `seqpass` runs one pass of every
shipped sequence inside `seg10:0x1925` — the engine's own interpreter loop,
with its own dispatch, its own predicates and its own block scanners — against
a context built by hand, and compares which scenes the pass starts, stops and
finishes, in what order, and what state everything is left in. `blocks` asks
`seg10:0x106B` where the block starting at each of the 2,688 instructions
ends.

That pair found what a fortnight of watching the screensaver had not: `1510`
and `1520` are different instructions, `10x0` and `13x0` guards are different
instructions, and the block scanner treats both `1520` and `10x0` as ordinary
instructions rather than as anything special. Reading `1520` as a block
terminator makes a refused guard stop swallowing its block halfway, and the
rest of it then runs as though it were the next rule — several animations
starting at the moment one ends.

`seqrun` came later, and it is `seqpass`'s driver: `seg10:0x1B6B` walking
every sequence in id order, pass after pass, with the world moved on between
them by the same rule on both sides. It found the same symptom from the other
end. The engine's interpreter is handed a pointer and a sequence, so a pass in
which nothing wants to run interprets nothing; this port kept the pointer in a
field, and `begin_pass` had no way to tell `next_action` that it had found
nobody — so a finished scene's sequence went on reading, one instruction per
pass, into the next sequence's body, and started its animations. `VISITOR.ADS`
sequence 4 was drawing three Johnnys, two of them walking on the sea.

The same reading settled a whole class of these. Several things the port kept
on the screen belong to the **instance** in the engine, restored from its
record before every frame it runs (`seg10:0x1C81`): the clip rectangle, the
two drawing colours. Several things the port worked out when a scene started
are fixed when a *file loads*: an instance record per `1111` marker,
appended to one list, and that list is the draw order (`seg11:0x0645`,
`seg10:0x1C35`). With up to seven scenes on screen at once, both show. Johnny
was standing behind his own backdrop in `STAND.ADS`, and his raft in
`MARY.ADS` was three red pixels on the sand — clipped out by a rectangle
another scene had set.

Two of these need more of the interpreter than a function call. The walk
driver picks a route and then goes on to pivot Johnny and animate him, so
`tools/x86.py` grew the ability to **answer a call to one of the program's own
functions** — hook the step-to-path lookup and the route choice falls out;
hook the blitter and the pivot does, without the pager or GDI being anywhere
near it. Both found real bugs: the weighted pickers were taking the option
after the one a boundary draw belongs to, and the pivot was tossing its coin
even for a turn of nothing.

## Sixteen colours, and how to have more

Everything in `RESOURCE.001` is four-bit. All 2,392 sprite frames and all ten
640x480 backdrops use palette indices 0 to 15 and nothing else — that is the
whole visual budget of a 1992 VGA screensaver, and it is where the banding
comes from.

The surface now keeps two buffers: the indexed one the engine draws into,
unchanged and still authoritative for every check in this repository, and a
colour one beside it that each write resolves through the live palette. A
palette load recolours both, the way handing a new palette to an 8bpp DIB
recolours the screen. The screensaver presents the colour buffer through a
32bpp DIB.

That leaves room for artwork that has colours of its own. An **art pack** is a
directory of PNGs named after what they stand in for:

```text
art/
  JOHNWALK.BMP/000.png      art/OCEAN00.SCR.png
  JOHNWALK.BMP/001.png      art/ISLETEMP.SCR.png
```

`tools/export_assets.py` writes the originals out in exactly that shape, so
redrawing one is opening it and painting over it. Anything not replaced comes
out of the archive, so a pack can arrive a frame at a time. A replacement has
to be the size the original was — the scripts place every sprite by hand, in
the original's coordinates — and `jcart` says which are not:

```
$ cargo run --release --bin jcart -- art
art: 37 frames read
  wrong size  JOHNWALK.BMP/000: 99x99 where the original is 40x76
37 of 2402 images redrawn (1.5%)
```

`tools/art_manifest.py` says which are worth redrawing first: the busiest
twenty bitmaps are 634 frames and three fifths of every draw in the data.

## Settings, and the story that survives being switched off

Settings live in `johnny.ini` beside the screensaver, in the section and under
the key names the original used in `SCRANTIC.INI` — so somebody's 1992 file
works as it is. `Speed` is the one key that is not one of the original's
eleven; everything else here is read and written by `seg2:0x0C90` and the
`WritePrivateProfileString` calls after it.

```ini
[ScreenSaver.ScreenAntics]
StartTime=900        ; when Johnny's day begins, HHMM
Sounds=1
Clouds=1
Waves=1
Speed=100            ; percentage of the rate the data was authored at
Introduction=3       ; the story day on offer
NumDays=3            ; the story day reached
CurrentYear=1992     ; when those last moved on
```

The last four are not settings. `seg2:0x0C90` reads them back in and eleven
`WritePrivateProfileString` calls write them out again when Screen Antics
shuts down: `Introduction` is `ds:0x2D9B`, the story day on offer, and
`NumDays` is `ds:0x2D9C`, the day reached. That is how an eleven-day story is
eleven days long rather than eleven days *per session* — and a port that keeps
them in memory starts Johnny's story again every time the screen goes blank.
This one writes them back too.

## What is still not checked against the binary

Not the codecs, any more. They were the largest thing here that was
cross-checked rather than oracled, and the note explaining why said the
engine's decompressor pulls its input through a stream layer that would mean
standing up the C runtime's `FILE` machinery. That is true of one branch of
it. `seg6:0x0185` picks between two input modes on bit 5 of `ds:0x4030`, and
with the bit clear the input is a plain far memcpy from `ds:0x4040` — so a
stream descriptor built by hand is enough, and none of the runtime has to
exist. `tools/oracle.py codec` builds one and drives the original's own LZW
and RLE over every compressed leaf in the archive. See the query itself for
the three entry points.

Not the TTM interpreter either. That note said `seg12:0x040C` would run a
script but that stubbing the pager and the blitter changed the control flow
enough that it stopped walking — which was true of stubbing them one at a
time. Stating the *boundary* instead works: `x86.Machine.wall` answers every
call that leaves segments 1 and 12 while the interpreter is running, so the
blitter, the pager, the resource loader and the sound never run and the
interpreter never notices they are missing. Standing a loaded TTM up in front
of it is four pointers and a frame table, and `seg11:0x04D6` will build the
frame table itself — so what a frame even *is* comes from the binary.

`tools/oracle.py ttm` drives it over every frame of every script and compares
three things per frame: the instructions, as the dispatcher at `seg12:0x0620`
sees them and with the operands as the handler receives them; every drawing
decision, with the arguments the handler worked out; and what the frame left
on the instance record afterwards — palette and bitmap slot, colours, clip,
delay, end flag and goto target. 41 files, 6,631 frames, 28,150 instructions
and 16,652 draws, and all 51,475 lines match.

Sprites are in that number because the one thing the wall answers with more
than zero is the shape loader: `seg6:0x17FA` gets back a table of the same
length the real bitmap has, so `seg12:0x025B` finds artwork exactly where the
port does and the two agree about which of the 15,368 sprite draws happen at
all. What it found is below.

Nor the tick. `seg10:0x1ACB` *is* one, and it takes no arguments: it reads the
current ADS context out of `ds:0x2608` and does the three things a tick does —
the sequence bookkeeping at `seg10:0x1B12`, one pass of every sequence at
`seg10:0x1B6B`, and the instance walk at `seg10:0x1C32`. With the wall already
up it runs, and that puts the last layer under the binary: `seqrun` asks what a
pass does with no instances running and `ttm` what a frame does with nothing
else on screen, and what was left over between them was the interleaving.
`tools/oracle.py tick` drives it a scene at a time and compares, per tick, the
actions the pass took, which instances were stepped and which were held, in
what order, and everything they drew. What it found is below.

That leaves one, and it is worth being plain about it.

**The blitter.** And one small thing can be, which is worth saying first:
`ISLETEMP.SCR` is the 1992 engine's own render of the island this port
composes, so `jcisle` holds the composition up against it and scores each
piece. The island comes out at 96.5%, the palm trunk 95.2%, the fronds 90.5%,
two of the three patches of surf 75%, and the whole composition 92.3% over
18,248 painted pixels. That is the *only* place in the data where the
original's pixels can be compared with this port's, and it now runs in
`tools/check.sh`. A piece one pixel out drops into single figures, so it is a
sharp check rather than a reassuring one.

Which leaves the blit itself. `seg8:0x0AE4` is where a sprite lands, three thunks below the
opcode handler — and the pixels are not Sierra's. It builds device contexts
and hands the work to GDI, so there is no inner loop of the original's to
compare a port against: any port writes its own, and this one does.

What it decides before that call is a scale and a mirror.

The scale is dead. `seg8:0x0B02` rescales a shape whose own `[si+1]` differs
from `ds:0x2E77`, and `seg8:0x0B26` halves or doubles its dimensions when
`ds:0x2E76` differs from `ds:0x2E77` — but `seg2:0x148F` sets both of those
bytes to 3, in the same instruction that fixes the screen at 640x480, and
nothing else writes either. Neither branch can be taken in this build.

The mirror is a second copy. A shape carries two pairs of GDI handles: the
plain one at `[si+2]`/`[si+4]` and a pre-flipped one at
`[si+0x0A]`/`[si+0x0C]`, which TTM `2012` asks the loader to build and which
mirror bit 1 selects. `seg8:0x0B97` falls back to the plain pair if the
flipped one is missing, which is what the "Bitmaps won't be flipped." string
in the data segment is for. This port flips at draw time instead, which is why
`2012` has nothing to do here — and it is why only mirror modes 0 and 2 appear
in 15,000 sprite draws.

### What driving the tick found

Three more, all of them in the port, and all in the sixteen-millisecond
neighbourhood where the eye notices timing rather than geometry:

* **The script runs before the instances, not after.** `seg10:0x1B6B` and then
  `seg10:0x1C32`. This port had it the other way round, on the reasoning that a
  script running first would be reading last pass's answers — which is exactly
  what the engine wants, because it is the walk that clears state 4
  (`seg10:0x1DDC`). What the wrong order cost was a tick on the front of every
  scene: started by the pass, first drawn on the tick after.
* **A delay of zero is not a delay of one.** `seg10:0x196E` answers "the delay
  is up" without looking at the clock when `+0x29` is zero, and `seg10:0x0B3E`
  resets it to zero — so a scene whose last frame asks for no delay ends on the
  tick it ran. This port floored the delay at one and held that frame for a
  tick first.
* **A frame that changes the delay is shown one tick longer than one that
  inherits it.** `seg10:0x1CE6` re-bases the deadline to `now + delay` only
  when `ds:0x46E8` differs from `+0x29`; a frame that asks for the delay it
  already had keeps the deadline `seg10:0x196E` set when it last stepped the
  instance on, which is a tick earlier. Six ticks either way is a tenth of a
  second, and across a loop it is the difference between running in step with
  the original and drifting.

### What driving the interpreter found

Three things, all of them in the port:

* **Bitmap slots are per file, not global.** `seg12:0x0773` reads a slot out
  of the *loaded TTM* record at `ds:0x2638 + 0x0C + slot*2`, and
  `seg10:0x1C71` selects that record from the running instance's own file id
  before every frame. So two scenes of one `.TTM` share their sixteen slots —
  which is why a file can have several scenes live at once without reloading
  anything — and two scenes from *different* files cannot tread on each
  other's, however many are on screen and whatever numbers they pick. This
  port had one set of sixteen for everything. `GJVIS5.TTM` is what showed it:
  it draws from a slot it never loads, and with shared slots the port drew
  whatever the file before it had left there.
* **`A404` draws a circle, and the width operand is read by nobody.**
  `seg12:0x01C9` turns the four operands into three before anything is drawn —
  a radius of `h >> 1`, a centre at `(x + r, y + r)` — and `seg9:0x0688` hands
  GDI `Ellipse(cx - r, cy - r, cx + r, cy + r)`, whose right and bottom edges
  are excluded. All 437 in the data are square, so the width has never
  mattered; the far edge being one pixel out did.
* **The third patch of surf is two pixels out, and the code is not.**
  `seg3:0x13F2` puts it at `0x207` and the port draws what the binary says,
  but the reference's best match for it is at `0x209` and even there only
  54.7%, where its neighbours peak at 75%. The port stays with the binary and
  `jcisle` reports the number without asserting anything about it; the
  reasoning is in `docs/SCHEDULER.md`.
* **`1301` *is* `C051`.** `seg12:0x0467` rewrites it, and `1311` into `C061`,
  before it looks at anything else. Neither spelling appears in
  `RESOURCE.001` — all 535 sound cues are written `C051` — but the rewrite is
  in the interpreter, so it is in the decoder now.

And one thing that is not in the port. `seg12:0x0281` indexes a shape table by
the opcode's frame operand and does not bounds-check it. Eleven draws in the
shipped scripts ask for a frame their bitmap does not have — `GJNAT1.TTM`
wants frame 27 of a 26-frame sheet, `SMDATE.TTM` frame 31 of 24 — and what the
1992 code reads there is whatever the heap holds next. Every table in the
harness is padded past the largest index any script asks for, so the read
lands on a zero and the handler takes its no-shape path: the one answer that
is not garbage, and what the port does. They are the `blank` lines in the
query's output.

## Checking the port against itself, where the binary cannot reach

The oracle covers the deciding and now the drawing decisions too. What it
still cannot reach is the pixels: the blitter is GDI's, and putting a frame
*back* on screen is this port's own problem — the engine never has to. That
half needs an invariant of its own.

The engine draws every live frame again, every tick — that is what the `tick`
oracle turned up first, and it settled a claim that used to sit here saying the
opposite. `seg10:0x1CD6` raises `[si+0x0E]` after a frame's first run, and from
then on `seg12:0x040C` is handed the same frame every tick with `ds:0x46DA` set:
every opcode with a side effect checks that flag and does nothing, and every
opcode that draws does not check it and draws again.

This port puts the held frames back too, but from what they *drew* rather than
by running them again — which is only the same picture if putting a frame back
draws exactly what stepping it drew and changes nothing else. `JC_CHECK=1`
holds every tick to that: paint the whole tick again out of the frames alone,
and compare the pixels, the loaded artwork, the palette, the clip, the sound
queue, the generator and each instance's cursor. It is the stricter of the two
models: a held frame in the original re-reads a bitmap slot that may since have
been loaded over, and draws whatever is in it now.

`seg10:0x1E5A` is the same idea again, from the other end: it walks the
instance list and re-runs each instance's last frame with `[si+0x0E]` forced
to 1, restoring that instance's own colours and clip out of its record first,
exactly as the runner does. A frame is expected to be re-runnable, and the
parts of it that are not are marked as such.

```sh
JC_CHECK=1 JC_NOWRITE=1 cargo run --release --manifest-path johnny/Cargo.toml \
    --bin jcisland -- 40000
```

The first thing it said was that thirty-seven of the sixty-five scenes could
not be repainted. Every scene of one `.TTM` file shares that file's sixteen
bitmap slots, and the scripts load and free those from inside frames rather
than from a header — sixty-six frames in the data load or free artwork *and* draw with it.
Re-running such a frame reads a slot that now holds something else, so it
paints the wrong sprite or none at all for as long as the frame is held. So an
instance now keeps what its frame drew rather than where it drew it from: a
list of resolved marks, artwork and colour and clip already worked out.
Replaying that cannot load, cannot make a noise and cannot read a slot.

That check answers "can this tick be painted again", which an empty picture
passes perfectly. Two more ask the questions it cannot. `tools/framecheck.py`
renders a sample of frames from all sixty-five scenes and complains about a
frame that is one flat colour or a run in which nothing moves — that is how
the standing sequences were caught drawing an empty island, and the raft
caught being clipped down to three pixels. `tools/coverage.py` walks the parts
a soak from a fixed seed never reaches: all eleven story days, all seven
holidays, and both halves of the clock, twenty-five short runs in all.
`bash tools/check.sh` runs everything in this section and everything in the
last one.

Porting the engine's own generator is what made this possible. It is a
fifty-six entry additive lagged Fibonacci with a constant table in the data
segment, and "seeding" is `seg4:0x1EDC` calling it *n* times and throwing the
results away — so a whole run is determined by one number from 0 to 65535, and
two implementations can be compared draw for draw.
`docs/SCHEDULER.md` has the addresses and the frame tables.

And Johnny lives on his island — the scheduler picking scenes by weight and
time of day, walking him to them, and running them:

```sh
cargo run --release --manifest-path johnny/Cargo.toml --bin jcisland -- 600
```

He does not get one scene at a time. The engine builds a **queue**: it spends
an allowance of 300 on ten or twenty scenes, ending on a self-contained gag,
and only then does the clock move and the next queue get built. The point of
a queue is that everything in it has agreed about **where the island is** —
because the island does not sit still. `seg3:0x1B00` drops it at random in a
rectangle, and the queue's scenes vote that rectangle down to a half or a
quadrant, or pin it outright. Everything then moves with it: the surf, the
raft, the glint exclusion zones, Johnny's walk, and every sprite of every
scene, because `seg12:0x05B9` shifts a drawing opcode's operands before the
handler ever sees them. That is what `ISLETEMP.SCR` is — the island
*template*, at the anchor everything is expressed against.

It opens on the title card, and it knows what day it is: `HOLIDAY.BMP` puts a
pumpkin on the beach at the end of October, a decorated tree at Christmas, a
"HAPPY NEW YEAR" banner across the palm, and a shamrock in March. `JC_DATE`
pins the calendar if you do not want to wait, `JC_INTRO` sets how long the card
stays up.

**And it makes a noise now.** There is no audio in `RESOURCE.001` at all — the
twenty-three sound effects are Windows resources inside `SCRANTIC.EXE`, type
`WAVE`, named `WAVESFX1` upwards, 8-bit mono at 11025Hz. Put that file next to
the data and the screensaver pulls them out and plays them the way the original
does: `sndPlaySound`, asynchronous, from memory, one noise at a time. The
scripts ask for sound 535 times across 32 of the 41 of them.

`seg5:0x00BA` takes any id from 1 to 25 and looks it up in a table of
twenty-five far pointers, so two of the ids the scripts use — the file only
has twenty-three waves — find a null and play nothing. A run reports sound 24
as having no resource, and that is the original being silent there too, not
something this port failed to find.

`JC_NOWRITE=1` skips the PNG output. `JC_FROM`/`JC_TO` bound which ticks get
written, so a long run can be sampled without filling the disk. `JC_HHMM=1430`
pins the wall clock; without it `jcisland` steps one phase per queue so a
twenty-minute run still sees a whole day and night, where the screensaver
itself uses the real clock. `JC_FAITHFUL=1` restores the 1992
walk-router bug and draws the shells segment 3 puts on the beach that the
game's own picture of the island does not have — the two places where the
port and the binary disagree, both kept switchable. `JC_SPEED` is
a percentage against the rate the data was authored at, and **the default is
100**: the data runs exactly as written. It works on the screensaver too,
along with `JC_DAWN` —
when Johnny's day starts, as HHMM, defaulting to the 0900 the original ships
with.

That knob exists because the faithful rate is faster than most people
remember, and the binary says why. A 386 could not hold 62.5 passes a second;
the engine carries a speed index at `ds:0x30BE` for exactly that reason, and
its frame gate re-arms each deadline from *now* rather than from the one it
missed, so a machine that falls behind runs the animation slower and never
catches up. What people remember is this data on hardware that could not quite
play it.

It defaulted to 60 for a while on that reasoning, which was a guess at a
machine rather than a measurement. The timing is measured now:
`tools/oracle.py tick` compares this port's frame-by-frame scheduling against
`seg10:0x1ACB` and they agree to the tick. Slowing everything to 60% of that
is a preference about which decade to remember, so it is a setting and not a
default.

The time of day is not a clock in the ordinary sense. The phase is the number
of **half hours** since a dawn the user set in the config dialog, counted
modulo sixteen — so Johnny's day is eight real hours long. The shipped default
is nine in the morning, which puts dusk at three in the afternoon and the next
dawn at five: somebody wanted an office worker to see a whole day and a whole
night between arriving and going home.

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
— or point `JC_DATA` at the folder holding them. Put `SCRANTIC.EXE` there too
and you get the sound; without it the island is silent but everything else
works.

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
tools/x86.py                a 16-bit x86 interpreter that runs the original's
                            own functions
tools/scheduler.py          the scene/story tables from the executable
tools/gen_tables.py         those tables again, as the Rust the port compiles
tools/art_manifest.py       which bitmaps are worth redrawing first

tools/check.sh              everything below, cheapest first
tools/verify.py             the archive, the scripts, the generated tables,
                            and the facts the port's simplifications rest on
tools/dgds_digest.py        canonical digest, for differential testing
tools/difftest.sh           run the Rust decoder against the Python reference
tools/oracle.py             twenty-two questions put to the 1992 binary and
                            to the port, and diffed
tools/scenes.py             the 65 scenes, derived from the scene table
tools/sweep.py              play all of them with the repaint check on
tools/framecheck.py         look at what they actually draw
johnny/src/bin/jcisle.rs    the island against the engine's own render of it
tools/coverage.py           every story day, every holiday, day and night

johnny/                     the Rust port
johnny/src/bin/johnny_scr.rs   the Windows .scr shell
docs/                       what was found, and how the formats work
```

`original/` and `assets/` are generated and not committed — both are derived
from Sierra's copyrighted data, and both are one command away.
