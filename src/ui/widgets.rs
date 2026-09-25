use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, RichText, Sense, vec2};
use egui_plot::{GridMark, HoverPosition, Line, Plot};

use super::theme;
use crate::format;
use crate::history::{History, RETAIN_SECS};

/// Height of the usage bars inside tiles.
pub const BAR_HEIGHT: f32 = 18.0;
/// Height of one cell in the per-core CPU grid.
pub const CORE_CELL_HEIGHT: f32 = 20.0;
/// Card inner margin plus its 1px border, per side.
pub const CARD_INSET: f32 = 15.0;

/// Draws a tile that fills all the space its parent gives it, so tiles stretch with
/// the window. `body` gets whatever height is left under the title row.
/// `detail` is secondary info shown dimmer, just left of `headline` (may be empty).
pub fn card(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    title: &str,
    headline: &str,
    detail: &str,
    body: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new()
        .fill(palette.card)
        .stroke(egui::Stroke::new(1.0, palette.card_stroke))
        .corner_radius(14)
        .inner_margin(14)
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.horizontal(|ui| {
                // Right side first, so a long title gets what's left and is elided
                // ("…", full text on hover) instead of running into the numbers.
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(headline)
                            .monospace()
                            .size(15.0)
                            .strong()
                            .color(palette.accent),
                    );
                    if !detail.is_empty() {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(detail)
                                .monospace()
                                .size(12.0)
                                .color(palette.text_dim),
                        );
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(title).strong().size(15.0).color(palette.text),
                            )
                            .truncate(),
                        );
                    });
                });
            });
            ui.add_space(6.0);
            body(ui);
        });
}

pub struct GraphLine<'a> {
    pub key: &'a str,
    pub name: &'a str,
    pub color: Color32,
}

#[derive(Clone, Copy)]
pub enum Scale {
    Percent,
    BytesPerSec,
    /// Amount of memory; the axis spans at least `floor` (e.g. total VRAM) and grows
    /// when a line goes above it.
    Bytes {
        floor: f64,
    },
    /// Power; the axis spans at least `floor` (e.g. the power cap).
    Watts {
        floor: f64,
    },
    Celsius,
}

impl Scale {
    fn format(self, v: f64) -> String {
        match self {
            Scale::Percent => format::percent(v),
            Scale::BytesPerSec => format::rate(v),
            Scale::Bytes { .. } => format::bytes(v),
            Scale::Watts { .. } => format!("{v:.0} W"),
            Scale::Celsius => format!("{v:.0}°C"),
        }
    }

    /// Top of the y axis for data peaking at `peak`.
    fn ceiling(self, peak: f64) -> f64 {
        match self {
            Scale::Percent => 100.0,
            Scale::BytesPerSec => bytes_ceiling(peak),
            Scale::Bytes { floor } if peak <= floor && floor > 0.0 => floor,
            // Past the floor: next power of two, so quarter lines stay round.
            Scale::Bytes { .. } => bytes_ceiling(peak),
            // Round up to 20 W steps so quarter lines are whole watts.
            Scale::Watts { floor } => (peak.max(floor).max(20.0) / 20.0).ceil() * 20.0,
            Scale::Celsius => (peak.max(100.0) / 10.0).ceil() * 10.0,
        }
    }
}

/// Width of the y-axis labels beside every graph.
const Y_AXIS_WIDTH: f32 = 64.0;
/// A graph this wide (plot area, without the axis) shows exactly the History setting;
/// wider ones show proportionally further back, at the same time per pixel. About a
/// one-column tile in the default window.
const REFERENCE_GRAPH_WIDTH: f32 = 420.0;

/// Seconds shown by a graph whose plot area is `plot_width` wide.
pub fn graph_span(setting_secs: f64, plot_width: f32) -> f64 {
    (setting_secs * f64::from(plot_width / REFERENCE_GRAPH_WIDTH)).min(RETAIN_SECS)
}

