//! Persistent user configuration.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const DEFAULT_HOTKEY: &str = "CmdOrCtrl+Shift+Space";

/// Stored as JSON in the platform configuration directory.
///
/// Every field has a `serde` default so that a config file written by an older
/// version of the app keeps working after an upgrade.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// Layout ids that participate in the hotkey cycle, in cycle order.
    pub layouts: Vec<String>,
    /// Tauri accelerator string, e.g. `CmdOrCtrl+Shift+Space`.
    pub hotkey: String,
    /// Whether a left click on the tray icon switches the layout directly.
    pub switch_on_click: bool,
    /// Whether the app is registered to start with the session.
    pub autostart: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            layouts: Vec::new(),
            hotkey: DEFAULT_HOTKEY.to_string(),
            switch_on_click: true,
            autostart: false,
        }
    }
}

impl Config {
    /// Fills in the cycle with every layout the OS offers when it is empty,
    /// and drops ids that the OS no longer knows about.
    pub fn reconciled(&self, available: &[super::layouts::LayoutInfo]) -> Self {
        let known: Vec<&str> = available.iter().map(|l| l.id.as_str()).collect();
        let mut layouts: Vec<String> = self
            .layouts
            .iter()
            .filter(|id| known.contains(&id.as_str()))
            .cloned()
            .collect();

        if layouts.is_empty() {
            layouts = available.iter().map(|l| l.id.clone()).collect();
        }

        Self {
            layouts,
            ..self.clone()
        }
    }
}

fn config_path(dir: &Path) -> PathBuf {
    dir.join("config.json")
}

/// Reads the config, falling back to defaults when the file is missing,
/// unreadable or corrupt. A broken config never prevents startup.
pub fn load(dir: &Path) -> Config {
    let path = config_path(dir);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Config::default();
    };

    match serde_json::from_str::<Config>(&raw) {
        Ok(config) => config,
        Err(err) => {
            eprintln!(
                "klatz: ignoring unreadable config {}: {err}",
                path.display()
            );
            Config::default()
        }
    }
}

/// Writes the config atomically: a crash mid-write must not lose the settings.
pub fn save(dir: &Path, config: &Config) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|err| format!("Не удалось создать каталог {}: {err}", dir.display()))?;

    let path = config_path(dir);
    let json = serde_json::to_string_pretty(config)
        .map_err(|err| format!("Не удалось сериализовать настройки: {err}"))?;

    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)
        .map_err(|err| format!("Не удалось записать {}: {err}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|err| format!("Не удалось сохранить {}: {err}", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layouts::LayoutInfo;

    fn layout(id: &str) -> LayoutInfo {
        LayoutInfo {
            id: id.to_string(),
            name: id.to_uppercase(),
        }
    }

    #[test]
    fn empty_cycle_is_filled_from_available() {
        let config = Config::default();
        let merged = config.reconciled(&[layout("en"), layout("ru")]);
        assert_eq!(merged.layouts, vec!["en", "ru"]);
    }

    #[test]
    fn stale_ids_are_dropped_and_order_is_kept() {
        let config = Config {
            layouts: vec!["ru".into(), "zz".into(), "en".into()],
            ..Default::default()
        };
        let merged = config.reconciled(&[layout("en"), layout("ru")]);
        assert_eq!(merged.layouts, vec!["ru", "en"]);
    }

    #[test]
    fn defaults_are_used_for_missing_fields() {
        let parsed: Config = serde_json::from_str(r#"{"layouts":["en"]}"#).unwrap();
        assert_eq!(parsed.layouts, vec!["en"]);
        assert_eq!(parsed.hotkey, DEFAULT_HOTKEY);
        assert!(parsed.switch_on_click);
        assert!(!parsed.autostart);
    }

    #[test]
    fn corrupt_config_falls_back_to_defaults() {
        let dir = std::env::temp_dir().join("klatz-klatz-test-config");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(config_path(&dir), "{ not json").unwrap();

        assert_eq!(load(&dir).hotkey, DEFAULT_HOTKEY);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
