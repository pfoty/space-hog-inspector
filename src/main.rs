//! Space Hog Inspector: see where your disk space goes.

// A window app on Windows, so no console window opens alongside it.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod ai;
mod app;
mod platform;
mod scanner;
mod settings;
mod theme;
mod treemap;

use std::path::PathBuf;

use eframe::egui;

fn main() -> eframe::Result {
    // An optional folder to scan straight away. macOS may pass a `-psn_…`
    // argument when launching an app bundle, so skip flags.
    let initial = std::env::args().skip(1).find(|a| !a.starts_with('-')).map(PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Space Hog Inspector")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([760.0, 480.0])
            .with_icon(platform::window_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "Space Hog Inspector",
        options,
        Box::new(move |cc| {
            // The app is running by now, so this replaces winit's default
            // menu bar.
            let (menu, menu_events) = platform::install_menu_bar(app::MENU_SETTINGS, cc.egui_ctx.clone());
            Ok(Box::new(app::SpaceHogApp::new(cc, initial, menu, menu_events)))
        }),
    )
}
