# The scene scheduler

Read out of `SCRANTIC.EXE` segment 4. This is the layer above the ADS
sequencer: what Johnny does next, where he has to stand to do it, when the sun
sets, and how the eleven-day story advances.

No existing reimplementation recovered this from the binary — `jc_reborn` and
the others reconstructed the behaviour by watching the screensaver run. Run
`python3 tools/scheduler.py` to dump the tables.

## Two tables in the data segment

**ADS file table** at `ds:0x166E`, 17-byte records of
`{uint16 id; char name[13]; uint16 handle}`:

| id | file | id | file |
| --- | --- | --- | --- |
| 101 | `ACTIVITY.ADS` | 107 | `MISCGAG.ADS` |
| 102 | `BUILDING.ADS` | 108 | `STAND.ADS` |
| 104 | `FISHING.ADS` | 109 | `SUZY.ADS` |
| 105 | `JOHNNY.ADS` | 110 | `VISITOR.ADS` |
| 106 | `MARY.ADS` | 111 | `WALKSTUF.ADS` |

Id 103 is unused. `seg4:0x0D37` resolves an id against this table.

**Scene table** at `ds:0x1756`, 80 records of 17 bytes, zero-terminated:

| Offset | Size | Meaning |
| --- | --- | --- |
| `+0x00` | word | group id (200-215); zero terminates the table |
| `+0x02` | byte | selection weight |
| `+0x03` | byte | required start position, major |
| `+0x04` | byte | required start position, minor |
| `+0x05` | byte | position Johnny ends at, major |
| `+0x06` | byte | position Johnny ends at, minor |
| `+0x07` | byte | cost, deducted from the caller's time budget |
| `+0x08` | byte | earliest time-of-day phase |
| `+0x09` | byte | one past the latest phase |
| `+0x0A` | byte | story day, or 0 for an everyday scene |
| `+0x0B` | dword | category bitmask |
| `+0x0F` | byte | ADS file id |
| `+0x10` | byte | sequence number in that file; 0 disables the entry |

## Choosing a scene

`seg4:0x0D76` takes a category, the current time-of-day phase and a cost
ceiling. It walks the whole table and keeps an entry when **all** of:

* the category bit is set in `+0x0B`
* `+0x10` is non-zero (the entry is enabled)
* `+0x08 <= phase` and `+0x09 > phase`
* `+0x0A` is 0 — everyday scenes only, on this path
* `+0x07 / 2` is below the caller's ceiling

Then it sums `+0x02` over the survivors, draws `rand() % total`, and walks back
to the entry it lands on. **The same weighted-random shape as an ADS
`3010...30FF` block** — the engine uses it at both levels.

Four categories are used, from the four call sites:

| Category | Caller | Use |
| --- | --- | --- |
| `0x01` | `seg4:0x0A26` | scenes reachable from Johnny's position |
| `0x02` | `seg4:0x0A8A` | second slot, same shape |
| `0x04` | `seg4:0x091C` | self-contained gags, ending at position (0,0) |
| `0x80` | `seg4:0x08DD` | the day's story scene |

The story path is different. It first rolls `rand() % 10` and gives up unless
the result is zero — so **the story scene has a one-in-ten chance** each time
it is offered. It then sets the story stage to the day counter and matches on
`+0x0A` instead of the category mask.

## Positions and walking

Johnny's current position is a pair: `ds:0x30E4` (major) and `ds:0x30E0`
(minor). Every scene entry declares the position he must be standing in
(`+0x03`,`+0x04`) and where he ends up (`+0x05`,`+0x06`); 43 of the 80 move
him. `seg4:0x0B64` is the match test, and treats a major of 0 or a minor of -1
as "don't care" — which is what `(0,0)` in the table means.

32 distinct positions appear, major 0-6 and minor 0-7.

### The route tables

When the chosen scene starts somewhere Johnny is not, `seg4:0x0094` walks him
there. Run `python3 tools/scheduler.py --walk` to dump this.

`seg4:0x03AC` looks up a **6x6 route matrix** at `ds:0x0A94` — the word at
`0x0A94 + from*12 + to*2` — giving the offset of a route block for that
(current major, target major) pair.

A route block is a flat list: a `-position` marker, then `(step, weight)`
pairs, terminated by `-7`. The engine scans for the marker matching Johnny's
current position, draws `rand() % 100`, and walks the pairs subtracting
weights until it lands on one. **Every weight list sums to exactly 100** —
with one exception, below — which is a good check that the format is right.
The pair `(0, 0)` under the marker equal to the target means "already there".

