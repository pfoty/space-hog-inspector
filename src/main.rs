//! Space Hog Inspector: see where your disk space goes.

mod ai;
mod app;
mod scanner;
mod settings;
mod theme;
mod treemap;

use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui;
use muda::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};

fn main() -> eframe::Result {
    // An optional folder to scan straight away. macOS may pass a `-psn_…`
    // argument when launching an app bundle, so skip flags.
    let initial = std::env::args().skip(1).find(|a| !a.starts_with('-')).map(PathBuf::from);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Space Hog Inspector")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([760.0, 480.0])
            // An empty icon stops eframe replacing the bundle's AppIcon.icns
            // with its own default icon at launch.
            .with_icon(egui::IconData::default()),
        ..Default::default()
    };
    eframe::run_native(
        "Space Hog Inspector",
        options,
        Box::new(move |cc| {
            // The app is running by now, so this replaces winit's default
            // menu bar.
            let menu = build_menu_bar();
            let (tx, rx) = mpsc::channel();
            let repaint = cc.egui_ctx.clone();
            muda::MenuEvent::set_event_handler(Some(move |event| {
                let _ = tx.send(event);
                repaint.request_repaint();
            }));
            Ok(Box::new(app::SpaceHogApp::new(cc, initial, menu, rx)))
        }),
    )
}

fn build_menu_bar() -> Menu {
    let about = AboutMetadata {
        name: Some("Space Hog Inspector".into()),
        version: Some(env!("CARGO_PKG_VERSION").into()),
        ..Default::default()
    };
    let settings = MenuItem::with_id(app::MENU_SETTINGS, "Settings…", true, "CmdOrCtrl+Comma".parse().ok());
    let app_menu = Submenu::with_items(
        "Space Hog Inspector",
        true,
        &[
            &PredefinedMenuItem::about(None, Some(about)),
            &PredefinedMenuItem::separator(),
            &settings,
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::services(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::hide(None),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::quit(None),
        ],
    )
    .expect("app menu");
    let window_menu = Submenu::with_items(
        "Window",
        true,
        &[&PredefinedMenuItem::minimize(None), &PredefinedMenuItem::fullscreen(None)],
    )
    .expect("window menu");
    let menu = Menu::with_items(&[&app_menu, &window_menu]).expect("menu bar");
    menu.init_for_nsapp();
    window_menu.set_as_windows_menu_for_nsapp();
    menu
}
