# The scene scheduler

Read out of `SCRANTIC.EXE` segment 4 (scheduling, walking, story, clock) and
segment 3 (the island). `tools/scheduler.py` dumps the tables. `tools/oracle.py`
runs the original code against the port (24 queries; `water` is the backdrop
pass by pass, `run` the dispatcher `seg4:0x06BF` for 1,500 passes with scenes
stubbed). `tools/deep.sh` runs the deep tier in resumable pieces.

## Two tables in the data segment

**ADS file table** at `ds:0x166E`, 17-byte records
`{uint16 id; char name[13]; uint16 handle}`, resolved by `seg4:0x0D37`:

| id | file | id | file |
| --- | --- | --- | --- |
| 101 | `ACTIVITY.ADS` | 107 | `MISCGAG.ADS` |
| 102 | `BUILDING.ADS` | 108 | `STAND.ADS` |
| 104 | `FISHING.ADS` | 109 | `SUZY.ADS` |
| 105 | `JOHNNY.ADS` | 110 | `VISITOR.ADS` |
| 106 | `MARY.ADS` | 111 | `WALKSTUF.ADS` |

Id 103 is unused.

**Scene table** at `ds:0x1756`, 80 records of 17 bytes, zero-terminated:

| Offset | Size | Meaning |
| --- | --- | --- |
| `+0x00` | word | group id (200-215); zero terminates the table |
| `+0x02` | byte | selection weight |
| `+0x03` | byte | required start area 1-6; 0 = any |
| `+0x04` | byte | required start facing 0-7; -1 = any |
| `+0x05` | byte | end area; 0 = `rand % 6 + 1` at the landing |
| `+0x06` | byte | end facing; 0 = `rand & 7` at the landing |
| `+0x07` | byte | cost, deducted from the queue budget |
| `+0x08` | byte | earliest time-of-day phase |
| `+0x09` | byte | one past the latest phase |
| `+0x0A` | byte | story day, or 0 for an everyday scene |
| `+0x0B` | dword | category and backdrop bitmask |
| `+0x0F` | byte | ADS file id; `0xFF` is an idle row |
| `+0x10` | byte | sequence number in that file; 0 disables the entry |

32 distinct positions appear; 43 of the 80 rows move Johnny.

## Choosing a scene

`seg4:0x0D76(category, phase, ceiling)` keeps a row when all of:

* the category bit is set in `+0x0B`
* `+0x10` is non-zero
* `+0x08 <= phase < +0x09`
* `+0x0A == 0`
* `+0x07 / 2 < ceiling`
* `seg4:0x0EDB` says the row's island bits agree with the queue (below)

It sums `+0x02` over the survivors, draws `rand() % total` plus one, and
subtracts weights until the count stops being positive (`seg4:0x0E62`).

| Category | Caller | Use |
| --- | --- | --- |
| `0x01` | `seg4:0x0A26` | first slot |
| `0x02` | `seg4:0x0A8A` | later slots |
| `0x04` | `seg4:0x091C` | closing gag; ceiling forced to 999 (`seg4:0x0D90`) |
| `0x80` | `seg4:0x08DD` | story scene: `rand() % 10` must be 0, then match `+0x0A` against the story day |

## The queue

`seg4:0x08B9` builds the queue at `ds:0x30F4` (cursor `ds:0x30F2`) from a
budget of 300 (`seg4:0x0645`). Each row costs `+0x07`, 5 to 105, so a queue is
ten to twenty rows. Order:

1. Roll for the story (`choose(0x80)`). If it comes up and is not one of the
   two that ran last time, hold it: as the closing gag if its mask has the
   standalone bit, else as a companion.
2. Draw the closing gag from `0x04`; redraw while it closed the last queue, or
   while there is a companion and the gag also stands as a primary.
3. Charge both to the budget; hand both masks to `seg3:0x1C41`.
4. If the gag is a primary, the queue is that one scene (the four cutaways).
5. Slot 0: the companion if it is a primary, on a 50% flip; else a `0x01`
   draw.
6. Loop: 5% the companion, else a `0x02` draw. A row with mask bit 3 is
   repeatable: 1-6 goes weighted 10/20/30/20/10/10, redrawn until affordable.
7. Push the gag last.

Every queued row gets bit 15 set (unresolved). Johnny's position is written
from the first row's `from` field unconditionally; he walks to all the rest.

