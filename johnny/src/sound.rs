//! The sound effects, which live inside `SCRANTIC.EXE` rather than the
//! archive.
//!
//! `RESOURCE.001` has no audio in it: 117 bitmaps, 41 TTM scripts, ten
//! screens, ten ADS scripts, a preload list and a palette. The twenty-three sound
//! effects are Windows resources in the 16-bit executable's own resource
//! table, and the screensaver pulls them out at startup: `seg5:0x001A`
//! formats `"WAVESFX%d"` for 1 to 25, hands each to `FindResource` with the
//! type `"WAVE"`, and `LoadResource`/`LockResource` leave a far pointer in the
//! table at `ds:0x355A`.
//!
//! Two of the twenty-five are not there, which is why the loader tolerates a
//! miss: the resources are 1 to 23. The scripts ask for 24, and the scheduler
//! asks for 25 when the story scene finishes (`seg4:0x0891`), so both of
//! those are silent in the original as well -- there is nothing missing here
//! to go and find.
//!
//! Playing one is `seg5:0x00BA`: bounds-check the id, look the pointer up, and
//! call `sndPlaySound` with flags 7 — `SND_ASYNC | SND_NODEFAULT |
//! SND_MEMORY`. Asynchronous and from memory, and because `sndPlaySound`
//! replaces whatever is playing, the island only ever makes one noise at a
//! time. `ds:0x00BC` is the `Sounds` switch from `SCRANTIC.INI`; when it is
//! off the loader does not even run.
//!
//! This module only finds and hands back the bytes. Making a noise with them
//! is the host's job, because it is the one platform-specific thing left.

/// The `.WAV` blobs, indexed by the id the scripts use. Slot 0 is never used.
pub struct Sounds {
    waves: Vec<Option<Vec<u8>>>,
}

/// The last sound id the loader looks for (`seg5:0x0077` compares against
/// 25). Ids start at 1; slot 0 is never used.
pub const LAST: u16 = 25;

impl Sounds {
    /// Pull the `WAVE` resources out of a 16-bit NE executable.
    ///
    /// The resource table is walked rather than the file scanned for `RIFF`,
    /// so ids come from the table and match what the scripts ask for. Every
    /// entry whose payload does not start with `RIFF` is passed over — the
    /// same table holds the icons, the dialogs and the string table.
    pub fn from_exe(exe: &[u8]) -> Option<Sounds> {
        let mut waves: Vec<Option<Vec<u8>>> = (0..=LAST as usize).map(|_| None).collect();
        let ne = u16::from_le_bytes([*exe.get(0x3C)?, *exe.get(0x3D)?]) as usize;
        if exe.get(ne..ne + 2)? != b"NE" {
            return None;
        }
        let word = |at: usize| -> Option<u16> {
            Some(u16::from_le_bytes([*exe.get(at)?, *exe.get(at + 1)?]))
        };
        let table = ne + word(ne + 0x24)? as usize;
        // Everything in the table is counted in units of 1 << shift.
        let shift = word(table)? as u32;
        let mut p = table + 2;
        loop {
            let kind = word(p)?;
            if kind == 0 {
                break;
            }
            let count = word(p + 2)? as usize;
            p += 8;
            for _ in 0..count {
                let offset = (word(p)? as usize) << shift;
                let length = (word(p + 2)? as usize) << shift;
                let id = word(p + 6)?;
                p += 12;
                // The high bit marks an integer id; a name-keyed resource is
                // not one of these and there are none in this file.
                if id & 0x8000 == 0 {
                    continue;
                }
                let id = id & 0x7FFF;
                let Some(body) = exe.get(offset..offset + length) else {
                    continue;
                };
                if !body.starts_with(b"RIFF") {
                    continue;
                }
                if let Some(slot) = waves.get_mut(id as usize) {
                    *slot = Some(trim_riff(body));
                }
            }
        }
        if waves.iter().all(|w| w.is_none()) {
            return None;
        }
        Some(Sounds { waves })
    }

    /// The `.WAV` for a sound id, if there is one. `seg5:0x00CF` bounds the id
    /// to 1..25 and checks the pointer, so an id with no resource is silence
    /// rather than a mistake.
    pub fn wave(&self, id: u16) -> Option<&[u8]> {
        self.waves.get(id as usize)?.as_deref()
    }

    /// How many of them were found, for the tools to report.
    pub fn count(&self) -> usize {
        self.waves.iter().filter(|w| w.is_some()).count()
    }
}

/// Cut a resource down to the RIFF chunk it declares.
///
/// Resources are padded out to the table's alignment — 512 bytes here — and
/// the trailing zeros are not part of the file. Windows does not mind, but a
/// decoder handed the padding may.
fn trim_riff(body: &[u8]) -> Vec<u8> {
    let declared = body
        .get(4..8)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize + 8);
    match declared {
        Some(n) if n <= body.len() => body[..n].to_vec(),
        _ => body.to_vec(),
    }
}
