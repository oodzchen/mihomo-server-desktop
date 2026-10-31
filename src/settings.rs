//! The client's own saved settings. The interface language and theme keep a
//! copy of the instance's shared preference, or a choice made on the status
//! page while no instance runs (it becomes the instance's once one does).
use crate::i18n::Language;
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

/// Shared presentation preference; system mode delegates to GTK/WebKit.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    pub fn preference(value: &Value) -> Option<Self> {
        serde_json::from_value(value.get("theme")?.clone()).ok()
    }
}

/// `$XDG_CONFIG_HOME`, else `~/.config`.
pub fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
}

/// Where the settings live; `None` keeps them in memory only (tests).
pub struct Store(Option<PathBuf>);

impl Store {
    pub fn user() -> Self {
        Self(config_home().map(|config| config.join("mihomo-server-desktop").join("settings.json")))
    }

    #[cfg(test)]
    pub fn memory() -> Self {
        Self(None)
    }

    #[cfg(test)]
    pub fn at(path: PathBuf) -> Self {
        Self(Some(path))
    }

    /// The saved language; a missing or unreadable file is none.
    pub fn language(&self) -> Option<Language> {
        let text = std::fs::read_to_string(self.0.as_ref()?).ok()?;
        let settings: Value = serde_json::from_str(&text).ok()?;
        Language::from_code(settings.get("language")?.as_str()?)
    }

    pub fn theme(&self) -> Theme {
        let Some(path) = &self.0 else { return Theme::System };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| Theme::preference(&value))
            .unwrap_or_default()
    }

    /// Replace both preferences atomically (`None` saves no language).
    pub fn save(&self, language: Option<Language>, theme: Theme) -> Result<()> {
        let Some(path) = &self.0 else { return Ok(()) };
        let directory = path.parent().context("settings directory")?;
        std::fs::create_dir_all(directory)?;
        let temporary = path.with_extension("json.new");
        let text = serde_json::to_string_pretty(&json!({"language": language.map(Language::code), "theme": theme}))?;
        std::fs::write(&temporary, text + "\n").with_context(|| format!("cannot write {}", temporary.display()))?;
        std::fs::rename(&temporary, path).with_context(|| format!("cannot replace {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_round_trip_and_bad_files_are_ignored() {
        let directory = std::env::temp_dir().join(format!("mihomo-desktop-settings-{}", std::process::id()));
        let path = directory.join("nested").join("settings.json");
        let store = Store::at(path.clone());
        assert_eq!(store.language(), None);
        store.save(Some(Language::Zhtw), Theme::Dark).unwrap();
        assert_eq!(store.language(), Some(Language::Zhtw));
        assert_eq!(store.theme(), Theme::Dark);
        store.save(None, Theme::System).unwrap();
        assert_eq!(store.language(), None);
        assert_eq!(store.theme(), Theme::System);
        std::fs::write(&path, r#"{"language":"en"}"#).unwrap();
        assert_eq!(store.theme(), Theme::System);
        assert_eq!(Theme::preference(&json!({"theme":"unknown"})), None);
        std::fs::write(&path, "{broken").unwrap();
        assert_eq!(store.language(), None);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
