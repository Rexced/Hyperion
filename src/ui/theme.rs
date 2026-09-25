//! Colour themes: built-in presets plus user themes saved as shareable TOML files.
//!
//! A theme is six hand-picked colours (`ThemeSpec`); card, border, button and graph
//! shades are derived from them so every theme keeps consistent contrast. Any derived
//! colour can be overridden in `ThemeSpec::advanced`.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, CornerRadius, Mesh, Shadow, Stroke, Visuals};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Key of the theme used when the saved one no longer exists.
pub const DEFAULT_KEY: &str = "jade";
/// Keys of themes loaded from files are prefixed so they can't collide with presets.
const CUSTOM_PREFIX: &str = "custom:";

/// A colour stored as `"#rrggbb"` in theme files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeColor(pub Color32);

impl ThemeColor {
    const fn hex(rgb: u32) -> Self {
        Self(Color32::from_rgb(
            (rgb >> 16) as u8,
            (rgb >> 8) as u8,
            rgb as u8,
        ))
    }

    fn parse(text: &str) -> Option<Self> {
        let hex = text.trim().strip_prefix('#').unwrap_or(text.trim());
        if hex.len() != 6 {
            return None;
        }
        u32::from_str_radix(hex, 16).ok().map(Self::hex)
    }
}

impl fmt::Display for ThemeColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [r, g, b, _] = self.0.to_array();
        write!(f, "#{r:02x}{g:02x}{b:02x}")
    }
}

impl Serialize for ThemeColor {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ThemeColor {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Self::parse(&text).ok_or_else(|| {
            serde::de::Error::custom(format!("`{text}` is not a colour like \"#ff8c32\""))
        })
    }
}

/// Optional overrides for colours that are normally derived.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Advanced {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card: Option<ThemeColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card_stroke: Option<ThemeColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub well: Option<ThemeColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grid: Option<ThemeColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<ThemeColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ram: Option<ThemeColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swap: Option<ThemeColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read: Option<ThemeColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write: Option<ThemeColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rx: Option<ThemeColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx: Option<ThemeColor>,
    /// Per-core bar colours, blended by load: idle, busy, maxed out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_low: Option<ThemeColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_mid: Option<ThemeColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_high: Option<ThemeColor>,
}

impl Advanced {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Each override with its label, for the editor.
    pub fn fields_mut(&mut self) -> [(&'static str, &mut Option<ThemeColor>); 14] {
        [
            ("Card", &mut self.card),
            ("Card border", &mut self.card_stroke),
            ("Inset background", &mut self.well),
            ("Graph grid", &mut self.grid),
            ("CPU line", &mut self.cpu),
            ("RAM line", &mut self.ram),
            ("Swap line", &mut self.swap),
            ("Disk read line", &mut self.read),
            ("Disk write line", &mut self.write),
            ("Download line", &mut self.rx),
            ("Upload line", &mut self.tx),
            ("Load: calm", &mut self.load_low),
            ("Load: busy", &mut self.load_mid),
            ("Load: maxed", &mut self.load_high),
        ]
    }
}

/// Everything that defines a theme; this is exactly what a theme file contains.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThemeSpec {
    pub name: String,
    /// Light themes derive darker borders and buttons from the background instead of
    /// lighter ones.
    #[serde(default)]
    pub light: bool,
    pub bg_top: ThemeColor,
    pub bg_bottom: ThemeColor,
    pub text: ThemeColor,
    pub text_dim: ThemeColor,
    pub accent: ThemeColor,
    pub accent2: ThemeColor,
    #[serde(default, skip_serializing_if = "Advanced::is_empty")]
    pub advanced: Advanced,
}

/// Line colour for each metric's graph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MetricColors {
    pub cpu: Color32,
    pub ram: Color32,
    pub swap: Color32,
    pub read: Color32,
    pub write: Color32,
    pub rx: Color32,
    pub tx: Color32,
}

