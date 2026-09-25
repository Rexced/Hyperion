use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::metrics::{EnabledSet, MetricId};
use crate::ui::theme;

/// 100 ms – 1 s in 100 ms steps, then 1 s – 10 s in 1 s steps.
pub const INTERVAL_STEPS_MS: [u64; 19] = [
    100, 200, 300, 400, 500, 600, 700, 800, 900, 1000, 2000, 3000, 4000, 5000, 6000, 7000, 8000,
    9000, 10000,
];
pub const HISTORY_STEPS_SECS: [u64; 7] = [30, 60, 120, 300, 600, 900, 1800];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub interval_ms: u64,
    pub history_secs: u64,
    /// Theme key (a preset, or `custom:<file>`); unknown keys fall back to the default.
    pub theme: String,
    /// Keyed by `MetricId::key()`; missing keys fall back to the metric's default.
    pub metrics: BTreeMap<String, bool>,
    /// Dashboard tile keys (e.g. "cpu", "disk:sda") in display order, set by dragging
    /// tiles around. A tile missing from this list is appended after the ones in it.
    pub tile_order: Vec<String>,
    /// Tiles popped out of the grid into their own window. A key here is never also in
    /// `tile_order`.
    pub detached_tiles: BTreeMap<String, DetachedTile>,
    /// Tile key -> `[columns, rows]` it spans in the grid, set by resizing a tile.
    /// Tiles not listed are one cell.
    pub tile_spans: BTreeMap<String, [u8; 2]>,
    /// Drive id (udev serial) -> whether its tile is shown. Drives not listed show
    /// only if they hold the root filesystem.
    pub drives: BTreeMap<String, bool>,
    /// GPU id (PCI slot) -> whether its tiles are shown. GPUs not listed show if
    /// they're dedicated cards (or the only GPU).
    pub gpus: BTreeMap<String, bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DetachedTile {
    /// Global top-left in logical pixels; `None` when the platform can't report or set
    /// window positions (plain Wayland outside Hyprland), so the compositor picks.
    pub pos: Option<[f32; 2]>,
    pub size: [f32; 2],
    /// Grid position it was torn off from, so closing the window puts it back there.
    #[serde(default)]
    pub home: Option<usize>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            interval_ms: 1000,
            history_secs: 60,
            theme: theme::DEFAULT_KEY.to_owned(),
            metrics: BTreeMap::new(),
            tile_order: Vec::new(),
            detached_tiles: BTreeMap::new(),
            tile_spans: BTreeMap::new(),
            drives: BTreeMap::new(),
            gpus: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("hyperion").join("config.toml"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::from_toml(&text).unwrap_or_else(|e| {
                eprintln!("hyperion: ignoring invalid config {}: {e}", path.display());
                Self::default()
            }),
            Err(e) if e.kind() == ErrorKind::NotFound => Self::default(),
            Err(e) => {
                eprintln!("hyperion: cannot read config {}: {e}", path.display());
                Self::default()
            }
        }
    }

    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str::<Self>(text).map(Self::normalized)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path().context("no config directory")?;
        let dir = path.parent().context("config path has no parent")?;
        std::fs::create_dir_all(dir)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Where custom theme files live (`~/.config/hyperion/themes` on Linux,
    /// `%APPDATA%\hyperion\themes` on Windows).
    pub fn themes_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("hyperion").join("themes"))
    }

    pub fn enabled(&self) -> EnabledSet {
        let mut set = EnabledSet::default();
        for id in MetricId::ALL {
            set.set(id, self.is_enabled(id));
        }
        set
    }

    pub fn is_enabled(&self, id: MetricId) -> bool {
        self.metrics
            .get(id.key())
            .copied()
            .unwrap_or_else(|| id.default_enabled())
    }

    pub fn set_enabled(&mut self, id: MetricId, on: bool) {
        self.metrics.insert(id.key().to_owned(), on);
    }

    fn normalized(mut self) -> Self {
        self.interval_ms = INTERVAL_STEPS_MS[nearest_step(&INTERVAL_STEPS_MS, self.interval_ms)];
        self.history_secs =
            HISTORY_STEPS_SECS[nearest_step(&HISTORY_STEPS_SECS, self.history_secs)];
        self
    }
}

pub fn nearest_step(steps: &[u64], value: u64) -> usize {
    steps
        .iter()
        .enumerate()
        .min_by_key(|(_, s)| s.abs_diff(value))
        .map_or(0, |(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_steps_match_spec() {
        assert_eq!(INTERVAL_STEPS_MS.first(), Some(&100));
        assert_eq!(INTERVAL_STEPS_MS[9], 1000);
        assert_eq!(INTERVAL_STEPS_MS.last(), Some(&10_000));
        assert!(
            INTERVAL_STEPS_MS[..10]
                .windows(2)
                .all(|w| w[1] - w[0] == 100)
        );
        assert!(
            INTERVAL_STEPS_MS[9..]
                .windows(2)
                .all(|w| w[1] - w[0] == 1000)
        );
    }

    #[test]
    fn roundtrips_and_snaps_values() {
        let mut c = Config::default();
        c.set_enabled(MetricId::DiskIo, false);
        let text = toml::to_string_pretty(&c).unwrap();
        assert_eq!(Config::from_toml(&text).unwrap(), c);

        let snapped = Config::from_toml("interval_ms = 1450\nhistory_secs = 100000").unwrap();
        assert_eq!(snapped.interval_ms, 1000);
        assert_eq!(snapped.history_secs, 1800);
    }

    #[test]
    fn missing_metrics_use_defaults_and_unknown_keys_are_ignored() {
        let c =
            Config::from_toml("[metrics]\n\"net.throughput\" = false\n\"gone.metric\" = true\n")
                .unwrap();
        assert!(!c.is_enabled(MetricId::NetThroughput));
        assert!(c.is_enabled(MetricId::CpuTotal));
        assert_eq!(c.interval_ms, 1000);
    }
}
