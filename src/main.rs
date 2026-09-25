mod app;
mod config;
mod format;
mod frame_limit;
mod history;
mod hypr;
mod main_window;
mod metrics;
mod placement;
#[cfg(target_os = "linux")]
mod privileged;
mod sampler;
mod ui;

use eframe::egui;

fn main() -> eframe::Result {
    #[cfg(target_os = "linux")]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.first().map(String::as_str) == Some(privileged::FLAG) {
            std::process::exit(privileged::run(&args[1..]));
        }
    }
    let config = config::Config::load();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Hyperion")
            // Wayland app_id / X11 WM_CLASS: what Hyprland window rules match on.
            .with_app_id(main_window::APP_ID)
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([360.0, 240.0]),
        // With vsync, a frame blocks until the compositor says the screen is ready.
        // Compositors never say that to windows on hidden workspaces, so the event loop
        // froze and Hyprland offered to kill us. egui only redraws on new data or input,
        // so running without vsync costs nothing.
        glow_options: eframe::egui_glow::GlowConfiguration {
            vsync: false,
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        "Hyperion",
        options,
        Box::new(|cc| Ok(Box::new(app::HyperionApp::new(cc, config)))),
    )
}