/// Every colour the UI uses, resolved from a `ThemeSpec`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub light: bool,
    pub bg_top: Color32,
    pub bg_bottom: Color32,
    pub card: Color32,
    pub card_stroke: Color32,
    pub well: Color32,
    pub grid: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    pub accent2: Color32,
    pub lines: MetricColors,
    /// Colours a load meter blends through at 0%, 50% and 100%.
    pub load: [Color32; 3],
}

impl Palette {
    /// Colour for a load from 0 to 100%: calm to busy over the first half, busy to
    /// maxed over the second.
    pub fn load_color(&self, percent: f32) -> Color32 {
        let t = (percent / 100.0).clamp(0.0, 1.0);
        let [low, mid, high] = self.load;
        if t < 0.5 {
            lerp(low, mid, t * 2.0)
        } else {
            lerp(mid, high, (t - 0.5) * 2.0)
        }
    }
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

impl ThemeSpec {
    pub fn palette(&self) -> Palette {
        let bottom = self.bg_bottom.0;
        let (card, stroke, well) = if self.light {
            (
                with_alpha(lighten(bottom, 14), 0xe6),
                darken(bottom, 34),
                darken(bottom, 10),
            )
        } else {
            (
                with_alpha(lighten(bottom, 16), 0xd8),
                lighten(bottom, 32),
                darken(bottom, 6),
            )
        };
        let a = &self.advanced;
        let or = |o: Option<ThemeColor>, default: Color32| o.map_or(default, |c| c.0);
        let (accent, accent2) = (self.accent.0, self.accent2.0);
        let card_stroke = or(a.card_stroke, stroke);
        Palette {
            light: self.light,
            bg_top: self.bg_top.0,
            bg_bottom: bottom,
            card: or(a.card, card),
            card_stroke,
            well: or(a.well, well),
            grid: or(a.grid, card_stroke),
            text: self.text.0,
            text_dim: self.text_dim.0,
            accent,
            accent2,
            load: [
                or(a.load_low, accent2),
                or(a.load_mid, accent),
                or(
                    a.load_high,
                    if self.light {
                        Color32::from_rgb(0xd1, 0x24, 0x2f)
                    } else {
                        Color32::from_rgb(0xff, 0x55, 0x55)
                    },
                ),
            ],
            lines: MetricColors {
                cpu: or(a.cpu, accent),
                ram: or(a.ram, accent),
                swap: or(a.swap, accent2),
                read: or(a.read, accent2),
                write: or(a.write, accent),
                rx: or(a.rx, accent2),
                tx: or(a.tx, accent),
            },
        }
    }
}

/// Current value of every advanced colour (override or derived), for the editor to
/// show and to start from when an override is switched on.
pub fn resolved_advanced(p: &Palette) -> Advanced {
    let c = |v| Some(ThemeColor(v));
    Advanced {
        card: c(p.card),
        card_stroke: c(p.card_stroke),
        well: c(p.well),
        grid: c(p.grid),
        cpu: c(p.lines.cpu),
        ram: c(p.lines.ram),
        swap: c(p.lines.swap),
        read: c(p.lines.read),
        write: c(p.lines.write),
        rx: c(p.lines.rx),
        tx: c(p.lines.tx),
        load_low: c(p.load[0]),
        load_mid: c(p.load[1]),
        load_high: c(p.load[2]),
    }
}

// ---------------------------------------------------------------------------------
// Built-in presets

/// `[bg_top, bg_bottom, text, text_dim, accent, accent2]` as 0xRRGGBB.
type Colors = [u32; 6];

fn preset(name: &str, light: bool, c: Colors) -> ThemeSpec {
    let [bg_top, bg_bottom, text, text_dim, accent, accent2] = c.map(ThemeColor::hex);
    ThemeSpec {
        name: name.to_owned(),
        light,
        bg_top,
        bg_bottom,
        text,
        text_dim,
        accent,
        accent2,
        advanced: Advanced::default(),
    }
}

/// `(key, spec)` for every built-in theme. Keys are stored in the config file; never
/// rename them.
fn presets() -> Vec<(&'static str, ThemeSpec)> {
    let mut high_contrast = preset(
        "High Contrast",
        false,
        [0x000000, 0x000000, 0xffffff, 0xd0d0d0, 0x6fc3df, 0xf38518],
    );
    high_contrast.advanced.card_stroke = Some(ThemeColor::hex(0x6fc3df));
    high_contrast.advanced.card = Some(ThemeColor::hex(0x000000));

    let mut list = vec![
        // Hyperion's own
        (
            "jade",
            preset(
                "Jade",
                false,
                [0x0f332a, 0x061713, 0xe4f2ec, 0x8fb5a8, 0xff8c32, 0x5fe3b8],
            ),
        ),
        (
            "amber",
            preset(
                "Terminal Amber",
                false,
                [0x14120a, 0x000000, 0xffd99a, 0xa87d45, 0xffb000, 0xff6a00],
            ),
        ),
        // VS Code built-ins
        (
            "vscode-dark-modern",
            preset(
                "Dark Modern",
                false,
                [0x1f1f1f, 0x181818, 0xcccccc, 0x9d9d9d, 0x0078d4, 0x4ec9b0],
            ),
        ),
        (
            "vscode-dark-plus",
            preset(
                "Dark+",
                false,
                [0x252526, 0x1e1e1e, 0xd4d4d4, 0x858585, 0x569cd6, 0xce9178],
            ),
        ),
        (
            "abyss",
            preset(
                "Abyss",
                false,
                [0x051336, 0x000c18, 0xa6b7e6, 0x5d7599, 0xddbb88, 0x22aa44],
            ),
        ),
        (
            "kimbie-dark",
            preset(
                "Kimbie Dark",
                false,
                [0x362712, 0x221a0f, 0xd3af86, 0xa57a4c, 0xf06431, 0x889b4a],
            ),
        ),
        (
            "tomorrow-night-blue",
            preset(
                "Tomorrow Night Blue",
                false,
                [0x00346e, 0x002451, 0xffffff, 0x7285b7, 0xffc58f, 0x99ffff],
            ),
        ),
        ("high-contrast", high_contrast),
        (
            "monokai",
            preset(
                "Monokai",
                false,
                [0x2f302a, 0x1e1f1c, 0xf8f8f2, 0xa1a19a, 0xf92672, 0xa6e22e],
            ),
        ),
        (
            "solarized",
            preset(
                "Solarized Dark",
                false,
                [0x073642, 0x002b36, 0x93a1a1, 0x586e75, 0xb58900, 0x268bd2],
            ),
        ),
        // Popular community themes
        (
            "one-dark-pro",
            preset(
                "One Dark Pro",
                false,
                [0x282c34, 0x21252b, 0xabb2bf, 0x7f848e, 0x61afef, 0x98c379],
            ),
        ),
        (
            "tokyo-night",
            preset(
                "Tokyo Night",
                false,
                [0x1a1b26, 0x16161e, 0xc0caf5, 0x787c99, 0x7aa2f7, 0xbb9af7],
            ),
        ),
        (
            "catppuccin-mocha",
            preset(
                "Catppuccin Mocha",
                false,
                [0x1e1e2e, 0x11111b, 0xcdd6f4, 0xa6adc8, 0xcba6f7, 0xfab387],
            ),
        ),
        (
            "github-dark",
            preset(
                "GitHub Dark",
                false,
                [0x161b22, 0x0d1117, 0xe6edf3, 0x7d8590, 0x2f81f7, 0x3fb950],
            ),
        ),
        (
            "night-owl",
            preset(
                "Night Owl",
                false,
                [0x0b2942, 0x011627, 0xd6deeb, 0x5f7e97, 0x82aaff, 0xc792ea],
            ),
        ),
        (
            "ayu-mirage",
            preset(
                "Ayu Mirage",
                false,
                [0x242936, 0x1f2430, 0xcccac2, 0x707a8c, 0xffcc66, 0x73d0ff],
            ),
        ),
        (
            "rose-pine",
            preset(
                "Rosé Pine",
                false,
                [0x1f1d2e, 0x191724, 0xe0def4, 0x908caa, 0xeb6f92, 0x9ccfd8],
            ),
        ),
        (
            "everforest",
            preset(
                "Everforest",
                false,
                [0x2d353b, 0x232a2e, 0xd3c6aa, 0x859289, 0xa7c080, 0xe69875],
            ),
        ),
        (
            "kanagawa",
            preset(
                "Kanagawa",
                false,
                [0x1f1f28, 0x16161d, 0xdcd7ba, 0x727169, 0x7e9cd8, 0xffa066],
            ),
        ),
        (
            "dracula",
            preset(
                "Dracula",
                false,
                [0x2c2e3f, 0x1a1b26, 0xf8f8f2, 0x9a9cb8, 0xbd93f9, 0xff79c6],
            ),
        ),
        (
            "nord",
            preset(
                "Nord",
                false,
                [0x3b4252, 0x2e3440, 0xeceff4, 0x9dabbd, 0x88c0d0, 0xa3be8c],
            ),
        ),
        (
            "gruvbox",
            preset(
                "Gruvbox Dark",
                false,
                [0x32302f, 0x1d2021, 0xebdbb2, 0xa89984, 0xfe8019, 0xb8bb26],
            ),
        ),
        // Light
        (
            "vscode-light-modern",
            preset(
                "Light Modern",
                true,
                [0xf3f3f3, 0xe8e8e8, 0x3b3b3b, 0x6e7681, 0x005fb8, 0x0f8a4c],
            ),
        ),
        (
            "quiet-light",
            preset(
                "Quiet Light",
                true,
                [0xf5f5f5, 0xe7e7e7, 0x333333, 0x7a7a7a, 0x7a3e9d, 0x448c27],
            ),
        ),
        (
            "catppuccin-latte",
            preset(
                "Catppuccin Latte",
                true,
                [0xeff1f5, 0xdce0e8, 0x4c4f69, 0x6c6f85, 0x8839ef, 0xfe640b],
            ),
        ),
        (
            "github-light",
            preset(
                "GitHub Light",
                true,
                [0xf6f8fa, 0xeaeef2, 0x1f2328, 0x656d76, 0x0969da, 0x1a7f37],
            ),
        ),
        (
            "solarized-light",
            preset(
                "Solarized Light",
                true,
                [0xfdf6e3, 0xeee8d5, 0x586e75, 0x93a1a1, 0xb58900, 0x268bd2],
            ),
        ),
    ];

    // Each theme's own "error red" for a maxed-out load meter.
    let maxed: [(&str, u32); 27] = [
        ("jade", 0xff4d4d),
        ("amber", 0xff3b1f),
        ("vscode-dark-modern", 0xf14c4c),
        ("vscode-dark-plus", 0xf44747),
        ("abyss", 0xf25c5c),
        ("kimbie-dark", 0xdc3958),
        ("tomorrow-night-blue", 0xff9da4),
        ("high-contrast", 0xff5f5f),
        ("monokai", 0xff2d55),
        ("solarized", 0xdc322f),
        ("one-dark-pro", 0xe06c75),
        ("tokyo-night", 0xf7768e),
        ("catppuccin-mocha", 0xf38ba8),
        ("github-dark", 0xf85149),
        ("night-owl", 0xef5350),
        ("ayu-mirage", 0xf28779),
        ("rose-pine", 0xff5c8a),
        ("everforest", 0xe67e80),
        ("kanagawa", 0xff5d62),
        ("dracula", 0xff5555),
        ("nord", 0xbf616a),
        ("gruvbox", 0xfb4934),
        ("vscode-light-modern", 0xcd3131),
        ("quiet-light", 0xaa3731),
        ("catppuccin-latte", 0xd20f39),
        ("github-light", 0xd1242f),
        ("solarized-light", 0xdc322f),
    ];
    for (key, spec) in &mut list {
        if let Some((_, red)) = maxed.iter().find(|(k, _)| k == key) {
            spec.advanced.load_high = Some(ThemeColor::hex(*red));
        }
    }
    // Amber's second accent is orange, which reads as "busy", not "calm".
    if let Some((_, amber)) = list.iter_mut().find(|(k, _)| *k == "amber") {
        amber.advanced.load_low = Some(ThemeColor::hex(0xa87d45));
    }
    list
}

// ---------------------------------------------------------------------------------
// Library: presets + themes loaded from files

pub struct Entry {
    pub key: String,
    pub spec: ThemeSpec,
    /// Where a custom theme lives; `None` for presets.
    pub path: Option<PathBuf>,
}

impl Entry {
    pub fn is_custom(&self) -> bool {
        self.path.is_some()
    }
}

pub struct Library {
    presets: Vec<Entry>,
    custom: Vec<Entry>,
    dir: Option<PathBuf>,
}

impl Library {
    /// Presets plus every `*.toml` in `dir`. Returns human-readable problems with
    /// files that couldn't be loaded; those files are skipped, never modified.
    pub fn load(dir: Option<PathBuf>) -> (Self, Vec<String>) {
        let presets = presets()
            .into_iter()
            .map(|(key, spec)| Entry {
                key: key.to_owned(),
                spec,
                path: None,
            })
            .collect();
        let mut library = Self {
            presets,
            custom: Vec::new(),
            dir,
        };
        let problems = library.reload_custom();
        (library, problems)
    }

    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    fn reload_custom(&mut self) -> Vec<String> {
        self.custom.clear();
        let Some(dir) = &self.dir else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut problems = Vec::new();
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| Some(e.ok()?.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        paths.sort();
        for path in paths {
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let parsed = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| toml::from_str::<ThemeSpec>(&text).map_err(|e| e.to_string()));
            match parsed {
                Ok(spec) => self.custom.push(Entry {
                    key: format!("{CUSTOM_PREFIX}{stem}"),
                    spec,
                    path: Some(path),
                }),
                Err(e) => problems.push(format!("{}: {e}", path.display())),
            }
        }
        problems
    }

