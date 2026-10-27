//! User settings, saved as JSON.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ai::Provider;
use crate::platform;

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
        Some(platform::app_data_dir()?.join("settings.json"))
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
