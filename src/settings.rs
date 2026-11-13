//! User settings: `~/.config/veloedit/settings.json` (override the directory
//! with `$VELOEDIT_CONFIG_DIR`). Plain JSON; unknown keys are preserved.

use std::{fs, io, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::{session::atomic_write, ui::theme};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// "dark" | "light"
    pub theme: String,
    pub font_size: f32,
    /// Spaces inserted by the Tab key.
    pub tab_size: u8,
    /// Wrap long lines at the window edge.
    pub word_wrap: bool,
    /// Show the code overview on the right.
    pub minimap: bool,
    /// Show the menu bar (File, Edit, ...).
    pub menu_bar: bool,
    /// Markdown files open with the rendered preview next to the source.
    pub markdown_preview: bool,
    /// Saving removes the spaces and tabs at the end of every line.
    pub trim_trailing_whitespace: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { theme: "dark".into(), font_size: 14.0, tab_size: 4, word_wrap: false, minimap: true, menu_bar: true, markdown_preview: true, trim_trailing_whitespace: false }
    }
}

impl Settings {
    pub fn colors(&self) -> theme::C {
        theme::by_name(&self.theme).unwrap_or(&theme::DARK)
    }

    pub fn font_size(&self) -> f32 {
        self.font_size.clamp(8.0, 40.0)
    }

    pub fn tab_size(&self) -> usize {
        usize::from(self.tab_size.clamp(1, 16))
    }
}

pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("VELOEDIT_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("veloedit")
}

pub fn path() -> PathBuf {
    config_dir().join("settings.json")
}

/// Missing file = defaults. A file that is not valid JSON is an `Err` so the
/// caller can tell the user instead of silently ignoring their edits.
pub fn load() -> Result<Settings, String> {
    match fs::read_to_string(path()) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("settings.json: {e}")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(format!("settings.json: {e}")),
    }
}

/// Creates the file with defaults if needed; returns its path.
pub fn ensure_file() -> io::Result<PathBuf> {
    let path = path();
    if !path.exists() {
        fs::create_dir_all(config_dir())?;
        let json = serde_json::to_string_pretty(&Settings::default()).map_err(io::Error::other)?;
        atomic_write(&path, format!("{json}\n").as_bytes())?;
    }
    Ok(path)
}

/// Read-modify-write of one key, keeping every other key as the user left it.
pub fn set(key: &str, value: serde_json::Value) -> Result<(), String> {
    let path = ensure_file().map_err(|e| e.to_string())?;
    let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut doc: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("settings.json is not valid JSON ({e}); fix it first"))?;
    let obj = doc.as_object_mut().ok_or("settings.json must be a JSON object")?;
    obj.insert(key.to_owned(), value);
    let json = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    atomic_write(&path, format!("{json}\n").as_bytes()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_files_fall_back_per_key_and_values_are_clamped() {
        let s: Settings = serde_json::from_str(r#"{"theme":"light","font_size":500}"#).unwrap();
        assert_eq!(s.colors().name, "light");
        assert_eq!(s.font_size(), 40.0);
        assert_eq!(s.tab_size(), 4);
        assert!(s.minimap && s.menu_bar && !s.word_wrap, "missing keys fall back to the defaults");
        assert!(!s.trim_trailing_whitespace, "saving leaves files as they are unless asked");
        let s: Settings = serde_json::from_str(r#"{"trim_trailing_whitespace":true}"#).unwrap();
        assert!(s.trim_trailing_whitespace);
        let s: Settings = serde_json::from_str(r#"{"theme":"nope"}"#).unwrap();
        assert_eq!(s.colors().name, "dark", "unknown theme -> default");
    }
}