    pub fn presets(&self) -> impl Iterator<Item = &Entry> {
        self.presets.iter()
    }

    pub fn custom(&self) -> impl Iterator<Item = &Entry> {
        self.custom.iter()
    }

    /// The theme for `key`, or the default one if it no longer exists.
    pub fn get(&self, key: &str) -> &Entry {
        self.custom
            .iter()
            .chain(&self.presets)
            .find(|e| e.key == key)
            .or_else(|| self.presets.iter().find(|e| e.key == DEFAULT_KEY))
            .expect("the default preset always exists")
    }

    /// Writes `spec` as a theme file and returns its key. `existing` (a custom key)
    /// overwrites that theme's file; otherwise a new file named after the theme is
    /// created without replacing any other.
    pub fn save(&mut self, spec: &ThemeSpec, existing: Option<&str>) -> io::Result<String> {
        let dir = self
            .dir
            .clone()
            .ok_or_else(|| io::Error::other("no config directory on this system"))?;
        std::fs::create_dir_all(&dir)?;
        let path = match existing.and_then(|k| self.custom.iter().find(|e| e.key == k)) {
            Some(entry) => entry.path.clone().expect("custom themes have a path"),
            None => unused_path(&dir, &slug(&spec.name)),
        };
        let text = toml::to_string_pretty(spec).map_err(io::Error::other)?;
        std::fs::write(&path, text)?;
        self.reload_custom();
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        Ok(format!("{CUSTOM_PREFIX}{stem}"))
    }

