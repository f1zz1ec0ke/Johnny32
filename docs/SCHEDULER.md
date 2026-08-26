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

Every entry declares the position Johnny must be standing in (`+0x03`,`+0x04`)
and where he ends up (`+0x05`,`+0x06`). 43 of the 80 entries move him. When the
chosen scene starts somewhere he is not, the scheduler inserts a transition —
that is what `STAND.ADS` and `WALKSTUF.ADS` are for, and what the id-255
entries (a bare position change with no ADS) cover.

32 distinct positions appear, major 0-6 and minor 0-7. `(0,0)` is the
"anywhere" position that self-contained gags return to.

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

* The exact walk-transition search — which intermediate positions the
  scheduler steps through when the target is several positions away.
* What the two position components mean geometrically. Major 0-6 and minor
  0-7 look like an island region and a spot within it, but that is inference
  from the table, not from the code.
* `seg3:0x1C41`, which the picker hands the chosen entry's mask to.
