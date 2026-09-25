use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, RichText, Sense, vec2};
use egui_plot::{GridMark, HoverPosition, Line, Plot};

use super::theme;
use crate::format;
use crate::history::History;

pub struct MetricColors {
    pub cpu: Color32,
    pub ram: Color32,
    pub swap: Color32,
    pub read: Color32,
    pub write: Color32,
    pub rx: Color32,
    pub tx: Color32,
}

/// Maps a theme's two accent colors onto each metric's primary/secondary line.
pub fn metric_colors(p: &theme::Palette) -> MetricColors {
    MetricColors {
        cpu: p.accent,
        ram: p.accent,
        swap: p.accent2,
        read: p.accent2,
        write: p.accent,
        rx: p.accent2,
        tx: p.accent,
    }
}

/// Draws a tile and returns the drag response for its header (the drag handle).
pub fn card(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    id: egui::Id,
    title: &str,
    headline: &str,
    body: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let mut header_rect = egui::Rect::NOTHING;
    egui::Frame::new()
        .fill(palette.card)
        .stroke(egui::Stroke::new(1.0, palette.card_stroke))
        .corner_radius(14)
        .inner_margin(14)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let header = ui.horizontal(|ui| {
                ui.label(RichText::new(title).strong().size(15.0).color(palette.text));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(headline)
                            .monospace()
                            .size(15.0)
                            .strong()
                            .color(palette.accent),
                    );
                });
            });
            header_rect = header.response.rect;
            ui.add_space(6.0);
            body(ui);
        });
    // The header is the drag handle: dragging by the title bar can't steal hover
    // from interactive content in the body, such as the graph's hover tooltip.
    ui.interact(header_rect, id, Sense::click_and_drag())
        .on_hover_and_drag_cursor(egui::CursorIcon::Grab)
}

/// Dashed outline left in a tile's grid slot while its real content floats at the cursor.
pub fn drag_placeholder(ui: &mut egui::Ui, palette: &theme::Palette, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    ui.painter().rect_stroke(
        rect,
        14,
        egui::Stroke::new(1.5, palette.accent),
        egui::StrokeKind::Middle,
    );
}

/// Small floating label that follows the cursor while a tile is being dragged.
pub fn drag_ghost(ctx: &egui::Context, palette: &theme::Palette, pos: egui::Pos2, title: &str) {
    egui::Area::new(egui::Id::new("hyperion-drag-ghost"))
        .order(egui::Order::Tooltip)
        .movable(false)
        .interactable(false)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(palette.card)
                .stroke(egui::Stroke::new(1.5, palette.accent))
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.label(RichText::new(title).strong().color(palette.text));
                });
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
}

impl Scale {
    fn format(self, v: f64) -> String {
        match self {
            Scale::Percent => format::percent(v),
            Scale::BytesPerSec => format::rate(v),
        }
    }
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
    let buckets = (ui.available_width() as usize).max(32);
    let window = history.window();
    let series: Vec<_> = lines
        .iter()
        .map(|l| {
            (
                history.plot_points(l.key, buckets),
                l.name.to_owned(),
                l.color,
            )
        })
        .collect();
    let y_max = match scale {
        Scale::Percent => 100.0,
        Scale::BytesPerSec => {
            let peak = series
                .iter()
                .flat_map(|(pts, ..)| pts.iter().map(|p| p[1]))
                .fold(0.0, f64::max);
            bytes_ceiling(peak)
        }
    };

    Plot::new(id)
        .height(height)
        .allow_drag(false)
        .allow_zoom(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .allow_double_click_reset(false)
        .show_x(false)
        .show_y(false)
        .show_axes([false, true])
        .show_grid([false, true])
        .show_background(false)
        .grid_color(palette.card_stroke)
        .y_axis_min_width(64.0)
        .y_axis_formatter(move |mark, _| scale.format(mark.value))
        .y_grid_spacer(move |_| quarter_marks(y_max))
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
                plot.line(
                    Line::new(name, pts)
                        .color(color)
                        .width(2.0)
                        .fill(0.0)
                        .fill_alpha(0.18),
                );
            }
        });
}

/// Smallest power of two ≥ `peak` (min 1 KiB/s), so quarter grid lines land on round binary units.
fn bytes_ceiling(peak: f64) -> f64 {
    let floor = 1024.0;
    if peak <= floor {
        return floor;
    }
    2f64.powi(peak.log2().ceil() as i32)
}

fn quarter_marks(max: f64) -> Vec<GridMark> {
    (0..=4)
        .map(|i| GridMark {
            value: max * f64::from(i) / 4.0,
            step_size: max / 4.0,
        })
        .collect()
}

pub fn usage_bar(ui: &mut egui::Ui, fraction: f32, text: String, color: Color32) {
    ui.add(
        egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
            .text(text)
            .fill(color.gamma_multiply(0.6))
            .desired_height(18.0)
            .corner_radius(4),
    );
}

/// Compact grid of per-core usage cells, each filled proportionally to its load.
pub fn core_grid(ui: &mut egui::Ui, values: &[f32], color: Color32) {
    let spacing = ui.spacing().item_spacing.x;
    let width = ui.available_width();
    let cols = ((width + spacing) / (84.0 + spacing))
        .floor()
        .clamp(1.0, 8.0) as usize;
    let cell_w = (width - spacing * (cols as f32 - 1.0)) / cols as f32;
    let bg = ui.visuals().extreme_bg_color;
    let text_color = ui.visuals().text_color();

    for (row, chunk) in values.chunks(cols).enumerate() {
        ui.horizontal(|ui| {
            for (i, &v) in chunk.iter().enumerate() {
                let (rect, _) = ui.allocate_exact_size(vec2(cell_w, 20.0), Sense::hover());
                let painter = ui.painter();
                painter.rect_filled(rect, 4, bg);
                let mut fill = rect;
                fill.set_width(rect.width() * (v / 100.0).clamp(0.0, 1.0));
                painter.rect_filled(fill, 4, color.gamma_multiply(0.75));
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    format!("{:>2}  {:>3.0}%", row * cols + i, v),
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
    fn bytes_ceiling_is_power_of_two() {
        assert_eq!(bytes_ceiling(0.0), 1024.0);
        assert_eq!(bytes_ceiling(1500.0), 2048.0);
        assert_eq!(bytes_ceiling(3.0 * 1024.0 * 1024.0), 4.0 * 1024.0 * 1024.0);
    }
}
