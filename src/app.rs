use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::config::Config;
use crate::frame_limit::FrameLimiter;
use crate::history::History;
use crate::main_window::MainWindow;
use crate::metrics::MetricId;
use crate::sampler::sensors::{self, PowerAccess};
use crate::sampler::{self, Control, SamplerHandle};
use crate::ui::theme_editor::{self, ThemeEditor};
use crate::ui::{dashboard, settings, theme};

const SAVE_DEBOUNCE: Duration = Duration::from_millis(600);

pub struct HyperionApp {
    config: Config,
    themes: theme::Library,
    /// Open while creating or editing a custom theme; its draft is previewed live.
    theme_editor: Option<ThemeEditor>,
    /// The theme currently applied to egui, to re-apply only when it changes.
    applied: theme::ThemeSpec,
    palette: theme::Palette,
    dirty_since: Option<Instant>,
    history: Arc<Mutex<History>>,
    sampler: SamplerHandle,
    show_settings: bool,
    board: dashboard::TileBoard,
    main_window: MainWindow,
    power_access: PowerAccess,
    power_checked: Instant,
    /// Running permission request for CPU power (pkexec), and its last error.
    grant: Option<mpsc::Receiver<Result<(), String>>>,
    grant_error: Option<String>,
    frames: FrameLimiter,
}

impl HyperionApp {
    pub fn new(cc: &eframe::CreationContext<'_>, config: Config) -> Self {
        let (themes, problems) = theme::Library::load(Config::themes_dir());
        for problem in problems {
            eprintln!("hyperion: skipping theme file {problem}");
        }
        let applied = themes.get(&config.theme).spec.clone();
        let palette = applied.palette();
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
            themes,
            theme_editor: None,
            applied,
            palette,
            dirty_since: None,
            history,
            sampler,
            show_settings: false,
            board: dashboard::TileBoard::default(),
            main_window: MainWindow::detect(),
            power_access: sensors::power_access(),
            power_checked: Instant::now(),
            grant: None,
            grant_error: None,
            frames: FrameLimiter::new(FrameLimiter::display_hz()),
        }
    }

    fn apply_changes(&mut self, before: &Config) {
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

    /// Shows the theme editor if open, and applies whichever theme should be visible:
    /// the editor's draft while editing, otherwise the selected theme.
    fn update_theme(&mut self, ctx: &egui::Context) {
        if let Some(editor) = &mut self.theme_editor {
            match editor.show(ctx, &mut self.themes) {
                theme_editor::Outcome::Editing => {}
                theme_editor::Outcome::Saved(key) => {
                    self.config.theme = key;
                    self.theme_editor = None;
                }
                theme_editor::Outcome::Cancelled => self.theme_editor = None,
                theme_editor::Outcome::Deleted => {
                    self.config.theme = theme::DEFAULT_KEY.to_owned();
                    self.theme_editor = None;
                }
            }
        }
        let wanted = match &self.theme_editor {
            Some(editor) => editor.draft(),
            None => &self.themes.get(&self.config.theme).spec,
        };
        if *wanted != self.applied {
            self.applied = wanted.clone();
            self.palette = self.applied.palette();
            theme::apply(ctx, &self.palette);
            ctx.request_repaint();
        }
    }

    /// Keeps the CPU power permission state current and collects the result of a
    /// running permission request.
    fn update_power_access(&mut self) {
        if let Some(rx) = &self.grant
            && let Ok(result) = rx.try_recv()
        {
            self.grant = None;
            self.grant_error = result.err();
            self.power_checked = Instant::now() - Duration::from_secs(60);
        }
        if self.power_checked.elapsed() >= Duration::from_secs(2) {
            self.power_checked = Instant::now();
            self.power_access = sensors::power_access();
        }
    }

    /// Asks for admin permission via the desktop's password prompt (pkexec), in the
    /// background so the dashboard keeps updating.
    fn request_power_permission(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(grant_cpu_power());
            ctx.request_repaint();
        });
        self.grant = Some(rx);
        self.grant_error = None;
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

