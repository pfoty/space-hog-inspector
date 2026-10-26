//! Space Hog Inspector: see where your disk space goes.

mod app;
mod scanner;
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
            .with_min_inner_size([760.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Space Hog Inspector",
        options,
        Box::new(move |cc| Ok(Box::new(app::SpaceHogApp::new(cc, initial)))),
    )
}
