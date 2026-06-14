//! Persisted user preferences (currently just the chosen audio devices). Stored as
//! JSON in the Tauri app-config dir. The bridge owns reads/writes; the audio engine
//! never touches disk.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub input_device: Option<String>,
    #[serde(default)]
    pub output_device: Option<String>,
}

impl Settings {
    /// Path to the settings file within `config_dir`.
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join("settings.json")
    }

    /// Load settings, falling back to defaults on a missing or unreadable/corrupt file.
    pub fn load(config_dir: &Path) -> Settings {
        match std::fs::read_to_string(Self::path(config_dir)) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Settings::default(),
        }
    }

    /// Write settings as pretty JSON, creating `config_dir` if needed.
    pub fn save(&self, config_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(config_dir)?;
        let json = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into());
        std::fs::write(Self::path(config_dir), json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        // Unique per test name; avoids needing randomness/time.
        let dir = std::env::temp_dir().join(format!("ramsit-settings-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tmp("roundtrip");
        let s = Settings {
            input_device: Some("Mic A".into()),
            output_device: None,
        };
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_file_is_default() {
        let dir = tmp("missing");
        assert_eq!(Settings::load(&dir), Settings::default());
    }

    #[test]
    fn load_corrupt_json_is_default() {
        let dir = tmp("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(Settings::path(&dir), b"not json{{").unwrap();
        assert_eq!(Settings::load(&dir), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