#[cfg(target_os = "linux")]
fn grant_cpu_power() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let status = std::process::Command::new("pkexec")
        .arg(exe)
        .args([crate::privileged::FLAG, "cpu-power"])
        .status()
        .map_err(|_| {
            "pkexec isn't installed. Run `sudo hyperion --grant-sensors cpu-power` instead."
                .to_owned()
        })?;
    match status.code() {
        Some(0) => Ok(()),
        // pkexec: 126 = dialog dismissed, 127 = not authorized.
        Some(126 | 127) => Err("Permission wasn't granted.".to_owned()),
        _ => Err("Granting permission failed.".to_owned()),
    }
}

#[cfg(not(target_os = "linux"))]
fn grant_cpu_power() -> Result<(), String> {
    Err("Not supported on this platform yet.".to_owned())
}

impl eframe::App for HyperionApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frames.wait();
        let before = self.config.clone();
        let palette = self.palette;
        let ctx = ui.ctx().clone();

        // Closing the main window only hides it while tiles are popped out; they keep
        // running. With nothing popped out, closing quits as usual.
        if ui.input(|i| i.viewport().close_requested()) && !self.config.detached_tiles.is_empty() {
            ui.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.main_window.hide(&ctx);
        }
        self.main_window
            .observe_focus(ui.input(|i| i.viewport().focused));
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

        self.update_power_access();
        if self.show_settings {
            let history = self.history.lock().unwrap_or_else(PoisonError::into_inner);
            let any_dedicated = history.latest().gpus.iter().any(|g| !g.integrated);
            let gpus: Vec<settings::Device> = history
                .latest()
                .gpus
                .iter()
                .map(|g| settings::Device {
                    id: g.id.clone(),
                    label: format!(
                        "{} ({})",
                        g.name,
                        if g.integrated {
                            "integrated"
                        } else {
                            "dedicated"
                        }
                    ),
                    default_on: !g.integrated || !any_dedicated,
                })
                .collect();
            let drives: Vec<settings::Device> = history
                .latest()
                .drives
                .iter()
                .map(|d| settings::Device {
                    id: d.id.clone(),
                    label: if d.boot {
                        format!("{} (system drive)", d.model)
                    } else {
                        d.model.clone()
                    },
                    default_on: d.boot,
                })
                .collect();
            drop(history);
            let request = egui::Panel::right("settings")
                .frame(
                    egui::Frame::new()
                        .fill(palette.well.gamma_multiply(0.9))
                        .inner_margin(egui::Margin::symmetric(16, 8)),
                )
                .resizable(false)
                .exact_size(280.0)
                .show(ui, |ui| {
                    let status = settings::CpuPowerStatus {
                        access: self.power_access,
                        waiting: self.grant.is_some(),
                        error: self.grant_error.as_deref(),
                    };
                    settings::show(ui, &mut self.config, &self.themes, &status, &drives, &gpus)
                });
            match request.inner {
                Some(settings::Request::NewTheme(key)) => {
                    self.theme_editor = Some(ThemeEditor::new_from(self.themes.get(&key)));
                }
                Some(settings::Request::EditTheme(key)) => {
                    self.theme_editor = Some(ThemeEditor::edit(self.themes.get(&key)));
                }
                Some(settings::Request::GrantCpuPower) => self.request_power_permission(&ctx),
                None => {}
            }
        }

        let enabled = self.config.enabled();
        let shown = egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 8)))
            .show(ui, |ui| {
                let history = self.history.lock().unwrap_or_else(PoisonError::into_inner);
                let view = dashboard::View {
                    history: &history,
                    enabled,
                    palette: &palette,
                    drives: &self.config.drives,
                    gpus: &self.config.gpus,
                };
                dashboard::show(
                    ui,
                    view,
                    dashboard::Arrangement {
                        order: &mut self.config.tile_order,
                        detached: &mut self.config.detached_tiles,
                        spans: &mut self.config.tile_spans,
                    },
                    &mut self.board,
                    self.main_window.is_hidden(),
                )
            })
            .inner;

        // Bring the main window back when a pop-out asks, or when the last pop-out
        // was closed so nothing would be left on screen.
        if self.main_window.is_hidden()
            && (shown.restore_main || self.config.detached_tiles.is_empty())
        {
            self.main_window.show(&ctx);
        }

        self.update_theme(&ctx);
        if self.config != before {
            self.apply_changes(&before);
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
