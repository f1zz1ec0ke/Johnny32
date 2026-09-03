# Prior art

## `jc_reborn`

[jno6809/jc_reborn](https://github.com/jno6809/jc_reborn): C + SDL2, GPL-3.0.
Builds on Linux and Windows (MinGW), 32- and 64-bit. Runs from the original
`RESOURCE.MAP` and `RESOURCE.001`, interprets TTM and ADS, and handles scene
selection, walking transitions, clouds and island placement. Its own
assessment: every scene works "with only some inaccuracies here and there",
particularly the walking transitions. Behaviour was reconstructed by watching
the screensaver, not by disassembling the executable. A fork with more
platform ports: [sizious/johnny-castaway-reborn](https://github.com/sizious/johnny-castaway-reborn).

## Others

* [xesf/castaway](https://github.com/xesf/castaway): JavaScript, browser,
  early-stage (no day/night cycle, tides or story). Documents the resource
  index format only.
* [deckarep/Johnny-Castaway-2026-Public](https://github.com/deckarep/Johnny-Castaway-2026-Public):
  Go + Raylib, desktop and WebAssembly. Parses the assets and interprets
  ADS/TTM. Credits `jc_reborn` as its foundation.

## ScummVM

ScummVM's DGDS engine covers the Dynamix adventure games (Rise of the Dragon,
Heart of China, Willy Beamish), which share this container format. Johnny
Castaway's ADS/TTM usage is a subset of what that engine targets; check the
engine's current state before assuming it runs there.

## What this repo adds

None of the above published a disassembly of the 81,919 bytes of 16-bit code
in `SCRANTIC.EXE`. This repo reads the scheduler, the walk data, the story
state machine, the day/night timing, the island and the codecs out of it
(`docs/SCHEDULER.md`, `docs/FILE_FORMATS.md`) and checks the port against the
original code with `tools/oracle.py`.
