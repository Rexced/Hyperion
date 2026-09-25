use eframe::egui::{self, Color32, CornerRadius, Mesh, Rect, Shadow, Stroke, Visuals};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ThemeId {
    #[default]
    Jade,
    Dracula,
    Nord,
    Gruvbox,
    Monokai,
    Solarized,
    Amber,
}

impl ThemeId {
    pub const ALL: [ThemeId; 7] = [
        ThemeId::Jade,
        ThemeId::Dracula,
        ThemeId::Nord,
        ThemeId::Gruvbox,
        ThemeId::Monokai,
        ThemeId::Solarized,
        ThemeId::Amber,
    ];

    /// Stable identifier used in the config file; never rename.
    pub fn key(self) -> &'static str {
        match self {
            ThemeId::Jade => "jade",
            ThemeId::Dracula => "dracula",
            ThemeId::Nord => "nord",
            ThemeId::Gruvbox => "gruvbox",
            ThemeId::Monokai => "monokai",
            ThemeId::Solarized => "solarized",
            ThemeId::Amber => "amber",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeId::Jade => "Jade",
            ThemeId::Dracula => "Dracula",
            ThemeId::Nord => "Nord",
            ThemeId::Gruvbox => "Gruvbox Dark",
            ThemeId::Monokai => "Monokai",
            ThemeId::Solarized => "Solarized Dark",
            ThemeId::Amber => "Terminal Amber",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.key() == key)
    }

    pub fn palette(self) -> Palette {
        // (bg_top, bg_bottom, text, text_dim, accent, accent2)
        let (top, bottom, text, dim, accent, accent2) = match self {
            ThemeId::Jade => (
                Color32::from_rgb(0x0f, 0x33, 0x2a),
                Color32::from_rgb(0x06, 0x17, 0x13),
                Color32::from_rgb(0xe4, 0xf2, 0xec),
                Color32::from_rgb(0x8f, 0xb5, 0xa8),
                Color32::from_rgb(0xff, 0x8c, 0x32),
                Color32::from_rgb(0x5f, 0xe3, 0xb8),
            ),
            ThemeId::Dracula => (
                Color32::from_rgb(0x2c, 0x2e, 0x3f),
                Color32::from_rgb(0x1a, 0x1b, 0x26),
                Color32::from_rgb(0xf8, 0xf8, 0xf2),
                Color32::from_rgb(0x9a, 0x9c, 0xb8),
                Color32::from_rgb(0xbd, 0x93, 0xf9),
                Color32::from_rgb(0xff, 0x79, 0xc6),
            ),
            ThemeId::Nord => (
                Color32::from_rgb(0x3b, 0x42, 0x52),
                Color32::from_rgb(0x2e, 0x34, 0x40),
                Color32::from_rgb(0xec, 0xef, 0xf4),
                Color32::from_rgb(0x9d, 0xab, 0xbd),
                Color32::from_rgb(0x88, 0xc0, 0xd0),
                Color32::from_rgb(0xa3, 0xbe, 0x8c),
            ),
            ThemeId::Gruvbox => (
                Color32::from_rgb(0x32, 0x30, 0x2f),
                Color32::from_rgb(0x1d, 0x20, 0x21),
                Color32::from_rgb(0xeb, 0xdb, 0xb2),
                Color32::from_rgb(0xa8, 0x99, 0x84),
                Color32::from_rgb(0xfe, 0x80, 0x19),
                Color32::from_rgb(0xb8, 0xbb, 0x26),
            ),
            ThemeId::Monokai => (
                Color32::from_rgb(0x2f, 0x30, 0x2a),
                Color32::from_rgb(0x1e, 0x1f, 0x1c),
                Color32::from_rgb(0xf8, 0xf8, 0xf2),
                Color32::from_rgb(0xa1, 0xa1, 0x9a),
                Color32::from_rgb(0xf9, 0x26, 0x72),
                Color32::from_rgb(0xa6, 0xe2, 0x2e),
            ),
            ThemeId::Solarized => (
                Color32::from_rgb(0x07, 0x36, 0x42),
                Color32::from_rgb(0x00, 0x2b, 0x36),
                Color32::from_rgb(0x93, 0xa1, 0xa1),
                Color32::from_rgb(0x58, 0x6e, 0x75),
                Color32::from_rgb(0xb5, 0x89, 0x00),
                Color32::from_rgb(0x26, 0x8b, 0xd2),
            ),
            ThemeId::Amber => (
                Color32::from_rgb(0x14, 0x12, 0x0a),
                Color32::from_rgb(0x00, 0x00, 0x00),
                Color32::from_rgb(0xff, 0xd9, 0x9a),
                Color32::from_rgb(0xa8, 0x7d, 0x45),
                Color32::from_rgb(0xff, 0xb0, 0x00),
                Color32::from_rgb(0xff, 0x6a, 0x00),
            ),
        };
        Palette::derive(top, bottom, text, dim, accent, accent2)
    }
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg_top: Color32,
    pub bg_bottom: Color32,
    pub card: Color32,
    pub card_stroke: Color32,
    pub well: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    pub accent2: Color32,
}

impl Palette {
    /// Builds the full palette from a theme's six hand-picked colors; card/well/stroke
    /// tones are derived so every theme keeps consistent contrast without restating them.
    fn derive(
        bg_top: Color32,
        bg_bottom: Color32,
        text: Color32,
        text_dim: Color32,
        accent: Color32,
        accent2: Color32,
    ) -> Self {
        Self {
            bg_top,
            bg_bottom,
            card: with_alpha(lighten(bg_bottom, 16), 0xd8),
            card_stroke: lighten(bg_bottom, 32),
            well: darken(bg_bottom, 6),
            text,
            text_dim,
            accent,
            accent2,
        }
    }
}

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
    let mut v = Visuals::dark();
    v.override_text_color = Some(p.text);
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = lighten(p.bg_bottom, 8);
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
        (&mut w.inactive, lighten(p.bg_bottom, 20)),
        (&mut w.hovered, lighten(p.bg_bottom, 33)),
        (&mut w.active, lighten(p.bg_bottom, 44)),
        (&mut w.open, lighten(p.bg_bottom, 33)),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.corner_radius = CornerRadius::same(6);
        state.fg_stroke = Stroke::new(1.5, p.text);
    }
    w.hovered.bg_stroke = Stroke::new(1.0, p.accent2);
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    ctx.set_visuals_of(egui::Theme::Dark, v);
    ctx.set_theme(egui::ThemePreference::Dark);
}

/// Vertical background gradient, painted first so cards sit on top of it.
pub fn paint_background(ui: &egui::Ui, p: &Palette) {
    let rect: Rect = ui.max_rect();
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

    #[test]
    fn every_theme_key_roundtrips() {
        for t in ThemeId::ALL {
            assert_eq!(ThemeId::from_key(t.key()), Some(t));
        }
    }

    #[test]
    fn keys_are_unique() {
        let mut keys: Vec<_> = ThemeId::ALL.iter().map(|t| t.key()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), ThemeId::ALL.len());
    }

    #[test]
    fn unknown_key_is_none() {
        assert_eq!(ThemeId::from_key("not-a-theme"), None);
    }
}
