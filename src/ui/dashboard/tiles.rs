//! What each tile shows, and how much room it needs.

use std::collections::BTreeMap;

use eframe::egui::{self, RichText};

use super::View;
use crate::format;
use crate::metrics::{DriveSample, EnabledSet, GpuSample, MetricId::*, Snapshot};
use crate::ui::widgets::{
    BAR_HEIGHT, CARD_INSET, GraphLine, Scale, card, core_grid, core_grid_height, graph, usage_bar,
};

/// Below this a graph is no longer readable, so the grid scrolls instead of shrinking.
const MIN_GRAPH_HEIGHT: f32 = 70.0;
/// Title row plus the space under it (see `widgets::card`); an estimate that only
/// decides when scrolling starts.
const TITLE_HEIGHT: f32 = 30.0;
/// Space between a graph and whatever sits under it.
const SECTION_GAP: f32 = 6.0;

pub enum Card<'a> {
    Cpu,
    Memory,
    Net(&'a str),
    Drive(&'a DriveSample),
    Gpu(&'a GpuSample, GpuView),
}

/// The four tiles each GPU gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpuView {
    Usage,
    Power,
    Memory,
    Temp,
}

impl GpuView {
    fn key(self) -> &'static str {
        match self {
            GpuView::Usage => "usage",
            GpuView::Power => "power",
            GpuView::Memory => "memory",
            GpuView::Temp => "temp",
        }
    }

    fn label(self) -> &'static str {
        match self {
            GpuView::Usage => "Usage",
            GpuView::Power => "Power",
            GpuView::Memory => "Memory",
            GpuView::Temp => "Temperature",
        }
    }
}

/// "Radeon RX 6600/6600 XT/6600M" -> "Radeon RX 6600": the database lists every
/// card sharing a chip; the first is enough for a title.
fn short_gpu_name(name: &str) -> &str {
    name.split('/').next().unwrap_or(name).trim()
}

impl Card<'_> {
    /// Stable identifier stored in the config file; never change the format.
    pub fn key(&self) -> String {
        match *self {
            Card::Cpu => "cpu".to_owned(),
            Card::Memory => "memory".to_owned(),
            Card::Net(name) => format!("net:{name}"),
            Card::Drive(d) => format!("drive:{}", d.id),
            Card::Gpu(g, view) => format!("gpu:{}:{}", g.id, view.key()),
        }
    }

    pub fn title(&self) -> String {
        match *self {
            Card::Cpu => "CPU".to_owned(),
            Card::Memory => "Memory".to_owned(),
            Card::Net(name) => format!("Network · {name}"),
            Card::Drive(d) => d.model.clone(),
            Card::Gpu(g, view) => format!("{} · {}", short_gpu_name(&g.name), view.label()),
        }
    }
}

/// Whether a drive's tile is on: the user's choice, else only the boot drive.
pub fn drive_shown(drives: &BTreeMap<String, bool>, d: &DriveSample) -> bool {
    drives.get(&d.id).copied().unwrap_or(d.boot)
}

/// Whether a GPU's tiles are on: the user's choice, else dedicated GPUs only (or the
/// integrated one when it's all there is).
pub fn gpu_shown(gpus: &BTreeMap<String, bool>, g: &GpuSample, any_dedicated: bool) -> bool {
    gpus.get(&g.id)
        .copied()
        .unwrap_or(!g.integrated || !any_dedicated)
}

/// Every tile the current data and settings call for, in default order.
pub fn collect<'a>(
    latest: &'a Snapshot,
    enabled: EnabledSet,
    drives: &BTreeMap<String, bool>,
    gpus: &BTreeMap<String, bool>,
) -> Vec<Card<'a>> {
    let mut cards = Vec::new();
    if enabled.any(&[CpuTotal, CpuPerCore]) && latest.cpu.is_some() {
        cards.push(Card::Cpu);
    }
    let any_dedicated = latest.gpus.iter().any(|g| !g.integrated);
    for g in latest
        .gpus
        .iter()
        .filter(|g| gpu_shown(gpus, g, any_dedicated))
    {
        // A tile only when this GPU reports that data at all.
        let views = [
            (
                GpuUsage,
                GpuView::Usage,
                g.usage.is_some() || g.core_mhz.is_some(),
            ),
            (GpuPower, GpuView::Power, g.power_w.is_some()),
            (
                GpuMemory,
                GpuView::Memory,
                g.vram_total.is_some() || g.system_used.is_some(),
            ),
            (GpuTemp, GpuView::Temp, !g.temps.is_empty()),
        ];
        for (metric, view, has_data) in views {
            if enabled.contains(metric) && has_data {
                cards.push(Card::Gpu(g, view));
            }
        }
    }
    if enabled.any(&[RamUsage, SwapUsage]) && latest.mem.is_some() {
        cards.push(Card::Memory);
    }
    if enabled.contains(NetThroughput) {
        cards.extend(latest.net.iter().map(|n| Card::Net(&n.name)));
    }
    if enabled.any(&[DiskIo, DiskIops, DiskTemp, DiskSpace]) {
        cards.extend(
            latest
                .drives
                .iter()
                .filter(|d| drive_shown(drives, d))
                .map(Card::Drive),
        );
    }
    cards
}

