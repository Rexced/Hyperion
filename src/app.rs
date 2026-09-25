use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::config::Config;
use crate::history::History;
use crate::metrics::MetricId;
use crate::sampler::{self, Control, SamplerHandle};
use crate::ui::{dashboard, settings, theme};

const SAVE_DEBOUNCE: Duration = Duration::from_millis(600);

pub struct HyperionApp {
    config: Config,
    palette: theme::Palette,
    dirty_since: Option<Instant>,
    history: Arc<Mutex<History>>,
    sampler: SamplerHandle,
    show_settings: bool,
    board: dashboard::TileBoard,
}

impl HyperionApp {
    pub fn new(cc: &eframe::CreationContext<'_>, config: Config) -> Self {
        let palette = config.theme_id().palette();
        theme::apply(&cc.egui_ctx, &palette);
        let history = Arc::new(Mutex::new(History::new(config.history_secs as f64)));
        let sampler = sampler::spawn(
            cc.egui_ctx.clone(),
            Arc::clone(&history),
            Duration::from_millis(config.interval_ms),
            config.enabled(),
        );
        Self {
            config,
            palette,
            dirty_since: None,
            history,
            sampler,
            show_settings: false,
            board: dashboard::TileBoard::default(),
        }
    }

    fn apply_changes(&mut self, before: &Config, ctx: &egui::Context) {
        if self.config.theme != before.theme {
            self.palette = self.config.theme_id().palette();
            theme::apply(ctx, &self.palette);
        }
        if self.config.interval_ms != before.interval_ms {
            self.sampler.send(Control::Interval(Duration::from_millis(
                self.config.interval_ms,
            )));
        }
        let (old, new) = (before.enabled(), self.config.enabled());
        let mut history = self.history.lock().unwrap_or_else(PoisonError::into_inner);
        if old != new {
            self.sampler.send(Control::Enabled(new));
            for id in MetricId::ALL {
                if old.contains(id)
                    && !new.contains(id)
                    && let Some(prefix) = id.series_prefix()
                {
                    history.clear_prefix(prefix);
                }
            }
        }
        if self.config.history_secs != before.history_secs {
            history.set_window(self.config.history_secs as f64);
        }
        self.dirty_since = Some(Instant::now());
    }

    fn save_if_due(&mut self, ctx: &egui::Context, force: bool) {
        let Some(since) = self.dirty_since else {
            return;
        };
        let elapsed = since.elapsed();
        if force || elapsed >= SAVE_DEBOUNCE {
            if let Err(e) = self.config.save() {
                eprintln!("hyperion: failed to save config: {e:#}");
            }
            self.dirty_since = None;
        } else {
            ctx.request_repaint_after(SAVE_DEBOUNCE - elapsed);
        }
    }
}

impl eframe::App for HyperionApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let before = self.config.clone();
        let palette = self.palette;
        theme::paint_background(ui, &palette);

        egui::Panel::top("top_bar")
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 10)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    ui.painter()
                        .circle_filled(dot.center(), 5.0, palette.accent);
                    ui.label(
                        egui::RichText::new("HYPERION")
                            .size(20.0)
                            .strong()
                            .extra_letter_spacing(3.0)
                            .color(palette.text),
                    );
                    ui.label(
                        egui::RichText::new("system monitor")
                            .size(13.0)
                            .color(palette.text_dim),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.toggle_value(&mut self.show_settings, "⚙ Settings");
                    });
                });
            });

        if self.show_settings {
            egui::Panel::right("settings")
                .frame(
                    egui::Frame::new()
                        .fill(palette.well.gamma_multiply(0.9))
                        .inner_margin(egui::Margin::symmetric(16, 8)),
                )
                .resizable(false)
                .exact_size(280.0)
                .show(ui, |ui| settings::show(ui, &mut self.config));
        }

        let enabled = self.config.enabled();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 8)))
            .show(ui, |ui| {
                let history = self.history.lock().unwrap_or_else(PoisonError::into_inner);
                dashboard::show(
                    ui,
                    &history,
                    enabled,
                    &palette,
                    &mut self.config.tile_order,
                    &mut self.config.detached_tiles,
                    &mut self.board,
                );
            });

        let ctx = ui.ctx().clone();
        if self.config != before {
            self.apply_changes(&before, &ctx);
        }
        self.save_if_due(&ctx, false);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.dirty_since.is_some()
            && let Err(e) = self.config.save()
        {
            eprintln!("hyperion: failed to save config: {e:#}");
        }
    }
}
