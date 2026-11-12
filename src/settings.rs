//! Persistent user settings.
//!
//! Stored as JSON with the same property names as the original Windows app
//! (`config.cfg` in `TruckSim/ScsFileBrowser` under the platform's local data
//! directory), so an existing Windows configuration keeps working.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::util::extension;

/// Maximum number of entries in the "recent" list.
pub const MAX_RECENT: usize = 10;

/// An external program files can be opened with.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Program {
    pub name: String,
    pub path: String,
}

/// A file extension (including the dot) and the program that opens it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ExtensionSetting {
    pub name: String,
    pub default_program: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Settings {
    pub last_export_path: String,
    pub last_selected_file: String,
    pub last_selected_folder: String,
    pub recent_paths: Vec<String>,
    pub programs: Vec<Program>,
    pub extensions: Vec<ExtensionSetting>,
}

impl Settings {
    /// Move `path` to the front of the recent list, trimming it to [`MAX_RECENT`].
    pub fn add_recent(&mut self, path: &str) {
        self.recent_paths.retain(|p| p != path);
        self.recent_paths.insert(0, path.to_string());
        self.recent_paths.truncate(MAX_RECENT);
    }

    /// The program configured as the default for `archive_path`'s extension.
    pub fn default_program_for(&self, archive_path: &str) -> Option<&Program> {
        let ext = extension(archive_path);
        let setting = self.extensions.iter().find(|e| e.name == ext)?;
        if setting.default_program.trim().is_empty() {
            return None;
        }
        self.programs
            .iter()
            .find(|p| p.name == setting.default_program)
    }
}

/// Folder holding the config and log files.
pub fn app_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("TruckSim")
        .join("ScsFileBrowser")
}

/// Loads and saves [`Settings`] at a fixed path.
pub struct SettingsStore {
    path: PathBuf,
    pub settings: Settings,
}

impl SettingsStore {
    /// Open the store at the platform default location and load it.
    pub fn open_default() -> Self {
        Self::open(app_data_dir().join("config.cfg"))
    }

    /// Open the store at `path`. A missing or unreadable file yields defaults.
    pub fn open(path: PathBuf) -> Self {
        let mut store = Self {
            path,
            settings: Settings::default(),
        };
        store.load();
        store
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Reload from disk, discarding unsaved in-memory changes.
    pub fn load(&mut self) {
        self.settings = match fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                log::error!("Could not parse settings '{}': {e}", self.path.display());
                Settings::default()
            }),
            Err(_) => Settings::default(),
        };
    }

    pub fn save(&self) {
        if let Some(dir) = self.path.parent() {
            if let Err(e) = fs::create_dir_all(dir) {
                log::error!("Could not create settings folder: {e}");
                return;
            }
        }
        match serde_json::to_string_pretty(&self.settings) {
            Ok(json) => {
                // Write to a temp file first so a crash can't leave a half-written config.
                let tmp = self.path.with_extension("cfg.tmp");
                if let Err(e) = fs::write(&tmp, json).and_then(|_| fs::rename(&tmp, &self.path)) {
                    log::error!("Could not save settings: {e}");
                }
            }
            Err(e) => log::error!("Could not serialise settings: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_is_mru_and_capped() {
        let mut s = Settings::default();
        for i in 0..12 {
            s.add_recent(&format!("p{i}"));
        }
        assert_eq!(s.recent_paths.len(), MAX_RECENT);
        assert_eq!(s.recent_paths[0], "p11");
        s.add_recent("p5");
        assert_eq!(s.recent_paths[0], "p5");
        assert_eq!(s.recent_paths.iter().filter(|p| *p == "p5").count(), 1);
    }

    #[test]
    fn reads_original_app_format() {
        // Format written by the C# app (Newtonsoft.Json, PascalCase).
        let json = r#"{
            "LastExportPath": "D:\\out",
            "LastSelectedFile": "",
            "LastSelectedFolder": "D:\\games",
            "RecentPaths": ["D:\\games"],
            "Programs": [{"Name": "Notepad++", "Path": "C:\\np.exe"}],
            "Extensions": [{"DefaultProgram": "Notepad++", "Name": ".sii"}]
        }"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(s.programs[0].name, "Notepad++");
        assert_eq!(
            s.default_program_for("def/a.sii").unwrap().path,
            "C:\\np.exe"
        );
        assert!(s.default_program_for("def/a.txt").is_none());
        // Round-trips with the same keys.
        let back = serde_json::to_string(&s).unwrap();
        assert!(back.contains("\"DefaultProgram\""));
    }

    #[test]
    fn missing_fields_default() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SettingsStore::open(dir.path().join("sub").join("config.cfg"));
        store.settings.add_recent("x");
        store.save();
        let again = SettingsStore::open(store.path().to_path_buf());
        assert_eq!(again.settings.recent_paths, vec!["x".to_string()]);
    }
}