/// Height of what sits under the graph (per-core grid, usage bars) at this width.
fn below_graph(
    c: &Card<'_>,
    latest: &Snapshot,
    enabled: EnabledSet,
    inner_width: f32,
    spacing: egui::Vec2,
) -> f32 {
    let bar = BAR_HEIGHT + spacing.y;
    match *c {
        Card::Cpu if enabled.contains(CpuPerCore) => {
            let cores = latest.cpu.as_ref().map_or(0, |c| c.per_core.len());
            SECTION_GAP + spacing.y + core_grid_height(cores, inner_width, spacing)
        }
        Card::Memory => {
            let bars = [RamUsage, SwapUsage]
                .iter()
                .filter(|&&m| enabled.contains(m))
                .count();
            SECTION_GAP + spacing.y + bars as f32 * bar
        }
        Card::Drive(d) if enabled.contains(DiskSpace) => {
            SECTION_GAP + spacing.y + d.partitions.len() as f32 * partition_height(spacing)
        }
        _ => 0.0,
    }
}

/// One partition row in a drive tile: label, bar and a little space.
fn partition_height(spacing: egui::Vec2) -> f32 {
    14.0 + BAR_HEIGHT + 4.0 + 3.0 * spacing.y
}

/// The smallest height at which everything in the tile is still readable.
pub fn min_height(
    c: &Card<'_>,
    latest: &Snapshot,
    enabled: EnabledSet,
    width: f32,
    spacing: egui::Vec2,
) -> f32 {
    let inner_width = width - 2.0 * CARD_INSET;
    let content = match c {
        Card::Drive(_) if !enabled.contains(DiskIo) => {
            below_graph(c, latest, enabled, inner_width, spacing)
        }
        Card::Cpu if !enabled.contains(CpuTotal) => {
            below_graph(c, latest, enabled, inner_width, spacing)
        }
        _ => MIN_GRAPH_HEIGHT + below_graph(c, latest, enabled, inner_width, spacing),
    };
    2.0 * CARD_INSET + TITLE_HEIGHT + content
}

