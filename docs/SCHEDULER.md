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

## Still open

* What the two position components mean geometrically. Major 0-6 and minor
  0-7 look like an island region and a spot within it, but that is inference
  from the tables rather than something the code states. The waypoint
  coordinates should settle it once they are plotted over a background.
* `seg3:0x1C41`, which the picker hands the chosen entry's mask to.
* The exact cadence of the outer loop — how often a new scene is requested and
  what refills the caller's time budget.