## The dispatcher pass

Two clocks:

* A software tick every 16 ms (`seg2:0x16CE`) increments `ds:0x2E72` and
  decrements five delay slots: `ds:0x30D6` walk, `ds:0x334E` idle,
  `ds:0x0288` surf, `ds:0x028A` waves, `ds:0x3F2E`.
* A Windows timer (`SetTimer` 50 ms, about 55 ms in practice) runs one
  dispatcher pass, `seg4:0x06BF`.

Three or four ticks fire per pass; a delay of 1 to 3 ticks still lasts a
whole pass. The port keeps both: `Director::pass(fires)` per pass,
`ticks_by_pass(n) = n * 55 / 16`.

One pass:

1. Entry under the cursor null: rebuild (`seg4:0x0540`). Unload, `gettime` to
   phase, `getdate` to holiday, island reset `seg3:0x1C41(-1, -1)`, story roll
   `seg4:0x0BA5`, build `seg4:0x08B9`, position = first row's `from`, cursor
   0, backdrop `seg3:0x0136`. The clock only moves here.
2. Backdrop pass `seg3:0x040E(0)`.
3. The entry. With bit 15 set, `seg4:0x0B64` writes the wanted area and
   facing and says whether Johnny is there; if not, the walk driver
   `seg4:0x0094` runs and the pass ends.
   * Idle row: first pass rolls `rand % 240 + 180` into `ds:0x334E` and
     `seg4:0x0074` clears route and path. Every pass runs the walk driver.
     When the slot is 0: position = row's `to`, cursor++.
   * Scripted row: first pass resolves and starts the sequence
     (`seg4:0x0C88`, `seg10:0x04FC`, `seg10:0x0874`), clears bit 15, sets the
     draw offset (`seg3:0x00FC`). Every later pass ticks the runner
     `seg10:0x1ACB` and asks `seg10:0x0766` whether it still runs. When not,
     the landing `seg4:0x081C`: position = row's `to` (zeros filled at
     random), bit 15 set, cursor++; if the story state `ds:0x30EE` is 2, the
     offer `ds:0x2D9B` advances and the state becomes 3.
4. `seg3:0x1E50(0)`. If the entry this pass handled is the story scene
   (`ds:0x30EC`): request sound 25, clear `ds:0x30EC`, set `ds:0x30EE` to 2.

The story is marked told on the pass that first takes up its entry, and the
story scene's own landing advances the offer.

`seg4:0x0B64`: an area of 0 skips the area test; the facing is still required
unless it is -1.

## Where the island stands

`ds:0x30C0` holds six placement bits. `seg3:0x1C41` reads them from the high
word of a scene mask:

| High bit | `ds:0x30C0` |
| --- | --- |
| `0x20` | `= 1`: pinned at (0x011, 0x135) |
| `0x10` | `= 2`: pinned at (0x121, 0x12B) |
| `0x01` | `\|= 4`: top-left quadrant |
| `0x02` | `\|= 8`: top-right quadrant |
| `0x04` | `\|= 0x10`: bottom-left quadrant |
| `0x08` | `\|= 0x20`: bottom-right quadrant |

The reset writes -1 (anywhere); the queue's masks OR into it. `seg4:0x0EDB`
gates: `0x15` is the three left bits, `0x2A` the three right. A row wanting
one side cannot join a queue that claimed the other; a row wanting one
quadrant cannot join one that claimed the opposite half.

`seg3:0x1B00` places it: pinned states return their spot, otherwise a uniform
random point in a rectangle, halved per quadrant bit (shift off the full
extent, then halve):

| Layout | x | y | width | height |
| --- | --- | --- | --- | --- |
| bare island | 0x043 | 0x0FF | 0x0E2 | 0x081 |
| with the sandbar | 0x0A8 | 0x0FF | 0x06F | 0x03B |

`seg3:0x0236` subtracts the nominal anchor (0x121, 0x117), leaving the offset
`ds:0x30C6`/`ds:0x30C4`, applied to:

* island pieces, surf, raft, palm (`seg3:0x0262` on)
* glint and wave exclusion rectangles (`seg3:0x08A5`, `seg3:0x0F05`), without
  the port's one-pixel drawing correction
