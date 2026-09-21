#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod config;
mod media;
mod processor;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([960.0, 780.0])
            .with_min_inner_size([760.0, 620.0]),
        ..Default::default()
    };

    eframe::run_native(
        "動画コンタクトシート",
        options,
        Box::new(|cc| Ok(Box::new(app::ThumbnailApp::new(cc)))),
    )
}
