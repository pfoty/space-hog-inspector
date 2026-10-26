//! User settings (saved as JSON) and macOS Full Disk Access helpers.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::ai::Provider;

pub const FULL_DISK_ACCESS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Nesting levels drawn in the treemap.
    pub max_depth: usize,
    /// Descend into other mounted volumes.
    pub cross_volumes: bool,
    /// Without Full Disk Access, leave out folders that would make macOS
    /// ask for permission one folder at a time.
    pub skip_protected: bool,
    /// Installed AI agent asked whether items are safe to remove; `None`
    /// keeps AI off, so nothing is sent anywhere.
    pub ai_provider: Option<Provider>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { max_depth: 6, cross_volumes: false, skip_protected: true, ai_provider: None }
    }
}

impl Settings {
    /// Where settings are saved.
    pub fn file() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join("Library/Application Support/Space Hog Inspector/settings.json"))
    }

    /// Loads saved settings, falling back to defaults if none are saved yet.
    pub fn load() -> Settings {
        Self::file()
            .and_then(|f| fs::read_to_string(f).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let file = Self::file().ok_or("HOME is not set")?;
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(file, json).map_err(|e| e.to_string())
    }
}

/// Whether this app currently has Full Disk Access.
///
/// macOS has no API to ask directly, so this tries to read a file that only
/// apps with Full Disk Access may open. Reading it never shows a prompt.
pub fn has_full_disk_access() -> bool {
    let Some(home) = std::env::var_os("HOME") else { return false };
    let home = PathBuf::from(home);
    let probes = [
        home.join("Library/Application Support/com.apple.TCC/TCC.db"),
        home.join("Library/Safari/Bookmarks.plist"),
        PathBuf::from("/Library/Application Support/com.apple.TCC/TCC.db"),
    ];
    probes.iter().any(|p| fs::File::open(p).is_ok())
}

/// The `.app` bundle this program runs from, if it runs from one.
pub fn app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // .../Name.app/Contents/MacOS/binary
    let bundle = exe.parent()?.parent()?.parent()?;
    (bundle.extension()? == "app").then(|| bundle.to_path_buf())
}

pub fn open_full_disk_access_settings() -> std::io::Result<()> {
    Command::new("open").arg(FULL_DISK_ACCESS_URL).status().map(|_| ())
}

/// Shows the app bundle (or the binary, when run with `cargo run`) in Finder
/// so it can be dragged into the Full Disk Access list.
pub fn reveal_app_in_finder() -> std::io::Result<()> {
    let target = app_bundle().or_else(|| std::env::current_exe().ok()).unwrap_or_default();
    Command::new("open").arg("-R").arg(target).status().map(|_| ())
}

/// Starts a fresh copy of the app. The caller should then close this one;
/// macOS only applies a new Full Disk Access grant to a new process.
pub fn launch_new_instance() -> std::io::Result<()> {
    match app_bundle() {
        Some(bundle) => Command::new("open").arg("-n").arg(bundle).spawn().map(|_| ()),
        None => Command::new(std::env::current_exe()?).spawn().map(|_| ()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_use_defaults() {
        let s: Settings = serde_json::from_str(r#"{"max_depth": 9}"#).unwrap();
        assert_eq!(s.max_depth, 9);
        assert!(s.skip_protected);
        assert!(!s.cross_volumes);
        assert_eq!(s.ai_provider, None);
    }

    #[test]
    fn round_trips_through_json() {
        let s = Settings { max_depth: 3, cross_volumes: true, skip_protected: false, ai_provider: Some(Provider::Codex) };
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s, back);
    }
}
