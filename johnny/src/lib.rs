//! A native reimplementation of the 1992 Dynamix/Sierra screensaver
//! *Johnny Castaway* (internally `SCRANTIC`, "Screen Antics").
//!
//! Everything here is derived from a disassembly of the original 16-bit
//! executable rather than from watching it run; see `docs/` in the repository
//! for the write-up. The original data files are not redistributed — the
//! engine reads `RESOURCE.MAP` and `RESOURCE.001` from a copy you supply.

pub mod dgds;
