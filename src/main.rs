mod app;
mod models;
mod protocols;
mod ui;
mod worker;

use eframe::NativeOptions;
use egui::ViewportBuilder;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let icon = load_icon();

    let options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("TuxSCP")
            // Must match the .desktop basename / StartupWMClass so GNOME
            // (Wayland) can associate the window with its desktop entry icon
            .with_app_id("tuxscp")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 600.0])
            .with_drag_and_drop(true)
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "TuxSCP",
        options,
        Box::new(|cc| Ok(Box::new(app::LinuxScpApp::new(cc)))),
    )
}

fn load_icon() -> egui::IconData {
    let bytes = include_bytes!("../packaging/icons/tuxscp_256.png");
    let image = image::load_from_memory(bytes)
        .expect("embedded icon is valid PNG")
        .into_rgba8();
    let (width, height) = image.dimensions();
    egui::IconData { rgba: image.into_raw(), width, height }
}