The exception is a bug in the shipped data. Route 1 to 5, at position 1, lists
six options totalling 150:

```
step 4 w16, step 4 w17, step 3 w34, step 2 w33, step 1 w25, step 1 w25
```

Since the draw is `rand() % 100`, the running subtraction can never get past
the first 100 of weight, so **the last two options are unreachable**.

That matters more than it looks. Step 1 lands on position 5 — it is the only
option in that list that reaches the target directly. Steps 4, 3 and 2 land on
2, 3 and 6, from which the block is re-entered. So the effect of the bug is
that **Johnny has never once walked straight from position 1 to position 5**;
he always detours. Weighted as authored he would take the direct route about a
third of the time:

| Step | Lands on | As shipped | As authored |
| --- | --- | --- | --- |
| 1 | 5 (the target) | 0% | 32.7% |
| 2 | 6 | 32% | 22.0% |
| 3 | 3 | 34% | 22.7% |
| 4 | 2 | 34% | 22.7% |

The other two weighted pickers in the engine — the ADS random block at
`seg10:0x0CDA` and the scene chooser at `seg4:0x0E62` — both draw against a
summed total rather than a hardcoded 100, so this picker is the odd one out. A
port should draw `rand() % total`, which makes the data behave as authored and
matches the rest of the engine. `tools/verify.py` asserts this is the only
block affected, so a parser regression cannot hide behind it.

So a block is not a single hop — it is a routing table keyed on where Johnny
currently is, re-entered after each step until he arrives.

### The walk paths

A chosen step id resolves through a jump table of stubs at `seg4:0x04FB` to one
of **34 walk-path blocks** in the data segment, from `ds:0x0AEA` to `ds:0x162A`.
Each is a list of `(frame, x, y)` triples ending in an all-zero triple:

* `x`, `y` are real screen coordinates on the 640x350 background
* bit 15 of `frame` mirrors the sprite horizontally, which is how the same
  path serves both directions — and why the executable carries the string
  `Insufficent memory (%lu).  Bitmaps won't be flipped.`

A parallel table at `ds:0x02DE`, 3 bytes per step id, says where the step
leaves him: byte 1 is the destination minor and byte 2 the destination major,
written to `ds:0x30E0`/`ds:0x30E4` once the step finishes (`seg4:0x0161`). Byte
0 is a reference minor the engine compares against on arrival. When the new
major equals the target the route is cleared; otherwise the block is re-entered
from the new position, which is how multi-step walks work.

These paths are hand-authored, 5 to 32 waypoints each. That is the part a
behavioural reimplementation has to approximate, and it is why walking is
where `jc_reborn` reports its remaining inaccuracy.

## Time of day

`seg4:0x0560` computes it: take the wall clock, subtract the `StartTime` stored
in `SCRANTIC.INI`, and reduce modulo 16. The result lives in `ds:0x30BA`.

* phases **0-11** are day — the sea is one of `OCEAN00/01/02.SCR`, picked at
  random each redraw
* phases **12-15** are night — `NIGHT.SCR`

The `+0x08`/`+0x09` window on each scene is in these units, so `0-8` is a
morning scene and `12-16` only happens after dark.

## Holidays

`seg4:0x0560` also checks the calendar and puts a background id in `ds:0x30BC`.
When it is non-zero, `HOLIDAY.BMP` is drawn over the scene.

