//! Hiding the main window while popped-out tiles stay on screen, and bringing it back.
//!
//! How to hide a window differs per platform:
//! - Hyprland: move it to a private special workspace (it keeps running, unseen).
//! - Other Wayland desktops: apps can't hide their windows, only minimize them, and
//!   can't un-minimize; restoring asks the desktop to focus it, which may only flash
//!   it in the taskbar.
//! - X11, Windows, macOS: plain hide/show.

use eframe::egui::{self, ViewportCommand};

use crate::hypr::Hypr;

/// App id of the main window, set in `main.rs`.
pub const APP_ID: &str = "hyperion";

enum Backend {
    Hyprland(Hypr),
    Wayland,
    Native,
}

pub struct MainWindow {
    backend: Backend,
    hidden: bool,
    /// Focus has been lost since hiding; only focus *returning* after that means the
    /// user brought the window back themselves.
    lost_focus_since_hide: bool,
}

impl MainWindow {
    pub fn detect() -> Self {
        let backend = if let Some(h) = Hypr::detect() {
            Backend::Hyprland(h)
        } else if cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some() {
            Backend::Wayland
        } else {
            Backend::Native
        };
        Self {
            backend,
            hidden: false,
            lost_focus_since_hide: false,
        }
    }

    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    pub fn hide(&mut self, ctx: &egui::Context) {
        match &self.backend {
            Backend::Hyprland(h) => {
                if let Some(address) = h.own_window_address(APP_ID) {
                    h.hide_window(&address);
                }
            }
            Backend::Wayland => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
            Backend::Native => ctx.send_viewport_cmd(ViewportCommand::Visible(false)),
        }
        self.hidden = true;
        self.lost_focus_since_hide = false;
    }

    /// Notices the user bringing the window back themselves (taskbar, a compositor
    /// keybind), so pop-outs stop offering to restore it.
    pub fn observe_focus(&mut self, focused: Option<bool>) {
        match focused {
            Some(false) => self.lost_focus_since_hide = true,
            Some(true) if self.hidden && self.lost_focus_since_hide => self.hidden = false,
            _ => {}
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        match &self.backend {
            Backend::Hyprland(h) => {
                if let Some(address) = h.own_window_address(APP_ID) {
                    h.show_window(&address);
                }
            }
            Backend::Wayland => {
                ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
            Backend::Native => {
                ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
        }
        self.hidden = false;
    }
}