    pub fn delete(&mut self, key: &str) -> io::Result<()> {
        if let Some(path) = self
            .custom
            .iter()
            .find(|e| e.key == key)
            .and_then(|e| e.path.clone())
        {
            std::fs::remove_file(path)?;
        }
        self.reload_custom();
        Ok(())
    }
}

/// File-name-safe version of a theme name: "Rosé Pine 2!" -> "ros-pine-2".
fn slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    if out.is_empty() {
        "theme".to_owned()
    } else {
        out.to_owned()
    }
}

fn unused_path(dir: &Path, stem: &str) -> PathBuf {
    let mut path = dir.join(format!("{stem}.toml"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem}-{n}.toml"));
        n += 1;
    }
    path
}

// ---------------------------------------------------------------------------------
// Applying a palette to egui

fn lighten(c: Color32, amount: u8) -> Color32 {
    Color32::from_rgb(
        c.r().saturating_add(amount),
        c.g().saturating_add(amount),
        c.b().saturating_add(amount),
    )
}

fn darken(c: Color32, amount: u8) -> Color32 {
    Color32::from_rgb(
        c.r().saturating_sub(amount),
        c.g().saturating_sub(amount),
        c.b().saturating_sub(amount),
    )
}

fn with_alpha(c: Color32, alpha: u8) -> Color32 {
    let a = f32::from(alpha) / 255.0;
    Color32::from_rgba_premultiplied(
        (f32::from(c.r()) * a) as u8,
        (f32::from(c.g()) * a) as u8,
        (f32::from(c.b()) * a) as u8,
        alpha,
    )
}

