//! Artwork with more than sixteen colours, laid over the archive's.
//!
//! Everything in `RESOURCE.001` is four-bit: all 2,392 sprite frames and all
//! ten backdrops use palette indices 0 to 15 and nothing else. That is the
//! banding, and it is the whole of what a 1992 VGA screensaver could afford.
//!
//! An art pack is a directory of PNGs named after the record and the frame
//! they stand in for:
//!
//! ```text
//! art/
//!   JOHNWALK.BMP/000.png   art/ISLETEMP.SCR.png
//!   JOHNWALK.BMP/001.png   art/OCEAN00.SCR.png
//! ```
//!
//! `tools/export_assets.py` already writes the originals out in exactly that
//! shape, so redrawing one is a matter of opening it and painting over it.
//! Anything not replaced falls back to the archive, which means a pack can be
//! delivered a frame at a time and each one lands the moment it exists.
//!
//! Replacements keep the original's **size and origin**: the scripts position
//! every sprite themselves, in coordinates that assume the frame is the size
//! it was. Drawing at twice the resolution is a change to the surface as well
//! as to the art, and this is not that -- it is the same picture with the
//! colours it was always meant to have.

use crate::dgds::Frame;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct ArtPack {
    root: PathBuf,
    /// Record name -> the frames replaced, by index.
    frames: HashMap<String, HashMap<usize, Frame>>,
    /// How many were read, and how many were passed over and why.
    pub loaded: usize,
    pub rejected: Vec<String>,
}

impl ArtPack {
    /// Read every PNG under `root`. A missing directory is not an error: it
    /// is the ordinary case, and the archive's own art is the answer.
    pub fn load(root: &Path) -> ArtPack {
        let mut pack = ArtPack {
            root: root.to_path_buf(),
            frames: HashMap::new(),
            loaded: 0,
            rejected: Vec::new(),
        };
        let Ok(entries) = std::fs::read_dir(root) else {
            return pack;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if path.is_dir() {
                let record = name.to_ascii_uppercase();
                let Ok(inner) = std::fs::read_dir(&path) else {
                    continue;
                };
                for f in inner.flatten() {
                    let p = f.path();
                    let Some(stem) = p.file_stem().and_then(|n| n.to_str()) else {
                        continue;
                    };
                    // Case-insensitively, the way the top-level arm below
                    // does it: an artist saving `000.PNG` out of a paint
                    // program should not find it silently ignored in one
                    // place and honoured in the other.
                    let png = p
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
                    if !png {
                        continue;
                    }
                    let Ok(index) = stem.parse::<usize>() else {
                        continue;
                    };
                    pack.take(&record, index, &p);
                }
            } else if name.to_ascii_lowercase().ends_with(".png") {
                // A single-frame record: `ISLETEMP.SCR.png`.
                let record = name[..name.len() - 4].to_ascii_uppercase();
                pack.take(&record, 0, &path);
            }
        }
        pack
    }

    fn take(&mut self, record: &str, index: usize, path: &Path) {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                self.rejected.push(format!("{}: {e}", path.display()));
                return;
            }
        };
        match crate::png::read(&bytes) {
            Ok(img) => {
                self.frames.entry(record.to_string()).or_default().insert(
                    index,
                    Frame {
                        width: img.width,
                        height: img.height,
                        // `Frame` carries an indexed buffer as well as a
                        // colour one, and this keeps the two the same length.
                        // Only `colour` is ever read back out of a pack --
                        // `apply` copies that and leaves the original's
                        // indices alone -- so this is the invariant and not
                        // a picture of anything.
                        pixels: img
                            .pixels
                            .iter()
                            .map(|p| if p >> 24 == 0 { crate::render::TRANSPARENT } else { 15 })
                            .collect(),
                        colour: Some(img.pixels),
                    },
                );
                self.loaded += 1;
            }
            Err(e) => self.rejected.push(format!("{}: {e}", path.display())),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Swap in whatever this pack has for a record's frames.
    ///
    /// A replacement has to be the size the original was, because the scripts
    /// place every sprite by hand and in the original's coordinates. One that
    /// is not is refused by name rather than drawn in the wrong place.
    pub fn apply(&self, record: &str, frames: &mut [Frame]) -> usize {
        let Some(mine) = self.frames.get(record) else {
            return 0;
        };
        let mut n = 0;
        for (index, art) in mine {
            let Some(slot) = frames.get_mut(*index) else {
                continue;
            };
            if art.width != slot.width || art.height != slot.height {
                continue;
            }
            slot.colour = art.colour.clone();
            n += 1;
        }
        n
    }

    /// The same, for a record that is one image rather than a list.
    pub fn apply_one(&self, record: &str, frame: &mut Frame) -> bool {
        let Some(art) = self.frames.get(record).and_then(|m| m.get(&0)) else {
            return false;
        };
        if art.width != frame.width || art.height != frame.height {
            return false;
        }
        frame.colour = art.colour.clone();
        true
    }

    /// Every replacement that is the wrong size for what it stands in for --
    /// the one mistake an artist will actually make, so it is worth being
    /// able to report rather than silently ignoring.
    pub fn mismatches(&self, record: &str, frames: &[Frame]) -> Vec<String> {
        let Some(mine) = self.frames.get(record) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (index, art) in mine {
            match frames.get(*index) {
                None => out.push(format!("{record}/{index:03}: no such frame")),
                Some(slot) if art.width != slot.width || art.height != slot.height => {
                    out.push(format!(
                        "{record}/{index:03}: {}x{} where the original is {}x{}",
                        art.width, art.height, slot.width, slot.height
                    ));
                }
                Some(_) => {}
            }
        }
        out.sort();
        out
    }
}
