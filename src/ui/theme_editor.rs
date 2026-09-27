//! Window for creating and editing custom themes, with live preview.

use eframe::egui::{self, Color32, RichText};

use super::theme::{self, Library, ThemeColor, ThemeSpec};
use crate::hypr::Hypr;

/// App id (Wayland) / class (X11, Hyprland) of the editor's own OS window.
const APP_ID: &str = "hyperion-theme-editor";
const DEFAULT_SIZE: [f32; 2] = [380.0, 600.0];
const MIN_SIZE: [f32; 2] = [300.0, 260.0];

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
    /// `None` off Hyprland. Its own instance, not shared with the dashboard's: the
    /// IPC connection is just a socket path, cheap to open again.
    hypr: Option<Hypr>,
    /// Whether the float/no-anim window rule has been sent yet (Hyprland only).
    rule_registered: bool,
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
            hypr: Hypr::detect(),
            rule_registered: false,
        }
    }

    /// Edits an existing custom theme in place.
    pub fn edit(entry: &theme::Entry) -> Self {
        Self {
            draft: entry.spec.clone(),
            editing: entry.is_custom().then(|| entry.key.clone()),
            error: None,
            hypr: Hypr::detect(),
            rule_registered: false,
        }
    }

    pub fn draft(&self) -> &ThemeSpec {
        &self.draft
    }

    /// Shown as its own OS window (not an in-window `egui::Window`) so it gets a
    /// native title bar and resize border: movable and resizable anywhere on screen,
    /// and no longer confined to — and so clippable by — the main window's own
    /// canvas, which is what let the colour-picker's gradient square get cut off at
    /// the bottom when the main window sat low on the screen.
    pub fn show(&mut self, ctx: &egui::Context, library: &mut Library) -> Outcome {
        let title = if self.editing.is_some() {
            "Edit theme"
        } else {
            "New theme"
        };
        if let Some(h) = &self.hypr
            && !self.rule_registered
        {
            h.register_tile_rule(APP_ID, title, None, DEFAULT_SIZE);
            self.rule_registered = true;
        }
        let viewport = egui::ViewportId::from_hash_of("hyperion-theme-editor");
        let builder = egui::ViewportBuilder::default()
            .with_title(title)
            .with_app_id(APP_ID)
            .with_inner_size(DEFAULT_SIZE)
            .with_min_inner_size(MIN_SIZE)
            .with_resizable(true);
        let (outcome, close_requested) =
            ctx.show_viewport_immediate(viewport, builder, |ui, _class| {
                let mut outcome = Outcome::Editing;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    outcome = self.contents(ui, library);
                });
                let close_requested = ui.input(|i| i.viewport().close_requested());
                (outcome, close_requested)
            });
        if close_requested {
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
