mod app;
mod config;
mod format;
mod history;
mod hypr;
mod metrics;
mod sampler;
mod ui;

use eframe::egui;

fn main() -> eframe::Result {
    let config = config::Config::load();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Hyperion")
            // Wayland app_id / X11 WM_CLASS: what Hyprland window rules match on.
            .with_app_id("hyperion")
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([360.0, 240.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Hyperion",
        options,
        Box::new(|cc| Ok(Box::new(app::HyperionApp::new(cc, config)))),
    )
}
