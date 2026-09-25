use eframe::egui::{self, RichText};

use crate::config::{Config, HISTORY_STEPS_SECS, INTERVAL_STEPS_MS, nearest_step};
use crate::format;
use crate::metrics::{Category, MetricId};
use crate::sampler::sensors::PowerAccess;
use crate::ui::theme::Library;

/// Something the settings panel wants the app to open.
pub enum Request {
    /// Theme editor for a new theme copied from this key.
    NewTheme(String),
    /// Theme editor for this custom theme.
    EditTheme(String),
    /// Ask for admin permission to read CPU power.
    GrantCpuPower,
}

/// A piece of hardware the user can show or hide (a drive, a GPU).
pub struct Device {
    /// Stable across reboots; key in the config map.
    pub id: String,
    pub label: String,
    /// Shown when the user hasn't chosen yet.
    pub default_on: bool,
}

/// State of the "CPU power needs admin permission" flow, for display.
pub struct CpuPowerStatus<'a> {
    pub access: PowerAccess,
    pub waiting: bool,
    pub error: Option<&'a str>,
}

pub fn show(
    ui: &mut egui::Ui,
    config: &mut Config,
    themes: &Library,
    cpu_power: &CpuPowerStatus<'_>,
    drives: &[Device],
    gpus: &[Device],
) -> Option<Request> {
    let mut request = None;
    ui.add_space(8.0);
    ui.heading("Settings");
    ui.add_space(8.0);

    egui::ScrollArea::vertical().show(ui, |ui| {
        section(ui, "Appearance");
        request = theme_picker(ui, config, themes);

        section(ui, "Sampling");
        step_slider(
            ui,
            "Refresh every",
            &INTERVAL_STEPS_MS,
            &mut config.interval_ms,
            format::interval_ms,
        );
        step_slider(
            ui,
            "Graph history",
            &HISTORY_STEPS_SECS,
            &mut config.history_secs,
            format::span_secs,
        );
        ui.label(
            RichText::new(
                "What a normal-width graph shows. Wider tiles show further back, \
                 up to 30 min.",
            )
            .small()
            .weak(),
        );

        for category in Category::ALL {
            section(ui, category.label());
            if category == Category::Gpu {
                device_list(ui, "GPUs", gpus, &mut config.gpus);
                ui.add_space(4.0);
                ui.label(RichText::new("Show for each GPU:").small().weak());
            }
            if category == Category::Storage {
                device_list(ui, "Drives (mounted)", drives, &mut config.drives);
                ui.add_space(4.0);
                ui.label(RichText::new("Show for each drive:").small().weak());
            }
            for id in MetricId::ALL
                .into_iter()
                .filter(|m| m.category() == category)
            {
                if id == MetricId::CpuPower && cpu_power.access != PowerAccess::Readable {
                    if cpu_power_locked(ui, cpu_power) {
                        request = Some(Request::GrantCpuPower);
                    }
                    continue;
                }
                let mut on = config.is_enabled(id);
                if ui.checkbox(&mut on, id.label()).changed() {
                    config.set_enabled(id, on);
                }
            }
        }

        if let Some(path) = Config::path() {
            ui.add_space(16.0);
            ui.label(
                RichText::new(format!("Saved to {}", path.display()))
                    .small()
                    .weak(),
            );
        }
    });
    request
}

fn theme_picker(ui: &mut egui::Ui, config: &mut Config, themes: &Library) -> Option<Request> {
    let current = themes.get(&config.theme);
    let current_key = current.key.clone();
    ui.label("Theme");
    egui::ComboBox::from_id_salt("theme_picker")
        .selected_text(&current.spec.name)
        .height(420.0)
        .show_ui(ui, |ui| {
            let mut group =
                |ui: &mut egui::Ui, title: &str, entries: Vec<&crate::ui::theme::Entry>| {
                    if entries.is_empty() {
                        return;
                    }
                    ui.label(RichText::new(title).small().weak());
                    for entry in entries {
                        if ui
                            .selectable_label(entry.key == current_key, &entry.spec.name)
                            .clicked()
                        {
                            config.theme = entry.key.clone();
                        }
                    }
                    ui.separator();
                };
            group(
                ui,
                "Dark",
                themes.presets().filter(|e| !e.spec.light).collect(),
            );
            group(
                ui,
                "Light",
                themes.presets().filter(|e| e.spec.light).collect(),
            );
            group(ui, "Your themes", themes.custom().collect());
        });

    let mut request = None;
    ui.horizontal(|ui| {
        if ui
            .button("New from this…")
            .on_hover_text("Make your own theme starting from this one")
            .clicked()
        {
            request = Some(Request::NewTheme(current_key.clone()));
        }
        if current.is_custom() && ui.button("Edit…").clicked() {
            request = Some(Request::EditTheme(current_key.clone()));
        }
    });
    request
}

/// The CPU power toggle while it can't be used: greyed, and (if the machine has the
/// sensor) clickable to request permission. Returns whether it was clicked.
fn cpu_power_locked(ui: &mut egui::Ui, status: &CpuPowerStatus<'_>) -> bool {
    let mut off = false;
    if status.access == PowerAccess::Unavailable {
        ui.add_enabled(false, egui::Checkbox::new(&mut off, "Power draw"))
            .on_disabled_hover_text("This system has no CPU energy counter (RAPL).");
        return false;
    }
    if status.waiting {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new("Power draw — waiting for permission…").weak());
        });
        return false;
    }
    let clicked = ui
        .horizontal(|ui| {
            ui.add_enabled(false, egui::Checkbox::without_text(&mut off));
            ui.add(
                egui::Button::new(
                    RichText::new("Power draw — needs sudo, click to turn on").weak(),
                )
                .frame(false),
            )
            .on_hover_text(
                "Linux only lets root read the CPU energy counter, because very precise \
                 power readings can leak secrets (the PLATYPUS attack). This asks for your \
                 password once and lets only your user's group read it.",
            )
            .clicked()
        })
        .inner;
    if let Some(error) = status.error {
        ui.label(
            RichText::new(error)
                .small()
                .color(egui::Color32::from_rgb(0xe0, 0x5a, 0x5a)),
        );
    }
    clicked
}

/// Checkboxes for which devices get tiles; remembered by the device's stable id.
fn device_list(
    ui: &mut egui::Ui,
    title: &str,
    devices: &[Device],
    chosen: &mut std::collections::BTreeMap<String, bool>,
) {
    ui.label(RichText::new(title).small().weak());
    if devices.is_empty() {
        ui.label(RichText::new("None found").small().weak());
    }
    for device in devices {
        let mut on = chosen.get(&device.id).copied().unwrap_or(device.default_on);
        if ui.checkbox(&mut on, &device.label).changed() {
            chosen.insert(device.id.clone(), on);
        }
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).strong());
    ui.separator();
}

fn step_slider(
    ui: &mut egui::Ui,
    label: &str,
    steps: &[u64],
    value: &mut u64,
    fmt: fn(u64) -> String,
) {
    let mut idx = nearest_step(steps, *value);
    ui.label(label);
    ui.horizontal(|ui| {
        let slider = egui::Slider::new(&mut idx, 0..=steps.len() - 1).show_value(false);
        if ui.add(slider).changed() {
            *value = steps[idx];
        }
        ui.label(RichText::new(fmt(*value)).monospace().strong());
    });
}