pub fn apply(ctx: &egui::Context, p: &Palette) {
    let bottom = p.bg_bottom;
    // Buttons step away from the background: lighter on dark themes, darker on light.
    let step = |amount| {
        if p.light {
            darken(bottom, amount)
        } else {
            lighten(bottom, amount)
        }
    };
    let mut v = if p.light {
        Visuals::light()
    } else {
        Visuals::dark()
    };
    v.override_text_color = Some(p.text);
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = if p.light {
        lighten(bottom, 12)
    } else {
        lighten(bottom, 8)
    };
    v.faint_bg_color = p.card;
    v.extreme_bg_color = p.well;
    v.code_bg_color = p.well;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.window_stroke = Stroke::new(1.0, p.card_stroke);
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = Shadow::NONE;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.card;
    w.noninteractive.weak_bg_fill = p.card;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.card_stroke);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    for (state, fill) in [
        (&mut w.inactive, step(20)),
        (&mut w.hovered, step(33)),
        (&mut w.active, step(44)),
        (&mut w.open, step(33)),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.corner_radius = CornerRadius::same(6);
        state.fg_stroke = Stroke::new(1.5, p.text);
    }
    w.hovered.bg_stroke = Stroke::new(1.0, p.accent2);
    w.active.bg_stroke = Stroke::new(1.0, p.accent);

    let (theme, preference) = if p.light {
        (egui::Theme::Light, egui::ThemePreference::Light)
    } else {
        (egui::Theme::Dark, egui::ThemePreference::Dark)
    };
    ctx.set_visuals_of(theme, v);
    ctx.set_theme(preference);
}

