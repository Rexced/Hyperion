use eframe::egui::{self, RichText};

use crate::config::{Config, HISTORY_STEPS_SECS, INTERVAL_STEPS_MS, nearest_step};
use crate::format;
use crate::metrics::{Category, MetricId};
use crate::ui::theme::ThemeId;

pub fn show(ui: &mut egui::Ui, config: &mut Config) {
    ui.add_space(8.0);
    ui.heading("Settings");
    ui.add_space(8.0);

    egui::ScrollArea::vertical().show(ui, |ui| {
        section(ui, "Appearance");
        theme_picker(ui, config);

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

        for category in Category::ALL {
            section(ui, category.label());
            for id in MetricId::ALL
                .into_iter()
                .filter(|m| m.category() == category)
            {
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
}

fn theme_picker(ui: &mut egui::Ui, config: &mut Config) {
    let current = config.theme_id();
    ui.label("Theme");
    egui::ComboBox::from_id_salt("theme_picker")
        .selected_text(current.label())
        .show_ui(ui, |ui| {
            for theme in ThemeId::ALL {
                if ui
                    .selectable_label(theme == current, theme.label())
                    .clicked()
                {
                    config.theme = theme.key().to_owned();
                }
            }
        });
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