/// Draws a tile filling `ui`'s space; its graph takes whatever height the rest leaves.
pub fn draw(ui: &mut egui::Ui, c: &Card<'_>, view: View<'_>) {
    let View {
        history,
        enabled,
        palette,
        ..
    } = view;
    let latest = history.latest();
    let colors = palette.lines;
    let title = c.title();
    let graph_height = |ui: &egui::Ui| {
        let below = below_graph(
            c,
            latest,
            enabled,
            ui.available_width(),
            ui.spacing().item_spacing,
        );
        (ui.available_height() - below).max(MIN_GRAPH_HEIGHT)
    };
    match *c {
        Card::Cpu => {
            let Some(cpu) = &latest.cpu else { return };
            let headline = if enabled.contains(CpuTotal) {
                format::percent(f64::from(cpu.total))
            } else {
                String::new()
            };
            let detail = [
                cpu.temp_c
                    .filter(|_| enabled.contains(CpuTemp))
                    .map(|t| format!("{t:.0}°C")),
                cpu.power_w
                    .filter(|_| enabled.contains(CpuPower))
                    .map(|w| format!("{w:.0} W")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            card(ui, palette, &title, &headline, &detail, |ui| {
                if enabled.contains(CpuTotal) {
                    let h = graph_height(ui);
                    let line = GraphLine {
                        key: "cpu",
                        name: "CPU",
                        color: colors.cpu,
                    };
                    graph(ui, palette, "plot:cpu", history, &[line], Scale::Percent, h);
                }
                if enabled.contains(CpuPerCore) {
                    ui.add_space(SECTION_GAP);
                    core_grid(ui, &cpu.per_core, palette);
                }
            });
        }
        Card::Memory => {
            let Some(m) = latest.mem else { return };
            let headline = if enabled.contains(RamUsage) {
                format!(
                    "{} / {}",
                    format::bytes(m.used as f64),
                    format::bytes(m.total as f64)
                )
            } else {
                String::new()
            };
            card(ui, palette, &title, &headline, "", |ui| {
                let mut lines = Vec::new();
                if enabled.contains(RamUsage) {
                    lines.push(GraphLine {
                        key: "ram",
                        name: "RAM",
                        color: colors.ram,
                    });
                }
                if enabled.contains(SwapUsage) && m.swap_total > 0 {
                    lines.push(GraphLine {
                        key: "swap",
                        name: "Swap",
                        color: colors.swap,
                    });
                }
                let h = graph_height(ui);
                graph(ui, palette, "plot:mem", history, &lines, Scale::Percent, h);
                ui.add_space(SECTION_GAP);
                if enabled.contains(RamUsage) {
                    ratio_bar(ui, "RAM", m.used, m.total, colors.ram);
                }
                if enabled.contains(SwapUsage) {
                    if m.swap_total > 0 {
                        ratio_bar(ui, "Swap", m.swap_used, m.swap_total, colors.swap);
                    } else {
                        ui.label(RichText::new("No swap configured").weak());
                    }
                }
            });
        }
        Card::Net(name) => {
            let Some(n) = latest.net.iter().find(|n| n.name == name) else {
                return;
            };
            let headline = format!("↓ {}  ↑ {}", format::rate(n.rx_bps), format::rate(n.tx_bps));
            let (rx, tx) = (format!("net:{name}:rx"), format!("net:{name}:tx"));
            card(ui, palette, &title, &headline, "", |ui| {
                let lines = [
                    GraphLine {
                        key: &rx,
                        name: "Download",
                        color: colors.rx,
                    },
                    GraphLine {
                        key: &tx,
                        name: "Upload",
                        color: colors.tx,
                    },
                ];
                let h = graph_height(ui);
                graph(
                    ui,
                    palette,
                    ("plot:net", name),
                    history,
                    &lines,
                    Scale::BytesPerSec,
                    h,
                );
            });
        }
        Card::Gpu(g, view) => {
            let id = g.id.as_str();
            match view {
                GpuView::Usage => {
                    let headline = g
                        .usage
                        .map(|u| format::percent(f64::from(u)))
                        .unwrap_or_default();
                    let detail = g
                        .core_mhz
                        .map(|m| format!("{m:.0} MHz"))
                        .unwrap_or_default();
                    let key = format!("gpu-usage:{id}");
                    card(ui, palette, &title, &headline, &detail, |ui| {
                        let line = GraphLine {
                            key: &key,
                            name: "Usage",
                            color: colors.cpu,
                        };
                        let h = graph_height(ui);
                        graph(
                            ui,
                            palette,
                            ("plot:gpu-usage", id),
                            history,
                            &[line],
                            Scale::Percent,
                            h,
                        );
                    });
                }
                GpuView::Power => {
                    let headline = g.power_w.map(|w| format!("{w:.0} W")).unwrap_or_default();
                    let detail = g
                        .power_cap_w
                        .map(|c| format!("of {c:.0} W"))
                        .unwrap_or_default();
                    let key = format!("gpu-power:{id}");
                    let scale = Scale::Watts {
                        floor: f64::from(g.power_cap_w.unwrap_or(0.0)),
                    };
                    card(ui, palette, &title, &headline, &detail, |ui| {
                        let line = GraphLine {
                            key: &key,
                            name: "Power",
                            color: colors.cpu,
                        };
                        let h = graph_height(ui);
                        graph(
                            ui,
                            palette,
                            ("plot:gpu-power", id),
                            history,
                            &[line],
                            scale,
                            h,
                        );
                    });
                }
                GpuView::Memory => {
                    let bytes = |b: u64| format::bytes(b as f64);
                    let system = g.system_used.unwrap_or(0);
                    let (headline, detail, system_name) = if g.integrated {
                        (
                            bytes(system),
                            "shared with system RAM".to_owned(),
                            "Shared memory",
                        )
                    } else {
                        let headline = match (g.vram_used, g.vram_total) {
                            (Some(u), Some(t)) => format!("{} / {}", bytes(u), bytes(t)),
                            (Some(u), None) => bytes(u),
                            _ => String::new(),
                        };
                        let detail = if system > 0 {
                            format!("+{} in RAM", bytes(system))
                        } else {
                            String::new()
                        };
                        (headline, detail, "Spilled to RAM")
                    };
                    let (vram_key, sys_key) =
                        (format!("gpu-mem:{id}:vram"), format!("gpu-mem:{id}:sys"));
                    let scale = Scale::Bytes {
                        floor: g.vram_total.unwrap_or(0) as f64,
                    };
                    card(ui, palette, &title, &headline, &detail, |ui| {
                        let mut lines = Vec::new();
                        if !g.integrated && g.vram_used.is_some() {
                            lines.push(GraphLine {
                                key: &vram_key,
                                name: "VRAM",
                                color: colors.ram,
                            });
                        }
                        if g.system_used.is_some() {
                            lines.push(GraphLine {
                                key: &sys_key,
                                name: system_name,
                                color: colors.swap,
                            });
                        }
                        let h = graph_height(ui);
                        graph(ui, palette, ("plot:gpu-mem", id), history, &lines, scale, h);
                    });
                }
                GpuView::Temp => {
                    let hottest = g.temps.iter().map(|(_, t)| *t).fold(f32::MIN, f32::max);
                    let headline = format!("{hottest:.0}°C");
                    let detail = if g.temps.len() > 1 {
                        g.temps
                            .iter()
                            .map(|(label, t)| format!("{label} {t:.0}°"))
                            .collect::<Vec<_>>()
                            .join(" · ")
                    } else {
                        String::new()
                    };
                    let keys: Vec<String> = g
                        .temps
                        .iter()
                        .map(|(label, _)| format!("gpu-temp:{id}:{label}"))
                        .collect();
                    let colors_cycle = [palette.accent, palette.accent2, palette.load[2]];
                    card(ui, palette, &title, &headline, &detail, |ui| {
                        let lines: Vec<GraphLine<'_>> = g
                            .temps
                            .iter()
                            .zip(&keys)
                            .enumerate()
                            .map(|(i, ((label, _), key))| GraphLine {
                                key,
                                name: label,
                                color: colors_cycle[i % colors_cycle.len()],
                            })
                            .collect();
                        let h = graph_height(ui);
                        graph(
                            ui,
                            palette,
                            ("plot:gpu-temp", id),
                            history,
                            &lines,
                            Scale::Celsius,
                            h,
                        );
                    });
                }
            }
        }
        Card::Drive(d) => {
            let headline = if enabled.contains(DiskIo) {
                format!(
                    "R {}  W {}",
                    format::rate(d.read_bps),
                    format::rate(d.write_bps)
                )
            } else {
                String::new()
            };
            let detail = [
                d.temp_c
                    .filter(|_| enabled.contains(DiskTemp))
                    .map(|t| format!("{t:.0}°C")),
                enabled
                    .contains(DiskIops)
                    .then(|| format!("{:.0} IOPS", d.iops)),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            let (r, w) = (format!("disk:{}:r", d.name), format!("disk:{}:w", d.name));
            card(ui, palette, &title, &headline, &detail, |ui| {
                if enabled.contains(DiskIo) {
                    let lines = [
                        GraphLine {
                            key: &r,
                            name: "Read",
                            color: colors.read,
                        },
                        GraphLine {
                            key: &w,
                            name: "Write",
                            color: colors.write,
                        },
                    ];
                    let h = graph_height(ui);
                    let id = ("plot:drive", d.id.as_str());
                    graph(ui, palette, id, history, &lines, Scale::BytesPerSec, h);
                }
                if enabled.contains(DiskSpace) {
                    ui.add_space(SECTION_GAP);
                    for fs in &d.partitions {
                        ui.label(RichText::new(format!("{}  ·  {}", fs.mount, fs.fstype)).small());
                        usage_bar(
                            ui,
                            fs.used as f32 / fs.total as f32,
                            format!(
                                "{} / {} used",
                                format::bytes(fs.used as f64),
                                format::bytes(fs.total as f64)
                            ),
                            colors.read,
                        );
                        ui.add_space(4.0);
                    }
                }
            });
        }
    }
}

fn ratio_bar(ui: &mut egui::Ui, label: &str, used: u64, total: u64, color: egui::Color32) {
    let frac = used as f32 / total as f32;
    usage_bar(
        ui,
        frac,
        format!(
            "{label}  {} / {}  ({:.0}%)",
            format::bytes(used as f64),
            format::bytes(total as f64),
            frac * 100.0
        ),
        color,
    );
}