pub fn graph(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    id: impl egui::AsId,
    history: &History,
    lines: &[GraphLine<'_>],
    scale: Scale,
    height: f32,
) {
    let plot_width = (ui.available_width() - Y_AXIS_WIDTH).max(1.0);
    let buckets = (plot_width as usize).max(32);
    let window = graph_span(history.window(), plot_width);
    let series: Vec<_> = lines
        .iter()
        .map(|l| {
            (
                history.plot_points(l.key, window, buckets),
                l.name.to_owned(),
                l.color,
            )
        })
        .collect();
    let peak = series
        .iter()
        .flat_map(|(pts, ..)| pts.iter().map(|p| p[1]))
        .fold(0.0, f64::max);
    let y_max = scale.ceiling(peak);

    Plot::new(id)
        .height(height)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .allow_double_click_reset(false)
        .allow_axis_zoom_drag(false)
        // Hover only (default is click+drag): the tooltip keeps working, but a drag that
        // starts on the graph goes to the tile underneath so the tile can be moved.
        .sense(Sense::hover())
        .show_x(false)
        .show_y(false)
        .show_axes([false, true])
        .show_grid([false, true])
        .show_background(false)
        .grid_color(palette.grid)
        .y_axis_min_width(Y_AXIS_WIDTH)
        .y_axis_formatter(move |mark, _| scale.format(mark.value))
        .y_grid_spacer(move |_| grid_marks(y_max, height))
        .label_formatter(move |pos| match pos {
            HoverPosition::NearDataPoint {
                plot_name,
                position,
                ..
            } => Some(format!(
                "{plot_name}: {}\n{:.0} s ago",
                scale.format(position.y),
                -position.x
            )),
            HoverPosition::Elsewhere { .. } => None,
        })
        .show(ui, |plot| {
            plot.set_plot_bounds_x(-window..=0.0);
            plot.set_plot_bounds_y(0.0..=y_max);
            for (pts, name, color) in series {
                let line = Line::new(name, pts).color(color).width(2.0);
                // Temperatures sit far above 0, so a fill down to the axis is just a
                // solid block; show them as plain lines.
                plot.line(if matches!(scale, Scale::Celsius) {
                    line
                } else {
                    line.fill(0.0).fill_alpha(0.18)
                });
            }
        });
}

/// Smallest power of two ≥ `peak` (min 1 KiB/s), so grid lines land on round binary units.
fn bytes_ceiling(peak: f64) -> f64 {
    let floor = 1024.0;
    if peak <= floor {
        return floor;
    }
    2f64.powi(peak.log2().ceil() as i32)
}

/// Evenly spaced y-axis marks. Short graphs get halves instead of quarters: egui_plot
/// hides labels that would sit too close together, which left short graphs unlabeled.
fn grid_marks(max: f64, graph_height: f32) -> Vec<GridMark> {
    let divisions: u8 = if graph_height >= 110.0 { 4 } else { 2 };
    (0..=divisions)
        .map(|i| GridMark {
            value: max * f64::from(i) / f64::from(divisions),
            step_size: max / f64::from(divisions),
        })
        .collect()
}

pub fn usage_bar(ui: &mut egui::Ui, fraction: f32, text: String, color: Color32) {
    ui.add(
        egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
            .text(text)
            .fill(color.gamma_multiply(0.6))
            .desired_height(BAR_HEIGHT)
            .corner_radius(4),
    );
}

fn core_columns(width: f32, spacing: f32) -> usize {
    ((width + spacing) / (84.0 + spacing))
        .floor()
        .clamp(1.0, 8.0) as usize
}

/// Height `core_grid` will take for `cores` cells at this width.
pub fn core_grid_height(cores: usize, width: f32, spacing: egui::Vec2) -> f32 {
    let rows = cores.div_ceil(core_columns(width, spacing.x)) as f32;
    if rows == 0.0 {
        return 0.0;
    }
    rows * CORE_CELL_HEIGHT + (rows - 1.0) * spacing.y
}

/// How fast a per-core bar falls, in percentage points per second. It rises
/// instantly, so short spikes show, then glides down like a VU meter.
const METER_FALL_PER_SEC: f32 = 40.0;
/// Redraw interval while a bar is falling (30 fps).
const METER_FRAME: std::time::Duration = std::time::Duration::from_millis(33);

/// On-screen levels of a row of meters.
#[derive(Clone, Default)]
struct Meter {
    levels: Vec<f32>,
    last: f64,
}

impl Meter {
    /// Moves the levels toward `targets` for time `now` (seconds). Returns whether any
    /// bar is still falling, i.e. whether another frame is needed.
    fn update(&mut self, targets: &[f32], now: f64) -> bool {
        if self.levels.len() != targets.len() {
            self.levels = targets.to_vec();
            self.last = now;
            return false;
        }
        // Capped so a long gap between frames still shows a glide, not a jump.
        let dt = (now - self.last).clamp(0.0, 0.25) as f32;
        self.last = now;
        let mut falling = false;
        for (level, &target) in self.levels.iter_mut().zip(targets) {
            if target >= *level {
                *level = target;
            } else {
                *level = (*level - METER_FALL_PER_SEC * dt).max(target);
                falling |= *level > target;
            }
        }
        falling
    }
}

/// Grid of per-core usage cells. Bars behave like a VU meter and take their colour
/// from the theme's load scale; the text shows the actual reading.
pub fn core_grid(ui: &mut egui::Ui, values: &[f32], palette: &theme::Palette) {
    let spacing = ui.spacing().item_spacing.x;
    let width = ui.available_width();
    let cols = core_columns(width, spacing);
    let cell_w = (width - spacing * (cols as f32 - 1.0)) / cols as f32;
    let bg = ui.visuals().extreme_bg_color;
    let text_color = ui.visuals().text_color();

    // One CPU tile exists, so a fixed id keeps the meter smooth even when the tile
    // moves between the grid, the drag layer and a pop-out window.
    let id = egui::Id::new("hyperion-core-meter");
    let now = ui.input(|i| i.time);
    let (levels, falling) = ui.ctx().data_mut(|d| {
        let meter = d.get_temp_mut_or_default::<Meter>(id);
        let falling = meter.update(values, now);
        (meter.levels.clone(), falling)
    });
    if falling {
        // 30 steps a second is smooth for a bar falling 40% per second, and half
        // the cost of redrawing at the display's full rate.
        ui.ctx().request_repaint_after(METER_FRAME);
    }

    for (row, chunk) in values.chunks(cols).enumerate() {
        ui.horizontal(|ui| {
            for (i, &v) in chunk.iter().enumerate() {
                let index = row * cols + i;
                let level = levels.get(index).copied().unwrap_or(v);
                let (rect, _) =
                    ui.allocate_exact_size(vec2(cell_w, CORE_CELL_HEIGHT), Sense::hover());
                let painter = ui.painter();
                painter.rect_filled(rect, 4, bg);
                let mut fill = rect;
                fill.set_width(rect.width() * (level / 100.0).clamp(0.0, 1.0));
                painter.rect_filled(fill, 4, palette.load_color(level).gamma_multiply(0.85));
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    format!("{index:>2}  {v:>3.0}%"),
                    FontId::monospace(11.0),
                    text_color,
                );
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wider_graphs_show_further_back_at_the_same_time_per_pixel() {
        assert_eq!(graph_span(60.0, REFERENCE_GRAPH_WIDTH), 60.0);
        assert_eq!(graph_span(60.0, REFERENCE_GRAPH_WIDTH * 2.0), 120.0);
        assert_eq!(graph_span(60.0, REFERENCE_GRAPH_WIDTH / 2.0), 30.0);
        // Never beyond what is kept.
        assert_eq!(graph_span(1800.0, REFERENCE_GRAPH_WIDTH * 3.0), RETAIN_SECS);
    }

    #[test]
    fn meter_rises_instantly_and_falls_at_a_steady_rate() {
        let mut m = Meter::default();
        assert!(!m.update(&[10.0, 10.0], 0.0));
        // Jumps up at once.
        assert!(!m.update(&[90.0, 10.0], 0.1));
        assert_eq!(m.levels, [90.0, 10.0]);
        // Falls 40 points per second, not straight to the new reading.
        assert!(m.update(&[0.0, 10.0], 0.35));
        assert_eq!(m.levels[0], 80.0);
        // A long gap still only advances 0.25 s, so the fall stays visible.
        assert!(m.update(&[0.0, 10.0], 5.0));
        assert_eq!(m.levels[0], 70.0);
        // And it stops exactly at the reading.
        for step in 1..=20 {
            m.update(&[0.0, 10.0], 5.0 + f64::from(step) * 0.25);
        }
        assert_eq!(m.levels[0], 0.0);
    }

    #[test]
    fn core_grid_height_matches_rows() {
        let spacing = egui::vec2(8.0, 3.0);
        // 440px fits 4 cells of 84px (+8px gaps) per row -> 12 cores = 3 rows.
        assert_eq!(core_grid_height(12, 440.0, spacing), 3.0 * 20.0 + 2.0 * 3.0);
        assert_eq!(core_grid_height(0, 440.0, spacing), 0.0);
    }

    #[test]
    fn short_graphs_get_fewer_axis_marks() {
        let values = |h| {
            grid_marks(100.0, h)
                .iter()
                .map(|m| m.value)
                .collect::<Vec<_>>()
        };
        assert_eq!(values(200.0), [0.0, 25.0, 50.0, 75.0, 100.0]);
        assert_eq!(values(70.0), [0.0, 50.0, 100.0]);
    }

    #[test]
    fn memory_axis_spans_vram_until_something_exceeds_it() {
        let gib = 1024.0 * 1024.0 * 1024.0;
        let vram = Scale::Bytes { floor: 8.0 * gib };
        assert_eq!(vram.ceiling(2.0 * gib), 8.0 * gib);
        // Spill larger than VRAM: the axis grows to fit it.
        assert_eq!(vram.ceiling(11.0 * gib), 16.0 * gib);
    }

    #[test]
    fn power_and_temperature_axes() {
        assert_eq!(Scale::Watts { floor: 120.0 }.ceiling(15.0), 120.0);
        assert_eq!(Scale::Watts { floor: 120.0 }.ceiling(131.0), 140.0);
        assert_eq!(Scale::Watts { floor: 0.0 }.ceiling(3.0), 20.0);
        assert_eq!(Scale::Celsius.ceiling(45.0), 100.0);
        assert_eq!(Scale::Celsius.ceiling(104.0), 110.0);
    }

    #[test]
    fn bytes_ceiling_is_power_of_two() {
        assert_eq!(bytes_ceiling(0.0), 1024.0);
        assert_eq!(bytes_ceiling(1500.0), 2048.0);
        assert_eq!(bytes_ceiling(3.0 * 1024.0 * 1024.0), 4.0 * 1024.0 * 1024.0);
    }
}
