//! Window for creating and editing custom themes, with live preview.

use eframe::egui::{self, Color32, RichText};

use super::theme::{self, Library, ThemeColor, ThemeSpec};

pub enum Outcome {
    /// Still open; the app previews `draft()`.
    Editing,
    /// Saved under this theme key.
    Saved(String),
    Cancelled,
    Deleted,
}

pub struct ThemeEditor {
    draft: ThemeSpec,
    /// Key of the custom theme being edited in place; `None` makes a new theme.
    editing: Option<String>,
    error: Option<String>,
}

impl ThemeEditor {
    /// Starts a new theme as a copy of `from`.
    pub fn new_from(from: &theme::Entry) -> Self {
        let mut draft = from.spec.clone();
        draft.name = format!("{} (custom)", from.spec.name);
        Self {
            draft,
            editing: None,
            error: None,
        }
    }

    /// Edits an existing custom theme in place.
    pub fn edit(entry: &theme::Entry) -> Self {
        Self {
            draft: entry.spec.clone(),
            editing: entry.is_custom().then(|| entry.key.clone()),
            error: None,
        }
    }

    pub fn draft(&self) -> &ThemeSpec {
        &self.draft
    }

    pub fn show(&mut self, ctx: &egui::Context, library: &mut Library) -> Outcome {
        let mut outcome = Outcome::Editing;
        let mut open = true;
        let title = if self.editing.is_some() {
            "Edit theme"
        } else {
            "New theme"
        };
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(340.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                outcome = self.contents(ui, library);
            });
        if !open {
            return Outcome::Cancelled;
        }
        outcome
    }

    fn contents(&mut self, ui: &mut egui::Ui, library: &mut Library) -> Outcome {
        ui.horizontal(|ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut self.draft.name);
        });
        ui.checkbox(&mut self.draft.light, "Light theme")
            .on_hover_text("Derive darker borders and buttons for a light background");
        ui.add_space(8.0);

        egui::Grid::new("theme_main_colors")
            .num_columns(2)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                let d = &mut self.draft;
                for (label, color) in [
                    ("Background (top)", &mut d.bg_top),
                    ("Background (bottom)", &mut d.bg_bottom),
                    ("Text", &mut d.text),
                    ("Dim text", &mut d.text_dim),
                    ("Accent", &mut d.accent),
                    ("Second accent", &mut d.accent2),
                ] {
                    ui.label(label);
                    color_button(ui, color);
                    ui.end_row();
                }
            });

        ui.add_space(6.0);
        egui::CollapsingHeader::new("Advanced")
            .id_salt("theme_advanced")
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Normally derived from the colours above. Tick to override.")
                        .small()
                        .weak(),
                );
                // What each colour currently is, so switching an override on starts there.
                let mut resolved = theme::resolved_advanced(&self.draft.palette());
                let current = resolved.fields_mut().map(|(_, c)| c.expect("all resolved"));
                egui::Grid::new("theme_advanced_colors")
                    .num_columns(2)
                    .spacing([16.0, 4.0])
                    .show(ui, |ui| {
                        for ((label, field), now) in
                            self.draft.advanced.fields_mut().into_iter().zip(current)
                        {
                            let mut on = field.is_some();
                            if ui.checkbox(&mut on, label).changed() {
                                *field = on.then_some(now);
                            }
                            match field {
                                Some(color) => color_button(ui, color),
                                None => {
                                    let mut shown = now;
                                    ui.add_enabled_ui(false, |ui| color_button(ui, &mut shown));
                                }
                            }
                            ui.end_row();
                        }
                    });
            });

        if let Some(dir) = library.dir() {
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!(
                    "Saved as a .toml file in {} — share it, or drop others' theme files there.",
                    dir.display()
                ))
                .small()
                .weak(),
            );
        }
        if let Some(error) = &self.error {
            ui.colored_label(Color32::from_rgb(0xe0, 0x5a, 0x5a), error);
        }

        ui.add_space(8.0);
        let mut outcome = Outcome::Editing;
        ui.horizontal(|ui| {
            let name_ok = !self.draft.name.trim().is_empty();
            if ui.add_enabled(name_ok, egui::Button::new("Save")).clicked() {
                self.draft.name = self.draft.name.trim().to_owned();
                match library.save(&self.draft, self.editing.as_deref()) {
                    Ok(key) => outcome = Outcome::Saved(key),
                    Err(e) => self.error = Some(format!("Couldn't save: {e}")),
                }
            }
            if ui.button("Cancel").clicked() {
                outcome = Outcome::Cancelled;
            }
            if let Some(key) = &self.editing
                && ui.button("Delete theme").clicked()
            {
                match library.delete(key) {
                    Ok(()) => outcome = Outcome::Deleted,
                    Err(e) => self.error = Some(format!("Couldn't delete: {e}")),
                }
            }
        });
        outcome
    }
}

fn color_button(ui: &mut egui::Ui, color: &mut ThemeColor) {
    let [r, g, b, _] = color.0.to_array();
    let mut rgb = [r, g, b];
    ui.horizontal(|ui| {
        if ui.color_edit_button_srgb(&mut rgb).changed() {
            color.0 = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
        }
        ui.label(RichText::new(color.to_string()).monospace().small());
    });
}