/// Vertical background gradient, painted first so cards sit on top of it.
pub fn paint_background(ui: &egui::Ui, p: &Palette) {
    let rect = ui.max_rect();
    let mut mesh = Mesh::default();
    let (tl, tr) = (rect.left_top(), rect.right_top());
    let (bl, br) = (rect.left_bottom(), rect.right_bottom());
    for (pos, color) in [
        (tl, p.bg_top),
        (tr, p.bg_top),
        (br, p.bg_bottom),
        (bl, p.bg_bottom),
    ] {
        mesh.colored_vertex(pos, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(mesh);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> (tempfile::TempDir, Library) {
        let dir = tempfile::tempdir().unwrap();
        let (lib, problems) = Library::load(Some(dir.path().to_owned()));
        assert!(problems.is_empty());
        (dir, lib)
    }

    #[test]
    fn preset_keys_are_unique_and_include_the_default() {
        let mut keys: Vec<_> = presets().iter().map(|(k, _)| *k).collect();
        assert!(keys.contains(&DEFAULT_KEY));
        let n = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), n);
    }

    #[test]
    fn keys_saved_by_earlier_versions_still_resolve() {
        let (_dir, lib) = library();
        for key in [
            "jade",
            "dracula",
            "nord",
            "gruvbox",
            "monokai",
            "solarized",
            "amber",
        ] {
            assert_eq!(lib.get(key).key, key);
        }
        assert_eq!(lib.get("no-such-theme").key, DEFAULT_KEY);
    }

    #[test]
    fn colours_roundtrip_as_hex() {
        let c = ThemeColor::hex(0xff8c32);
        assert_eq!(c.to_string(), "#ff8c32");
        assert_eq!(ThemeColor::parse("#FF8C32"), Some(c));
        assert_eq!(ThemeColor::parse("ff8c32"), Some(c));
        assert_eq!(ThemeColor::parse("#ff8c3"), None);
    }

    #[test]
    fn light_themes_derive_darker_borders_dark_themes_lighter() {
        let p = |key: &str| {
            let (_, spec) = presets().into_iter().find(|(k, _)| *k == key).unwrap();
            spec.palette()
        };
        let lum = |c: Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
        let light = p("github-light");
        assert!(light.light && lum(light.card_stroke) < lum(light.bg_bottom));
        let dark = p("github-dark");
        assert!(!dark.light && lum(dark.card_stroke) > lum(dark.bg_bottom));
    }

    #[test]
    fn advanced_overrides_replace_derived_colours() {
        let mut spec = presets().remove(0).1;
        let red = ThemeColor::hex(0xff0000);
        spec.advanced.cpu = Some(red);
        spec.advanced.grid = Some(red);
        let p = spec.palette();
        assert_eq!(p.lines.cpu, red.0);
        assert_eq!(p.grid, red.0);
        assert_eq!(p.lines.ram, spec.accent.0);
    }

    #[test]
    fn custom_themes_save_as_files_and_reload() {
        let (dir, mut lib) = library();
        let mut spec = lib.get("nord").spec.clone();
        spec.name = "My Nord!".into();
        spec.advanced.tx = Some(ThemeColor::hex(0x123456));
        let key = lib.save(&spec, None).unwrap();
        assert_eq!(key, "custom:my-nord");
        assert!(dir.path().join("my-nord.toml").exists());

        // A second theme with the same name gets its own file.
        let key2 = lib.save(&spec, None).unwrap();
        assert_eq!(key2, "custom:my-nord-2");

        // Another instance picks both up from disk, unchanged.
        let (fresh, problems) = Library::load(Some(dir.path().to_owned()));
        assert!(problems.is_empty());
        assert_eq!(fresh.get(&key).spec, spec);
        assert_eq!(fresh.custom().count(), 2);

        lib.delete(&key2).unwrap();
        assert!(!dir.path().join("my-nord-2.toml").exists());
    }

    #[test]
    fn broken_theme_files_are_reported_and_skipped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("bad.toml"),
            "name = \"Bad\"\naccent = \"orange\"\n",
        )
        .unwrap();
        let (lib, problems) = Library::load(Some(dir.path().to_owned()));
        assert_eq!(lib.custom().count(), 0);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("bad.toml"));
    }

    #[test]
    fn theme_file_format_is_readable_toml() {
        let mut spec = presets().remove(0).1;
        spec.advanced.cpu = Some(ThemeColor::hex(0xaabbcc));
        let text = toml::to_string_pretty(&spec).unwrap();
        assert!(text.contains("accent = \"#ff8c32\""));
        assert!(text.contains("[advanced]\ncpu = \"#aabbcc\""));
        assert!(!text.contains("swap"));
    }

    #[test]
    fn every_preset_has_its_own_maxed_colour() {
        for (key, spec) in presets() {
            assert!(
                spec.advanced.load_high.is_some(),
                "{key} has no maxed colour"
            );
        }
    }

    #[test]
    fn load_colour_blends_through_three_stops() {
        let p = presets().remove(0).1.palette();
        assert_eq!(p.load_color(0.0), p.load[0]);
        assert_eq!(p.load_color(50.0), p.load[1]);
        assert_eq!(p.load_color(100.0), p.load[2]);
        assert_eq!(p.load_color(150.0), p.load[2]);
        let quarter = p.load_color(25.0);
        assert_ne!(quarter, p.load[0]);
        assert_ne!(quarter, p.load[1]);
    }

    #[test]
    fn slugs_are_file_name_safe() {
        assert_eq!(slug("Rosé Pine 2!"), "ros-pine-2");
        assert_eq!(slug("  ***  "), "theme");
    }
}