* walk waypoints (`seg4:0x02EA`)
* scene drawing opcodes: `seg3:0x00FC` copies offset plus one into
  `ds:0x46E4`/`ds:0x46E6`; `seg12:0x05B9` adds it to the first coordinate pair
  of the `0xA000` family, `0x4004` and `0x4204`, and to the second pair of
  `0x4004` and `0xA0A4` (table `cs:0x1379`). `0xA601`, `0xAF1F`, `0xAF2F` get
  nothing. A cutaway (`ds:0x30B4` = 2) draws in absolute coordinates; low mask
  bit `0x400` adds 272 pixels to the right (`seg4:0x07DF`).

`ISLETEMP.SCR` is the island at the nominal anchor, the template the scripts
load.

## What else the mask asks for

Low word, `seg3:0x1C7C`; all ones (`seg3:0x1C4B`) resets the lot.

| Bit | Variable | Meaning |
| --- | --- | --- |
| `0x0010` | `ds:0x30B6 = 0` | no holiday decoration |
| `0x0200` | `ds:0x30C2 = 0` | no rock and no sandbar (17 rows) |
| `0x0400` | per scene | draw 272px right |
| `0x0800` | `ds:0x30B4 = 1` | night regardless of the hour |
| `0x1000` | `ds:0x30B2 = 1` | flat calm: `ds:0x2F40 = 0`, no waves and no glints |
| `0x2000` | `ds:0x30B4 = 2` | cutaway: no island, the scene's own `.SCR` is the picture (4 rows) |
| `0x4000` | `ds:0x30B0 = 1` | no raft (1 row) |

## Positions and walking

Position is `ds:0x30E4` (area 1-6) and `ds:0x30E0` (facing 0-7); wanted area
`ds:0x30E2`, wanted facing `ds:0x30DE`. `tools/scheduler.py --walk` dumps the
walk data.

### The walk driver, `seg4:0x0094`

Once per pass, in order:

1. `ds:0x30D6` nonzero: redraw the pose, re-arm 4, return.
2. Route lookup `seg4:0x03AC(current, target)` when there is no route and
   Johnny is not at the target; 0 if either area is 0.
3. Step pick: `rand() % 100` against the weights (`seg4:0x0115`), taken when
   the running count stops being positive. Turn goal = the step's starting
   facing; destination recorded.
4. Wanted facing (`seg4:0x0179`): once at the target facing the goal, goal =
   wanted; if already equal, set the arrived pose and clear wanted to -1.
5. Pivot one eighth (`seg4:0x01CA`) the short way, `rand() & 1` when dead
   opposite, drawn with the arrived or travelling pose.
6. Waypoint (`seg4:0x0264`) if facing == goal; arrival when the next triple is
   all zero. A finished pivot and the first waypoint happen in one pass.
7. Draw the pose `ds:0x30C8`, re-arm 4 (`seg4:0x02B2`).

The route block is chosen once, on the pair Johnny set out on, and re-entered
from wherever he has got to.

### Route tables

`seg4:0x03AC` reads a 6x6 matrix at `ds:0x0A94`: the word at
`0x0A94 + from*12 + to*2` is the offset of a route block. A block is a flat
list: `-position` marker, `(step, weight)` pairs, terminator -7. `(0, 0)`
under the target's marker means "already there". Every weight list sums to
100 except route 1 to 5 at position 1, which sums to 150:

```
step 4 w16, step 4 w17, step 3 w34, step 2 w33, step 1 w25, step 1 w25
```

Against `rand() % 100` the last two options are unreachable, and step 1 is
the only one that lands on the target:

| Step | Lands on | As shipped | As authored |
| --- | --- | --- | --- |
| 1 | 5 (the target) | 0% | 32.7% |
| 2 | 6 | 32% | 22.0% |
| 3 | 3 | 34% | 22.7% |
| 4 | 2 | 34% | 22.7% |

The port draws `rand() % total`, like `seg10:0x0CDA` and `seg4:0x0E62`;
`JC_FAITHFUL=1` keeps the shipped draw. `tools/verify.py` asserts this is the
only block affected. The scene chooser adds one to its draw and the router
does not, so boundary draws fall on different sides.

### Walk paths and poses

