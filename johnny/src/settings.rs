//! `SCRANTIC.INI`, and the state the original kept in it.
//!
//! `seg2:0x0C90` reads ten settings out of the section
//! `[ScreenSaver.ScreenAntics]` with `GetPrivateProfileInt`, and eleven
//! `WritePrivateProfileString` calls put them back. Two of them are not
//! settings at all:
//!
//! * **`Introduction`** is `ds:0x2D9B`, the story day currently on offer.
//! * **`NumDays`** is `ds:0x2D9C`, the story day reached.
//!
//! Together with `CurrentYear` / `CurrentMonth` / `CurrentDay` -- the date
//! those last moved on -- that is how an eleven-day story survives the
//! machine being turned off. A port that keeps them only in memory starts
//! Johnny's story from the beginning every time the screensaver comes on,
//! which is not a story.
//!
//! The file is read from beside the executable. An original `SCRANTIC.INI`
//! is read too if there is one, so somebody's 1992 settings carry across.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The section every key lives in (`ds:0x0090`).
pub const SECTION: &str = "ScreenSaver.ScreenAntics";

/// What the screensaver runs with. Defaults are the values the executable
/// ships with in its data segment, which is what the original uses when the
/// file is missing -- it passes the current value as the default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Where `RESOURCE.001` and friends live. Empty means "beside me".
    pub source_dir: String,
    /// Draw the island at all (`ds:0x00B6`).
    pub background: bool,
    /// Clouds across the sky (`ds:0x00B8`).
    pub clouds: bool,
    /// Surf and glints on the water (`ds:0x00BA`).
    pub waves: bool,
    /// Sound effects (`ds:0x00BC`). The loader does not even run when off.
    pub sounds: bool,
    /// Dawn, as HHMM (`ds:0x00BE`). Johnny's day is eight real hours, so
    /// nine in the morning puts dusk at three.
    pub start_time: u16,
    /// The story day on offer (`ds:0x2D9B`, written as `Introduction`).
    pub story_offered: u8,
    /// The story day reached (`ds:0x2D9C`, written as `NumDays`).
    pub story_reached: u8,
    /// The date the story last moved on: year, month, day.
    pub story_date: (u16, u8, u8),
    /// How long the title card stays up, in ticks. The original has no such
    /// setting -- it shows the card while it loads, and this port loads too
    /// quickly for that to be visible.
    pub intro_ticks: u32,
    /// Percentage of the authored frame rate. See `director::DEFAULT_SPEED`.
    pub speed: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            source_dir: String::new(),
            background: true,
            clouds: true,
            waves: true,
            sounds: true,
            start_time: 900,
            story_offered: 0,
            story_reached: 0,
            story_date: (0, 0, 0),
            intro_ticks: crate::director::INTRO_TICKS,
            speed: crate::director::DEFAULT_SPEED,
        }
    }
}

impl Settings {
    /// Read a settings file. A missing or unreadable one is not an error --
    /// the defaults are what the original uses in that case too.
    pub fn load(path: &Path) -> Settings {
        let mut s = Settings::default();
        let Ok(text) = std::fs::read_to_string(path) else {
            return s;
        };
        let keys = parse(&text);
        let flag = |k: &str, now: bool| keys.get(k).map_or(now, |v| v != "0");
        let num = |k: &str, now: u32| {
            keys.get(k).and_then(|v| v.trim().parse().ok()).unwrap_or(now)
        };
        s.source_dir = keys.get("sourcedir").cloned().unwrap_or(s.source_dir);
        s.background = flag("background", s.background);
        s.clouds = flag("clouds", s.clouds);
        s.waves = flag("waves", s.waves);
        s.sounds = flag("sounds", s.sounds);
        s.start_time = num("starttime", s.start_time as u32) as u16;
        s.story_offered = num("introduction", s.story_offered as u32) as u8;
        s.story_reached = num("numdays", s.story_reached as u32) as u8;
        s.story_date = (
            num("currentyear", s.story_date.0 as u32) as u16,
            num("currentmonth", s.story_date.1 as u32) as u8,
            num("currentday", s.story_date.2 as u32) as u8,
        );
        s.intro_ticks = num("introticks", s.intro_ticks);
        s.speed = num("speed", s.speed).clamp(10, 400);
        s
    }

    /// Write it back, keeping the original's key names so the file stays
    /// readable by the thing it came from.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let b = |v: bool| if v { 1 } else { 0 };
        let text = format!(
            "; Johnny Castaway, a native port. The key names are the \
             original's.\n\
             ; Introduction and NumDays are the story: the day on offer and \
             the day reached.\n\
             [{SECTION}]\n\
             SourceDir={}\n\
             Background={}\n\
             Clouds={}\n\
             Waves={}\n\
             Sounds={}\n\
             StartTime={}\n\
             Introduction={}\n\
             NumDays={}\n\
             CurrentYear={}\n\
             CurrentMonth={}\n\
             CurrentDay={}\n\
             \n\
             ; This port's own, which the 1992 settings dialog had no room \
             for.\n\
             Speed={}\n\
             IntroTicks={}\n",
            self.source_dir,
            b(self.background),
            b(self.clouds),
            b(self.waves),
            b(self.sounds),
            self.start_time,
            self.story_offered,
            self.story_reached,
            self.story_date.0,
            self.story_date.1,
            self.story_date.2,
            self.speed,
            self.intro_ticks,
        );
        std::fs::write(path, text)
    }

    /// Let the environment win, for the tools. Nothing the screensaver does
    /// depends on this; it is how `jcisland` and `jcplay` stay one command
    /// long.
    pub fn with_env(mut self) -> Settings {
        fn num<T: std::str::FromStr>(key: &str) -> Option<T> {
            std::env::var(key).ok().and_then(|v| v.parse().ok())
        }
        if let Some(v) = num::<u32>("JC_SPEED") {
            self.speed = v.clamp(10, 400);
        }
        if let Some(v) = num::<u32>("JC_INTRO") {
            self.intro_ticks = v;
        }
        if let Some(v) = num::<u16>("JC_DAWN") {
            self.start_time = v;
        }
        if let Ok(v) = std::env::var("JC_DATA") {
            self.source_dir = v;
        }
        if let Some(v) = num::<u8>("JC_DAY") {
            self.story_offered = v;
            self.story_reached = v;
        }
        self
    }

    /// Where the settings file lives: beside the executable, unless
    /// `JC_CONFIG` says otherwise.
    pub fn path() -> PathBuf {
        if let Ok(p) = std::env::var("JC_CONFIG") {
            return PathBuf::from(p);
        }
        let beside = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("johnny.ini")));
        beside.unwrap_or_else(|| PathBuf::from("johnny.ini"))
    }
}

/// `key=value` lines under `[section]` headers, lower-cased keys, comments
/// after `;` or `#`. Only the one section is read; anything else is skipped,
/// so an original `SCRANTIC.INI` with other sections in it still works.
fn parse(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        let line = match line.find([';', '#']) {
            Some(i) => line[..i].trim(),
            None => line,
        };
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            inside = name.eq_ignore_ascii_case(SECTION);
            continue;
        }
        if !inside {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            out.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    out
}
