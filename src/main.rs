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
    // 32x32 RGBA icon – simple blue/white "SCP" icon encoded inline
    let size = 32usize;
    let mut rgba = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let idx = (y * size + x) * 4;
            // Blue background circle
            let dx = x as f32 - 15.5;
            let dy = y as f32 - 15.5;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist < 15.0 {
                rgba[idx] = 30;
                rgba[idx + 1] = 100;
                rgba[idx + 2] = 200;
                rgba[idx + 3] = 255;
            } else {
                rgba[idx + 3] = 0;
            }
        }
    }
    egui::IconData { rgba, width: size as u32, height: size as u32 }
}
