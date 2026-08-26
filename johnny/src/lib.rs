//! A native reimplementation of the 1992 Dynamix/Sierra screensaver
//! *Johnny Castaway* (internally `SCRANTIC`, "Screen Antics").
//!
//! Everything here is derived from a disassembly of the original 16-bit
//! executable rather than from watching it run; see `docs/` in the repository
//! for the write-up. The original data files are not redistributed — the
//! engine reads `RESOURCE.MAP`, `RESOURCE.001` and `SCRANTIC.EXE` from a copy
//! you supply.

pub mod artpack;
pub mod ads;
pub mod dgds;
pub mod director;
pub mod png;
pub mod render;
pub mod scenery;
pub mod settings;
pub mod sound;
pub mod tables;
pub mod ttm;

#[cfg(test)]
mod tests;
