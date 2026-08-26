# Prior art

Worth knowing before choosing an approach — a working native engine already
exists, and it is not the same thing as what this repo is set up to do.

## `jc_reborn` — the mature one

[jno6809/jc_reborn](https://github.com/jno6809/jc_reborn), C + SDL2, GPL-3.0.
Builds on Linux and Windows (MinGW), **32- and 64-bit**. Feed it the original
`RESOURCE.MAP` and `RESOURCE.001` and it runs. It interprets the TTM and ADS
scripts properly rather than hardcoding scenes, and handles scene selection,
walking transitions, clouds and island placement.

Its own assessment is that every scene works "with only some inaccuracies here
and there", and that "more needs to be done to fully understand a number of
details and faithfully reproduce the behaviour of the original engine" —
particularly the walking transitions.

The important part for us: **they reverse engineered by observing the running
screensaver, not by disassembling the executable.** That is precisely the gap
this repo is positioned to close.

A fork with extra platform ports lives at
[sizious/johnny-castaway-reborn](https://github.com/sizious/johnny-castaway-reborn).

## Others

* [xesf/castaway](https://github.com/xesf/castaway) — JavaScript, runs in a
  browser, early-stage with a long list of unimplemented features
  (day/night cycle, tides, story sequences). Documents the resource index
  format only.
* [deckarep/Johnny-Castaway-2026-Public](https://github.com/deckarep/Johnny-Castaway-2026-Public)
  — Go + Raylib, desktop and WebAssembly. Parses the original assets and
  interprets ADS/TTM. Credits `jc_reborn` as its foundation.

## ScummVM

ScummVM has a DGDS engine for the Dynamix adventure games (Rise of the Dragon,
Heart of China, Willy Beamish), which share this container format. Johnny
Castaway is a screensaver rather than a game and has repeatedly been treated as
an awkward fit; its ADS/TTM usage is a subset of what the engine targets.
Do not assume it runs there without checking the current state of the engine.

## What is actually unexplored

Every project above reconstructs behaviour by watching the original. Nobody has
published a disassembly of the 81,919 bytes of 16-bit code in `SCRANTIC.EXE`,
which is where the scheduler, the story state machine and the day/night timing
actually live.

That is what this repo now has. `docs/SCHEDULER.md` has the 80-entry scene
table with its weights, positions, time windows and story days, read out of
segment 4 rather than inferred — including things that are close to
unguessable from watching, like the story scene being offered on a one-in-ten
roll, and the story advancing only on real elapsed days.

Two smaller demonstrations that reading beats guessing: the LZW codec's
block-reading behaviour is not visible in the data at all and makes every
compressed chunk decode exactly, and the `TT3:` chunks turn out to be
RLE-compressed in a way that leaves them looking uncompressed.