A step id resolves through stubs at `seg4:0x04FB` to one of 34 blocks,
`ds:0x0AEA` to `ds:0x162A`: `(frame, x, y)` triples ending in all zeros, `x`,
`y` on the 640x350 background, bit 15 of `frame` mirroring the sprite. Paths
are hand-authored, 5 to 32 waypoints. A parallel table at `ds:0x02DE`, 3 bytes
per step: starting facing, destination facing, destination area.

Twelve blocks are standing poses, eight entries indexed by facing
(`seg4:0x0258`). `ds:0x0347` and `ds:0x034E` hold one per area:

| Area | 1 | 2 | 3 | 4 | 5 | 6 |
| --- | --- | --- | --- | --- | --- | --- |
| arrived (`ds:0x0347`) | 6 | 10 | 17 | 22 | 28 | 34 |
| travelling (`ds:0x034E`) | 5 | 9 | 16 | 21 | 27 | 33 |

`ds:0x30C8` points at what to draw: a pose or the waypoint just reached, one
`JOHNWALK.BMP` frame that stays on screen between scenes.

## The story

State in `SCRANTIC.INI`:

| Key | Variable | Meaning |
| --- | --- | --- |
| `Introduction` | `ds:0x2D9B` | story day on offer |
| `NumDays` | `ds:0x2D9C` | story day reached |
| `CurrentYear/Month/Day` | `ds:0x2D99/97/95` | date of the last run |
| `StartTime` | `ds:0x00BE` | clock origin, ships at 900 |

Day rollover (`seg4:0x0BC8`, `seg4:0x0BDC`) when the stored date differs:

```
if (NumDays < Introduction) NumDays++;
if (NumDays > 11 || NumDays < 2) { Introduction = 1; NumDays = 1; }
store today's date
```

The offer advances at the story scene's landing, so it runs a step ahead and
the reached day catches up after a real day. The story wraps after eleven.
Sound 25 has no resource, so the story cue is silent in the original too.

| Day | Scene | Day | Scene |
| --- | --- | --- | --- |
| 1 | `MARY.ADS` seq 2 | 7 | `MARY.ADS` seq 4 |
| 2 | `JOHNNY.ADS` seq 2 | 8 | `MARY.ADS` seq 5 |
| 3 | `SUZY.ADS` seq 1 | 9 | `SUZY.ADS` seq 2 |
| 4 | `MARY.ADS` seq 3 | 10 | `JOHNNY.ADS` seq 6 |
| 5 | `MARY.ADS` seq 1 | 11 | `JOHNNY.ADS` seq 1 |
| 6 | `JOHNNY.ADS` seq 3 | | |

## Time of day

`seg4:0x0554`, in half hours: `seg4:0x0C48` turns HHMM into
`hhmm / 50 + (hhmm % 100) / 30`, and phase = (now - `StartTime`) mod 16 into
`ds:0x30BA`. Phases 0-11 are day (`OCEAN00/01/02.SCR` at random per backdrop
build), 12-15 night (`NIGHT.SCR`). Scene windows `+0x08`/`+0x09` use these
units.

## Holidays

`seg4:0x0560` writes an id to `ds:0x30BC`. `seg3:0x1E50` draws `HOLIDAY.BMP`
when it is non-zero, unless `ds:0x30B6` is 0 or the scene is a cutaway;
`seg3:0x1E87` indexes `cs:0x1F67` by `id - 1`.