| Date | Id |
| --- | --- |
| 29-31 October | 1 (Halloween) |
| 23-25 December | 2 (Christmas) |
| 29-31 December, and 1 January | 3 (New Year) |
| 15-17 March | 4 (St Patrick's Day) |

## The eleven-day story

The day rollover is at `seg4:0x0BC8`. `SCRANTIC.INI` keeps the state:

| Key | Variable | Meaning |
| --- | --- | --- |
| `NumDays` | `ds:0x2D9C` | days elapsed |
| `Introduction` | `ds:0x2D9B` | story stage reached |
| `CurrentYear/Month/Day` | `ds:0x2D99/97/95` | date of the last run |
| `StartTime` | `ds:0x00BE` | clock origin for the time-of-day cycle |

On the first run of a new calendar day the stored date differs, and then:

```
if (NumDays < Introduction) NumDays++;
if (NumDays > 11 || NumDays < 2) { Introduction = 1; NumDays = 1; }
store today's date
```

and once the day's story scene has actually played, `Introduction` advances if
the day counter has caught up. So the story is **gated on real elapsed days**,
wraps after eleven, and cannot be rushed by leaving the screensaver running.

The arc itself, from the `+0x0A` field:

| Day | Scene | Day | Scene |
| --- | --- | --- | --- |
| 1 | `MARY.ADS` seq 2 | 7 | `MARY.ADS` seq 4 |
| 2 | `JOHNNY.ADS` seq 2 | 8 | `MARY.ADS` seq 5 |
| 3 | `SUZY.ADS` seq 1 | 9 | `SUZY.ADS` seq 2 |
| 4 | `MARY.ADS` seq 3 | 10 | `JOHNNY.ADS` seq 6 |
| 5 | `MARY.ADS` seq 1 | 11 | `JOHNNY.ADS` seq 1 |
| 6 | `JOHNNY.ADS` seq 3 | | |

## The clock

Everything in the data — animation delays, how long a scene plays, how fast
Johnny walks — is counted in one unit, and the unit is **16 milliseconds**.

`seg2:0x1516` calls the timer setup at `seg9:0x076A` with 13, and that turns
into `1000 / (13 * 18)` = 4ms per unit of a software clock (`ds:0x45E0`); the
18 is the PC's own 18.2Hz tick. There is exactly one timer registered
(`seg2:0x1522`), asking for an interval of 4 units, so its callback at
`seg2:0x16CE` runs every 16ms — 62.5 times a second. That callback is the
whole clock. It does two things:

* bumps the 32-bit tick counter at `ds:0x2E72`
* counts down five delay slots: `ds:0x30D6`, `ds:0x334E`, `ds:0x0288`,
  `ds:0x028A` and `ds:0x3F2E`

The authors clearly thought in 60ths of a second. The delays in the scripts
are 7, 8, 10, 15, 20, 30, 60, 120, 180, 240, 360 — a `SET_DELAY` of 60 is a
second, of 240 four seconds. `PlayScene`'s durations are -60, -120, -180,
-240, -300, -360, -1000. At 62.5Hz a 60 is 0.96 seconds, which is near enough
that nobody would have noticed.

Three consumers of that clock are worth naming:

| What | Where | Rate |
| --- | --- | --- |
| Walk, one waypoint | `seg4:0x039D` writes 4 to `ds:0x30D6`, `seg4:0x00A3` waits for zero | 64ms |
| Ocean, one of three phases | `seg3:0x0E96` writes 4 to `ds:0x0288`, `seg3:0x0E8F` waits | 64ms |
| Animation frame | `SET_DELAY` into `ds:0x46E8`, armed per instance at `seg10:0x1D02`, gated at `seg10:0x1980` | the delay, in ticks |

The animation gate is worth reading in full, because it is what makes a delay
mean "between frames" rather than "once": `seg10:0x1980` compares the
instance's deadline at +0x37 with the tick counter, and when it passes, pushes
the deadline forward by the delay again before running the frame.

### The instance state machine

`seg10:0x1C56` runs an instance only when its state is 1, 2, 3 or 5. Nothing
else. That one line settles the whole lifecycle:

* **state 0** — stopped. `StopScene` writes it straight into the instance
  record, and 111 places in the scripts do. A stopped scene stops drawing,
  immediately.
* **state 4** — over. `seg10:0x1DDC` turns it into state 0 on the very next
  pass, so it lasts exactly one, and it does not draw during it.
* **states 1, 2, 3** — queued: play once, play a count, play until a deadline.
* **state 5** — set only by `FinishScene`, which no script uses.

So a scene played with `flags` of zero runs its program through **once**, and
the single pass it spends in state 4 is the entire window in which
`IF_RUNNING` can see it. That is the handshake every script is built on: play
a scene, and the block guarded on it running fires the moment it is over.

### What the clock actually says

The port runs the data at the rate it was authored for, and that is
measurable: `SJGLIMPS.TTM` scene 17 asks for 133 ticks of delay across its
twenty frames, and the port takes 134.

It still feels quicker than the screensaver people remember, and the reason is
in the binary. A 386 could not hold 62.5 passes a second. The engine has a
speed index at `ds:0x30BE` precisely because it might not keep up, and the
frame gate at `seg10:0x19A1` re-arms each deadline from *now* rather than from
the deadline it missed — so a machine that falls behind runs the animation
slower, permanently, and never catches up. The screensaver most people
remember is this data on hardware that could not quite play it.

`JC_SPEED` is a percentage against the authored rate: 100 is faithful, 70 runs
it at seven tenths.

## How long a scene lasts

Nothing in the scene table says. A scene lasts as long as its ADS sequence
takes, and the sequence is a cascade: each guarded block waits on the state of
the scene the block before it started.

`PlayScene`'s third operand is the lever. `seg10:0x0DDB` reads it as signed:

| Operand | Run state | Meaning |
| --- | --- | --- |
| `0` | 1 | play the program through |
| `> 0` | 2, count at +0x31 | play it that many times |
| `< 0` | 3, deadline at +0x33 | keep playing until `tick + \|operand\|` |

The state is then the whole story, and `seg10:0x1D2B` is where it turns:

* a scene reaching the end of its program does **not** stop — state 1 starts
  it over and becomes **4**, state 2 starts it over until the count runs out,
  state 3 starts it over until its deadline passes (`seg10:0x1DEB` promotes it
  to 4 when it does)
* only a scene already in state **4** stops, going to state **0**
  (`seg10:0x1DE2`)

So a scene played with an operand of zero runs its program through *twice*:
once to get from queued to running, once to finish. That is not a quirk to be
tidied away — it is load-bearing. Every script is a chain of
`IF_RUNNING(previous) -> play next`, and without that first pass the guard
would never come true and the chain would never start.

Two more details make the chains work:

* **A scene says when it is done.** TTM opcode `0x0110` is two instructions
  (`seg12:0x0F73`): set `ds:0x4644` and carry on. The frame still finishes
  drawing; the runner reads the flag afterwards. All 528 uses of it across the
  41 scripts are scene terminators — almost no scene simply runs out of
  instructions.
* **A scene that jumps back to its own start never ends.** `seg10:0x1D9E`
  notices (goto target == the scene's start pointer) and sets a self-loop flag
  at +0xC, and `seg10:0x07E3` passes over instances carrying it. Those are the
  background layers — a fire, a gull, Johnny standing about — and they must
  not hold the scene open, or nothing would ever end.

## The island itself

The screen is **640x480**. `seg2:0x1483` writes 640 and 480 into `ds:0x2562`
and `ds:0x2564`, and `seg3:0x01D1` clears a rectangle that size. The scene
backdrops the animation scripts load are 640x350 and cover the top of it; the
sea is the full height.

`seg3:0x01E7` builds the picture in three passes.

**The sea.** Before phase 12, one of `OCEAN00/01/02.SCR` at random, from the
pointer table at `ds:0x0262`. From phase 12, `NIGHT.SCR` at `ds:0x02D3`. All
four are 640x480.

**The island**, six pieces out of `BACKGRND.BMP`, blitted through the handle
table at `ds:0x30AC`:

| Frame | Size | At | Drawn at | What |
| --- | --- | --- | --- | --- |
| 0 | 280x52 | (289, 279) | `seg3:0x0270` | the island |
| 1 | 384x69 | (250, 303) | `seg3:0x0293` | sandbar, optional |
| 2 | 64x30 | (151, 328) | `seg3:0x02B0` | rock, optional |
| 14 | 104x28 | (397, 279) | `seg3:0x02EA` | scrub |
| 13 | 24x145 | (443, 148) | `seg3:0x0307` | the palm trunk |
| 12 | 152x69 | (366, 122) | `seg3:0x0324` | the fronds |

The two optional pieces come and go on a coin flip that is rolled once and
kept in `ds:0x30C2` (`seg3:0x0226`).

**The surf.** Seven patches of breaking water round the shore, three frames
each:

| At | Frames | Size | Counter | Set up at |
| --- | --- | --- | --- | --- |
| (271, 306) | 3-5 | 72x29 | 0 | `seg3:0x13A0` |
| (365, 319) | 6-8 | 160x25 | 1 | `seg3:0x13C9` |
| (519, 303) | 9-11 | 72x32 | 2 | `seg3:0x13F2` |
| (234, 323) | 30-32 | 120x48 | 0 | `seg3:0x141B` |
| (368, 356) | 33-35 | 176x28 | 1 | `seg3:0x1444` |
| (559, 323) | 36-38 | 88x48 | 2 | `seg3:0x146D` |
| (130, 340) | 39-41 | 104x29 | 0 | `seg3:0x1496` |

Three counters (`ds:0x0282`, `ds:0x0284`, `ds:0x0286`) say which frame each
patch is showing. They step every 4 ticks — 64ms, from the delay slot at
`ds:0x0288` — and only one of them moves per step (`seg3:0x0E9C`):

```
if c0 == c2      { c0 += 1 }
else if c1 != c0 { c1 += 1 }
else             { c2 += 1 }
each wraps 3 -> 0
```

All three start at 2 (`seg3:0x02C5`). From there it is a nine-step cycle, so
the shoreline comes back round every 576ms, and because the counters chase
each other the seven patches never move together.

### The colour that is not there

The three ocean screens are the only artwork in the game that draws with
index 0, the transparency key — about 10,000 pixels each, in wave-crest
shapes. What the engine puts there is not yet known, but the shape of the
answer is:

* every one of `OCEAN00`'s 9,917 index-0 pixels is index 9 in `OCEAN01`, and
  every neighbouring pixel in its own image is index 9 as well. Whatever goes
  there is a second shade of open water.
* it is not a palette trick. The binary imports no `AnimatePalette`, no
  `SetPaletteEntries`, no `CreatePalette` — the whole GDI import list stops at
  ordinal 119 apart from the DIB calls.
* `ISLETEMP.SCR`, which is the same island painted once and frozen, has white
  and cyan crests in the same water. Filling the key with white does make the
  oceans read as foam.

The port fills it with index 9 — the water it sits in — which cannot be far
wrong either way.

### The raft

The one thing on the island that changes with the story. `MRAFT.BMP` is
loaded next to `BACKGRND.BMP` at `seg3:0x0179` and kept in `ds:0x30AE`; it
holds five frames of a raft under construction, 72x39 growing to 128x39.

`seg3:0x1D7E` reads the story day out of `ds:0x2D9C`, takes one off it and
clamps to 1..5, so there is a raft from the first day, it grows through days
three to six, and from day six it is finished. It goes at (513, 266), or
(530, 281) when the rock and sandbar are there (`seg3:0x1DCB`).

One scene turns it off — `MARY.ADS` sequence 5, "JOHNNY LEAVES", the day-eight
story scene where he sails away on it. The scene animates the raft itself, so
the backdrop must not also be drawing one.

### What a scene asks of the backdrop

The scene table's 32-bit mask at `+0x0B` is not only a category filter. The
chooser hands it to `seg3:0x1C41`, which reads more bits out of it:

| Bit | Effect |
| --- | --- |
| `0x0010` | clears `ds:0x30B6` |
| `0x0200` | `ds:0x30C2 = 0` — no rock and no sandbar |
| `0x0800` | `ds:0x30B4 = 1` |
| `0x1000` | sets `ds:0x30B2` |
| `0x2000` | `ds:0x30B4 = 2` — a cutaway: `seg3:0x01C4` clears the whole 640x480 to black instead of drawing a sea, and the scene's own `.SCR` is the picture |
| `0x4000` | sets `ds:0x30B0` — no raft |
| `0x010000`-`0x200000` | the island placement bits below |

Passing all ones resets the lot (`seg3:0x1C4B`), which is what puts the island
back to normal between scenes.

Seventeen of the eighty scenes ask for no rock; four are cutaways — Suzy's
city, meeting Suzy, the office, and the ending — and one asks for no raft.

### The island is placed at random, and the port does not do it

`seg3:0x1B00` picks the island's position rather than fixing it. It hands
`seg3:0x14BF` a rectangle — (168, 255) by 111x59 with the rock, (67, 255) by
226x129 without — and the high bits of the scene mask (`ds:0x30C0`) narrow it
to a quadrant or replace it outright: bit 21 puts the island at (17, 309), bit
20 at (289, 299). Everything else in segment 3 is drawn relative to the result,
and so is Johnny's walking: `seg4:0x02EA` adds `ds:0x30C6`/`ds:0x30C4` to each
waypoint before blitting him.

The port keeps the island at (289, 279), which is the anchor the offsets are
all expressed against and the placement `ISLETEMP.SCR` was painted at. The
reason not to move it is that **the scenes do not follow**: the TTM sprite
handler at `seg12:0x025B` draws at absolute coordinates, and segment 12 does
not reference the origin at all — only segments 3 and 4 do. Move the island
and Johnny acts out his scenes over open water. Whether the original had the
same problem, or whether something else pins the origin in practice, is not
settled.

### Waves on the open water

There is a second wave system, and this one comes and goes. `seg3:0x04BD`
walks an array of twelve-byte object records at `ds:0x2F42`; an empty slot may
be filled, a live one is moved on.

Three of the four kinds are waves, and each is a four-frame `BACKGRND.BMP`
animation played as a ping-pong. The sequences live at `ds:0x0268`, `ds:0x0270`
and `ds:0x0278`, terminated by `0xFF`:

| Kind | Sequence | Frames | Size | On the water at once | Spawns in | Set up at |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | `ds:0x0278` | 18-21 | 32x6 | 4 | x -10 +650, y 249 +40 | `seg3:0x05CA` |
| 2 | `ds:0x0270` | 22-25 | 64x12 | 2 | x -40 +680, y 289 +93 | `seg3:0x0627` |
| 3 | `ds:0x0268` | 26-29 | 104x16 | 1 | x -70 +710, y 382 +142 | `seg3:0x0682` |

The three sizes are three distances: the small one high and far out, the big
one low and close. `seg3:0x14BF` picks the spot inside the band, taking the
sprite's own size off the span so it lands whole, and the caps come from
`seg3:0x046E`.

A slot is filled by two rolls through the weighted chooser at `seg4:0x1F1B`,
whose arguments are percentages (it scales each by 655, a hundredth of 65536,
and walks them against a 16-bit draw). First a flat 35% for the slot to be
filled at all (`seg3:0x04CE`), then `[35, 5, 5, 5]` for what goes in it
(`seg3:0x04EE`): 35% a glint, 5% each wave size, 50% nothing after all.

A wave holds each frame for 0, 1 or 2 steps — `seg4:0x1F1B(30, 50)`, so 30%
one, 50% two, 20% none — and a step is 160ms, from the delay slot at
`ds:0x028A` (`seg3:0x0A19`). Seven frames later the cursor hits the `0xFF` and
the wave retires, giving its slot back. So one lasts between about one and
three and a half seconds.

They barely travel. Each step adds `ds:0x0280` to the x, and that is +1 or -1,
re-rolled at `seg3:0x0A59` on a straight 50/50. The effect is a crest that
swells, shimmers and subsides where it is, not one that rolls ashore.

### The speed gate

Nothing in that array spawns unless `ds:0x30BE` is over 100 (`seg3:0x04E4`),
and `ds:0x30BE` is a **speed index**: `seg2:0x1600` times a calibration loop
against the system clock and divides `100000 * iterations` by the elapsed
time, twice by ten. On a slow 386 the decorative objects were simply skipped.
Any machine running this port is over the line, so the port spawns them.

### The sky

Fifteen cloud slots (`ds:0x2E82`), filled from `BACKGRND.BMP` frames 15, 16
and 17 — three sizes of cloud bank — with a cap on each and a weight in the
spawn roll (`seg3:0x0AB1`):

| Frame | Size | At most | Weight | Cap at |
| --- | --- | --- | --- | --- |
| 15 | 128x36 | 8 | 15% | `ds:0x2E7C` |
| 16 | 192x57 | 4 | 10% | `ds:0x2E7E` |
| 17 | 264x76 | 2 | 5% | `ds:0x2E80` |

The roll runs biggest first and falls through to the next size down when that
one is at its cap (`seg3:0x0AC9`). Clouds go anywhere across the width, in the
top 139 rows (`seg3:0x0B0C`). Each gets its own speed — `rand() % 5 + 6`
passes per pixel (`seg3:0x0B2B`) — and when one leaves the screen it is
retired and its cap given back (`seg3:0x0D19`), so the sky refills behind it.

**The wind is one thing.** `ds:0x0280` is +1 or -1, and `seg3:0x0A59` rolls it
once when the sky is set up. The clouds drift with it, and so do the waves out
on the water — they read the same variable. That is why everything on screen
moves the same way.

### Glints

`seg3:0x07DE` sets the drawing colour to 15 and draws a line from x to x+1: a
glint is two white pixels. Thirty at a time (`ds:0x2F38`), scattered across a
band of open water below the horizon — x anywhere, y in 214..249
(`seg3:0x053C`) — each lasting ten to thirty-nine steps of 160ms.

They take 35% of the spawn roll against 5% for each wave size, so most of what
that array holds is sparkle. One that lands on the palm is thrown away rather
than drawn (`seg3:0x055E` tests the trunk, `seg3:0x058A` the fronds) — these
are reflections, and the palm is not water.

### The raft

The one thing on the island that changes with the story. `MRAFT.BMP` is
loaded next to `BACKGRND.BMP` at `seg3:0x0179` and kept in `ds:0x30AE`; it
holds five frames of a raft under construction, 72x39 growing to 128x39.

`seg3:0x1D7E` reads the story day out of `ds:0x2D9C`, takes one off it and
clamps to 1..5, so there is a raft from the first day, it grows through days
three to six, and from day six it is finished. It goes at (513, 266), or
(530, 281) when the rock and sandbar are there (`seg3:0x1DCB`).

One scene turns it off — `MARY.ADS` sequence 5, "JOHNNY LEAVES", the day-eight
story scene where he sails away on it. The scene animates the raft itself, so
the backdrop must not also be drawing one.

### What a scene asks of the backdrop

The scene table's 32-bit mask at `+0x0B` is not only a category filter. The
chooser hands it to `seg3:0x1C41`, which reads more bits out of it:

| Bit | Effect |
| --- | --- |
| `0x0010` | clears `ds:0x30B6` |
| `0x0200` | `ds:0x30C2 = 0` — no rock and no sandbar |
| `0x0800` | `ds:0x30B4 = 1` |
| `0x1000` | sets `ds:0x30B2` |
| `0x2000` | `ds:0x30B4 = 2` — a cutaway: `seg3:0x01C4` clears the whole 640x480 to black instead of drawing a sea, and the scene's own `.SCR` is the picture |
| `0x4000` | sets `ds:0x30B0` — no raft |
| `0x010000`-`0x200000` | the island placement bits below |

Passing all ones resets the lot (`seg3:0x1C4B`), which is what puts the island
back to normal between scenes.

Seventeen of the eighty scenes ask for no rock; four are cutaways — Suzy's
city, meeting Suzy, the office, and the ending — and one asks for no raft.

### The island is placed at random, and the port does not do it

`seg3:0x1B00` picks the island's position rather than fixing it. It hands
`seg3:0x14BF` a rectangle — (168, 255) by 111x59 with the rock, (67, 255) by
226x129 without — and the high bits of the scene mask (`ds:0x30C0`) narrow it
to a quadrant or replace it outright: bit 21 puts the island at (17, 309), bit
20 at (289, 299). Everything else in segment 3 is drawn relative to the result,
and so is Johnny's walking: `seg4:0x02EA` adds `ds:0x30C6`/`ds:0x30C4` to each
waypoint before blitting him.

The port keeps the island at (289, 279), which is the anchor the offsets are
all expressed against and the placement `ISLETEMP.SCR` was painted at. The
reason not to move it is that **the scenes do not follow**: the TTM sprite
handler at `seg12:0x025B` draws at absolute coordinates, and segment 12 does
not reference the origin at all — only segments 3 and 4 do. Move the island
and Johnny acts out his scenes over open water. Whether the original had the
same problem, or whether something else pins the origin in practice, is not
settled.

### Waves on the open water

There is a second wave system, and this one comes and goes. `seg3:0x04BD`
walks an array of twelve-byte object records at `ds:0x2F42`; an empty slot may
be filled, a live one is moved on.

Three of the four kinds are waves, and each is a four-frame `BACKGRND.BMP`
animation played as a ping-pong. The sequences live at `ds:0x0268`, `ds:0x0270`
and `ds:0x0278`, terminated by `0xFF`:

| Kind | Sequence | Frames | Size | On the water at once | Spawns in | Set up at |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | `ds:0x0278` | 18-21 | 32x6 | 4 | x -10 +650, y 249 +40 | `seg3:0x05CA` |
| 2 | `ds:0x0270` | 22-25 | 64x12 | 2 | x -40 +680, y 289 +93 | `seg3:0x0627` |
| 3 | `ds:0x0268` | 26-29 | 104x16 | 1 | x -70 +710, y 382 +142 | `seg3:0x0682` |

The three sizes are three distances: the small one high and far out, the big
one low and close. `seg3:0x14BF` picks the spot inside the band, taking the
sprite's own size off the span so it lands whole, and the caps come from
`seg3:0x046E`.

A slot is filled by two rolls through the weighted chooser at `seg4:0x1F1B`,
whose arguments are percentages (it scales each by 655, a hundredth of 65536,
and walks them against a 16-bit draw). First a flat 35% for the slot to be
filled at all (`seg3:0x04CE`), then `[35, 5, 5, 5]` for what goes in it
(`seg3:0x04EE`): 35% a glint, 5% each wave size, 50% nothing after all.

A wave holds each frame for 0, 1 or 2 steps — `seg4:0x1F1B(30, 50)`, so 30%
one, 50% two, 20% none — and a step is 160ms, from the delay slot at
`ds:0x028A` (`seg3:0x0A19`). Seven frames later the cursor hits the `0xFF` and
the wave retires, giving its slot back. So one lasts between about one and
three and a half seconds.

They barely travel. Each step adds `ds:0x0280` to the x, and that is +1 or -1,
re-rolled at `seg3:0x0A59` on a straight 50/50. The effect is a crest that
swells, shimmers and subsides where it is, not one that rolls ashore.

### The speed gate

Nothing in that array spawns unless `ds:0x30BE` is over 100 (`seg3:0x04E4`),
and `ds:0x30BE` is a **speed index**: `seg2:0x1600` times a calibration loop
against the system clock and divides `100000 * iterations` by the elapsed
time, twice by ten. On a slow 386 the decorative objects were simply skipped.
Any machine running this port is over the line, so the port spawns them.

### Glints, not yet ported

The fourth kind (spawn at `seg3:0x0515`, update at `seg3:0x07DE`) has a budget
of thirty and is the only one that gets 35% of the roll rather than 5%. It is
not sprite-based: it blits small regions between the three pages at
`ds:0x1EB0`, `ds:0x1EB2` and `ds:0x1EB4` and puts them back when its hold
expires. `seg3:0x055E` rejects any position overlapping the palm trunk
(24x145 at 443,148) or the fronds (152x69 at 366,122), which is what gives it
away as something on the water rather than on the island — thirty glints
twinkling on the sea. Porting it needs the page semantics of the blitter at
`seg13:0x007E`, which are not read out yet.

## In the port

`tools/gen_tables.py` emits these tables as `johnny/src/tables.rs`, so the port
does not need the 16-bit binary at run time. `johnny/src/director.rs`
implements the chooser, the walk router and the clock; `jcisland` runs it.

Two things the port had to get right that are easy to get wrong:

* `GotoFrame` does not jump on the spot. `seg12:0x0FC3` stashes the target and
  it is taken at the frame boundary — jumping immediately makes a scene that
  loops back on itself spin forever inside one call rather than yielding.
* The island backdrop. `seg3:0x01E7` picks `NIGHT.SCR` or one of
  `OCEAN00/01/02.SCR`, but all four are 640x480 and use palette index 0 — the
  transparency key — across thousands of pixels. They are the VGA mode's
  animated sea, drawn over something else, not a backdrop in their own right.
  The port uses `ISLETEMP.SCR` for now, which is the 640x350 island the scenes
  themselves load and matches the geometry the sprites and walk paths expect.

## Still open

* What the two position components mean geometrically. Major 0-6 and minor
  0-7 look like an island region and a spot within it, but that is inference
  from the tables rather than something the code states. The waypoint
  coordinates should settle it once they are plotted over a background.
* `seg3:0x1C41`, which the picker hands the chosen entry's mask to.
* The outer loop, which is further along than it was. `seg4:0x0A30` shows the
  shape: after the chooser lands on an entry it does `budget -= cost` and
  stores the entry into a **list** at `ds:0x30F4`, indexed by a counter, going
  round again while the budget holds. So the scheduler picks a *run of scenes*
  against a time allowance rather than one at a time, which is what `cost` is
  a price in. Where the allowance comes from and what refills it is still to
  be read.
* What the engine draws in the oceans' index-0 crests, above under "The
  colour that is not there".
* ADS `0x4000`: `seg10:0x15FB` unlinks a scene from the instance list and
  puts it back somewhere else, so it and `0x4010` are "in front of" and
  "behind". Which is which is not read out. Only `0x4000` is used, four times,
  in `BUILDING.ADS`, always on the same scene.
* Whether the island's random placement can be honoured — above, under "The
  island is placed at random".