| Date | Id | Frame | Where | Redraw size |
| --- | --- | --- | --- | --- |
| 29-31 October (Halloween) | 1 | 0 | (0x19B, 0x12A) | 40x34 |
| 23-25 December (Christmas) | 2 | 2 | (0x195, 0x10B) | 56x65 |
| 29-31 December, 1 January (New Year) | 3 | 3 | (0x16A, 0x09B) | 152x47 |
| 15-17 March (St Patrick's) | 4 | 1 | (0x14E, 0x11E) | 120x47 |

## The clock

`seg2:0x1516` calls `seg9:0x076A` with 13: `1000 / (13 * 18)` = 4 ms per unit
of `ds:0x45E0`; the one timer (`seg2:0x1522`) at 4 units runs `seg2:0x16CE`
every 16 ms. Script delays are in ticks (60 is about a second), but frames
only advance on the 55 ms pass, so the original never showed more than 18
frames a second. `JC_SPEED` is a percentage of the authored rate.

| Consumer | Slot | Re-armed to |
| --- | --- | --- |
| walk waypoint or pivot | `ds:0x30D6` | 4 |
| idle row | `ds:0x334E` | `rand % 240 + 180` |
| surf phase | `ds:0x0288` | 4 |
| wave movement | `ds:0x028A` | 10 |
| animation frame | deadline at instance `+0x37` against `ds:0x2E72` | the frame's delay |

## How long a scene lasts

`PlayScene`'s third operand (`seg10:0x0DDB`, signed):

| Operand | State | Meaning |
| --- | --- | --- |
| `0` | 1 | play the program through |
| `> 0` | 2, count at `+0x31` | play it that many times |
| `< 0` | 3, deadline at `+0x33` | play until `tick + \|operand\|` |

`seg10:0x1C56` runs an instance in state 1, 2, 3 or 5 only. At the end of its
program (`seg10:0x1D2B`) a scene restarts: state 1 becomes 4; state 2 until
the count runs out; state 3 until the deadline (`seg10:0x1DEB` promotes to 4).
State 4 becomes 0 on the next pass (`seg10:0x1DE2`), so it lasts one pass and
an operand of 0 runs the program twice. State 0 is `StopScene`; state 5 is
`FinishScene`, unused.

* TTM `0x0110` (`seg12:0x0F73`) sets `ds:0x4644` and the frame finishes
  drawing. All 528 uses are scene terminators.
* A goto to the scene's own start sets a self-loop flag at `+0xC`
  (`seg10:0x1D9E`); `seg10:0x07E3` ignores such instances when asking whether
  a sequence is done.
* `GotoFrame` (`seg12:0x0FC3`) stashes the target; it is taken at the frame
  boundary.

## The island itself

Screen 640x480 (`seg2:0x1483` into `ds:0x2562`/`ds:0x2564`); scene backdrops
are 640x350, sea screens 640x480. `seg3:0x0136` loads bitmaps on its first
call and builds on later ones:

1. Sea: `OCEANnn.SCR` by `rand % 3` in daylight and not a cutaway;
   `NIGHT.SCR` otherwise.
2. Coin `rand & 1` into `ds:0x30C2` (rock and sandbar) unless a scene said
   no rock.
3. Place, `seg3:0x1B00`. Then the island pieces.
4. Surf reset unless a cutaway: `ds:0x0288 = 0`, counters
   `ds:0x0282..0x0286 = 2`, one step.
5. Object count `ds:0x2F40` = 30, or 0 for flat calm or waves off; object
   array reset `seg3:0x0463`.
6. Cloud count `ds:0x2F36` = 15 or 0; sky `seg3:0x0A38`.
7. One `seg3:0x040E(1)` pass; holiday `seg3:0x1E50(1)`.

Pieces from `BACKGRND.BMP` (handles `ds:0x30AC`):

| Frame | Size | At | Drawn at | What |
| --- | --- | --- | --- | --- |
| 0 | 280x52 | (289, 279) | `seg3:0x0270` | the island |
| 1 | 384x69 | (250, 303) | `seg3:0x0293` | sandbar, when `ds:0x30C2` |
| 2 | 64x30 | (151, 328) | `seg3:0x02B0` | rock, when `ds:0x30C2` |
| 14 | 104x28 | (397, 279) | `seg3:0x02EA` | shells, see below |
| 13 | 24x145 | (443, 148) | `seg3:0x0307` | palm trunk |
| 12 | 152x69 | (366, 122) | `seg3:0x0324` | fronds |

The sandbar layout adds four surf patches, its own placement rectangle
(`seg3:0x1B56`) and raft spot (`seg3:0x1DCB`). Surf, three frames per patch:

| At | Frames | Size | Counter | Set up at |
| --- | --- | --- | --- | --- |
| (271, 306) | 3-5 | 72x29 | 0 | `seg3:0x13A0` |
| (365, 319) | 6-8 | 160x25 | 1 | `seg3:0x13C9` |
| (519, 303) | 9-11 | 72x32 | 2 | `seg3:0x13F2` |
| (234, 323) | 30-32 | 120x48 | 0 | `seg3:0x141B`, sandbar only |
| (368, 356) | 33-35 | 176x28 | 1 | `seg3:0x1444`, sandbar only |
| (559, 323) | 36-38 | 88x48 | 2 | `seg3:0x146D`, sandbar only |
| (130, 340) | 39-41 | 104x29 | 0 | `seg3:0x1496`, rock ripple |

Counters `ds:0x0282`, `ds:0x0284`, `ds:0x0286`; one moves per step
(`seg3:0x0E9C`), a nine-step cycle:

```
if c0 == c2      { c0 += 1 }
else if c1 != c0 { c1 += 1 }
else             { c2 += 1 }
each wraps 3 -> 0
```

**Raft**: `MRAFT.BMP` (`seg3:0x0179`, handle `ds:0x30AE`), five frames 72x39
to 128x39. `seg3:0x1D7E` takes `ds:0x2D9C - 1` clamped to 1..5. At (513, 266),
or (530, 281) with the sandbar. `MARY.ADS` sequence 5 turns it off with mask
bit `0x4000` because the scene animates it.

**`ISLETEMP.SCR`** is a render of the bare-island composition with the surf
counters at zero. `jcisle` (`tools/check.sh`) scores each piece where the code
puts it:

| Piece | Score | Floor |
| --- | --- | --- |
| frame 0, the island | 96.5% | 90% |
| frame 13, the palm trunk | 95.2% | 90% |
| frame 12, the fronds | 90.5% | 85% |
| surf at `0x10F` | 75.2% | 60% |
| surf at `0x16D` | 75.3% | 60% |
| surf at `0x207` | 34.0% | not checked |
| the whole composition | 92.3% | 80% |

Matching pieces sit one pixel left of the nominal anchor, so the port draws
with `ORIGIN = (-1, 0)`. Frame 14 (shells) is nowhere in the reference (best
28%; composition 81.1% without it, 76.6% with), so the port omits it;
`JC_FAITHFUL=1` draws it. The third surf patch peaks at `0x209` at 54.7%; the
port keeps the code's `0x207`.

**Index 0 in the sea screens.** `OCEAN00/01/02.SCR` draw about 10,000 pixels
each with index 0, the transparency key, in patches of open water; all of
`OCEAN00`'s 9,917 are index 9 in `OCEAN01`. No palette animation is imported.
`JOHNCAST.PAL` swaps 0 (magenta) and 5 (black); `seg3:0x01C4` clears to 5.
The port fills them with index 9. What the engine drew is open.

### The per-pass backdrop, `seg3:0x040E(0)`

Nothing for a cutaway. Otherwise:

* Wave walk `seg3:0x04A2` over all 30 slots of `ds:0x2F42` (12-byte records).
  Empty slot: 35% roll (`seg3:0x04CE`), then weighted 35/5/5/5
  (`seg3:0x04EE`) for a glint or wave size 1-3. Movement only when
  `ds:0x028A` is 0, re-armed to 10 afterwards.
* The cloud walk is skipped: only the setup call `seg3:0x040E(1)` runs it, so
  clouds never move in the original. The port runs it every pass;
  `JC_FAITHFUL=1` leaves the sky still.
* Surf steps when `ds:0x0288` is 0 and re-arms to 4, unless flat calm.

Nothing spawns unless the speed index `ds:0x30BE` (a calibration loop at
`seg2:0x1600`) is over 100 (`seg3:0x04E4`). `seg4:0x1F1B` is the weighted
chooser: percentage arguments, scaled by 655, walked against a 16-bit draw.

Waves are four-frame `BACKGRND.BMP` ping-pongs, sequences terminated by
`0xFF`, caps from `seg3:0x046E`:

| Kind | Sequence | Frames | Size | At once | Spawn band | Set up at |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | `ds:0x0278` | 18-21 | 32x6 | 4 | x -10 +650, y 249 +40 | `seg3:0x05CA` |
| 2 | `ds:0x0270` | 22-25 | 64x12 | 2 | x -40 +680, y 289 +93 | `seg3:0x0627` |
| 3 | `ds:0x0268` | 26-29 | 104x16 | 1 | x -70 +710, y 382 +142 | `seg3:0x0682` |

`seg3:0x14BF` picks `base + rand() % span` with no size subtraction for waves.
A wave holds each frame 0, 1 or 2 steps (`seg4:0x1F1B(30, 50)`) and each step
adds the wind `ds:0x0280` (+1 or -1) to x. Waves fully inside one of three
hard-coded surf rectangles per layout are not drawn.

Glints (`seg3:0x07DE`): colour 15, a line from x to x+1, y in 214..249
(`seg3:0x053C`), countdown `rand % 30 + 10` rolled before the spot. One
landing on either palm rectangle (`seg3:0x055E` trunk, `seg3:0x058A` fronds)
is thrown away.

### The sky

Fifteen slots at `ds:0x2E82`, `BACKGRND.BMP` frames 15, 16, 17:

| Frame | Size | At most | Cap at |
| --- | --- | --- | --- |
| 15 | 128x36 | 8 | `ds:0x2E7C` |
| 16 | 192x57 | 4 | `ds:0x2E7E` |
| 17 | 264x76 | 2 | `ds:0x2E80` |

Setup `seg3:0x0A38`: wind 50% into `ds:0x0280` (`seg3:0x0A59`, shared with
the waves); per slot a 30% gate (`seg3:0x0A8E`) then 5/10/15 biggest first,
falling through at a cap (`seg3:0x0AC9`); x anywhere, y in the top 139 rows
(`seg3:0x0B0C`); speed `rand % 5 + 6` passes per pixel (`seg3:0x0B2B`);
countdown `rand % speed`. Walk `seg3:0x0B67`: refill empty slots 20/30/50 by
size, 1,000 pixels off screen upwind (`seg3:0x0C28`); move each cloud whose
countdown ran out one pixel; retire one off the far side (`seg3:0x0D19`).

## The ADS runner, `seg10:0x1ACB`

Once per dispatcher pass, on the context at `ds:0x2608`:

1. `seg10:0x1B12`: each sequence's null-terminated instance array at
   `ctx+0x3D2+sid*2` (built by `seg10:0x040F` from `2000`/`2005`). A sequence
   in state 3 (how `seg4:0x07CE` starts one) has its records reset by
   `seg10:0x0B3E`.
2. `seg10:0x1B6B`: one pass of every sequence in state 1, in id order,
   through `seg10:0x1925`.
3. `seg10:0x1C32`: the instance walk from `ctx+0x10`, chained at `+0x3B`.
   Each record selects its own TTM (`seg10:0x1C71`; bitmap slots are per
   file, `seg12:0x0773`), restores colours and clip (`seg10:0x1C81`), runs its
   frame (`seg11:0x074A` into `seg12:0x040C`).

The script runs before the instances, so a scene started this pass draws its
first frame this pass, and a scene that ended last pass is still in state 4.

A held frame runs again every pass with `ds:0x46DA` set (`seg10:0x1CD6` sets
`[bx+0x0E]`). Opcodes with side effects (loads, `0080`, the `C` family,
`2022`, `B606`, `1061`) test the flag and do nothing; drawing opcodes draw
again, re-reading their bitmap slot.

Stepping (`seg10:0x196E`): `+0x29` zero steps at once; otherwise step when
`ds:0x2E72 >= +0x37`, next deadline `now + delay`. On a step `[bx+0x0E] = 0`
and `[bx+8] = [bx+6]` if a goto is pending, else `[bx+8]++`. Scene end uses
the same test (`seg10:0x1D65`). `seg10:0x1CE6` re-bases the deadline only when
the frame changed the delay (`ds:0x46E8` differs from `+0x29`).

`tools/x86.py` drives `seg12:0x040C` directly (`x86.Machine.wall` answers
every call leaving segments 1 and 12). Loaded-TTM record at `ds:0x2638`:
`+0x00` id, `+0x02` frame count, `+0x04`/`+0x06` bytes, `+0x08`/`+0x0A` last
byte, `+0x0C..+0x2B` sixteen bitmap slots, `+0xCC`/`+0xCE` next,
`+0xD0`/`+0xD2` frame table (`seg11:0x04D6`, one instance record per `1111`).
At `seg12:0x0620` an opcode is complete: `ds:0x46DC` opcode (`1301` rewritten
to `C051`, `1311` to `C061`), `[bp-0x26]` count, `[bp-4]` downward operands,
`[bp-0x1A]` string. A frame index past a sheet's end is unchecked
(`seg12:0x0281`; eleven shipped draws); the harness pads the table so
`seg12:0x0291` takes the no-shape path.

## Every opcode, audited

Thirty TTM opcodes and eighteen ADS ones are used (ADS: `docs/FILE_FORMATS.md`).

| TTM | Uses | What it does |
| --- | --- | --- |
| `0FF0` `1021` `2022` `1201` `0110` | 6631, 1148, 152, 75, 528 | frame boundary, delay, random delay, goto, scene end |
| `F01F` `F02F` `F05F` `1051` `1061` `0080` | 40, 263, 41, 456, 41, 190 | load screen / bitmap / palette, slot select, free |
| `A504` `A524` | 12546, 2822 | sprite, unmirrored and mirrored |
| `A0A4` `A104` `A002` | 784, 111, 63 | line, filled rect, pixel |
| `A404` | 437 | circle: `seg12:0x01C9` halves the fourth operand for a radius, centres at `(x + r, y + r)`, ignores the third; `seg9:0x0688` calls `Ellipse`, far edges exclusive |
| `2002` `4004` | 217, 52 | colours, clip |
| `A054` `A064` `B606` | 1, 1, 6 | region save and restore |
| `C051` | 535 | sound |
| `1101` `1111` `5005` | 5, 611, 0 | scene markers; only `1111` gets an instance record (`seg11:0x0645`) |
| `4214` | 60 | no-op (`seg12:0x0860`) |
| `A601` | 6814 | no-op (`seg12:0x02D8`) |
| `1121` | 61 | writes `[bx+0x16]`, read only by the unused `00C0` |
| `2012` | 52 | next bitmap load builds pre-flipped copies (`seg9:0x018A`); the port mirrors at draw time |
| `4204` | 38 | page-to-page region save when `ds:0x2641` says three pages; nothing on one surface |

## The decorations, and the title card

`seg2:0x14F0` shows `INTRO.SCR` while loading the backdrop (`seg3:0x0136`),
walk sprites (`seg4:0x0000`) and sounds (`seg5:0x0000`). The port holds the
card two seconds; `JC_INTRO` in ticks, 0 to skip.

## The sound

Twenty-three `WAVE` resources in `SCRANTIC.EXE`, `WAVESFX1` upwards, 8-bit
mono PCM at 11025 Hz. `seg5:0x001A` loads them at startup (`FindResource`,
`LoadResource`, `LockResource`) into far pointers at `ds:0x355A`, looking for
25 and finding 23. `seg5:0x00BA` bounds-checks the id and calls
`sndPlaySound` with `SND_ASYNC | SND_NODEFAULT | SND_MEMORY`: one sound at a
time. Scripts ask for 1 to 24, the story cue for 25. `ds:0x00BC` is the
`Sounds` switch from `SCRANTIC.INI`.

**Clip and colours are per instance.** `4004` writes the live clip at
`ds:0x1EA3..0x1EAA` (blitter reads it at `seg8:0x1ADE`) and a copy at instance
`+0x20` (`seg12:0x0F40`); `2002` stores colours at `+0x1E`/`+0x1F`
(`seg12:0x06A9`; default 15 from `seg10:0x0B6E`). `seg10:0x1C81` restores both
before every frame; `seg10:0x1E2F` resets the globals after the walk.

## In the port

`tools/gen_tables.py` emits the tables as `johnny/src/tables.rs`.
`johnny/src/director.rs` holds the queue, chooser, walk driver and clocks;
`johnny/src/scenery.rs` the island; `jcisland` runs the lot. `Clock::Wall` is
the shipped clock; the tools default to `Clock::PerQueue`, one phase per queue.

* Johnny is redrawn after the backdrop every pass.
* A held frame is repainted from resolved marks (`ttm::Paint`), not by
  re-running its instructions: 66 shipped frames load or free artwork and draw
  with it in the same frame. `JC_CHECK=1` verifies this every pass.
* `F200 n` starts sequence `n` alongside (state 4, `seg10:0x16C2`); every
  `STAND.ADS` sequence opens with `F200 14`, "STAND INIT". `F010 -1` puts the
  current sequence into state 2.

## Still open

* What the engine draws in the sea screens' index-0 patches.
* The blitter's pixel loop: `seg8:0x0AE4` hands the blit to GDI. Its scale
  path (`seg8:0x0B02`, `seg8:0x0B26`) is dead because `seg2:0x148F` fixes
  `ds:0x2E76` and `ds:0x2E77` at 3. The mirror is a second bitmap pair at
  `[si+0x0A]`/`[si+0x0C]` built by `2012`; `seg8:0x0B97` falls back to the
  plain pair at `[si+2]`/`[si+4]`.
