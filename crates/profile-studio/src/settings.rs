//! Preferences that persist between runs: theme, last profiles directory, layout, window size.
//!
//! One [`SettingsStore`] global serves every window. Changes go through [`update`], which saves the
//! file right away; a missing or unreadable file means defaults.

use std::path::{Path, PathBuf};
use std::{fs, io};

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    /// Follow the system appearance, also when it changes.
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    pub fn label(self) -> &'static str {
        match self {
            ThemePreference::System => "System",
            ThemePreference::Light => "Light",
            ThemePreference::Dark => "Dark",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemePreference,
    /// The directory loaded last.
    pub profiles_dir: Option<PathBuf>,
    /// Width of the navigation tree, in pixels.
    pub sidebar_width: Option<f32>,
    pub issues_open: bool,
    /// Height of the issues panel, in pixels.
    pub issues_height: Option<f32>,
    /// Width and height of the window, in pixels.
    pub window_size: Option<(f32, f32)>,
}

impl Settings {
    /// Reads `path`; a missing or invalid file gives the defaults.
    pub fn load(path: &Path) -> Settings {
        match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
                eprintln!("ignoring {}: {err}", path.display());
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    /// Writes `path` through a temporary file, so a failure never leaves half a file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let temporary = path.with_extension("json.saving");
        fs::write(&temporary, serde_json::to_string_pretty(self).expect("settings serialize"))?;
        fs::rename(&temporary, path)
    }
}

/// `ORCA_STUDIO_SETTINGS`, else the platform's configuration directory.
pub fn default_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ORCA_STUDIO_SETTINGS") {
        return Some(PathBuf::from(path));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let base = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from)
    };
    Some(base.join("orca-profile-studio/settings.json"))
}

/// The settings of this run and where they are saved; `path` is `None` in tests.
pub struct SettingsStore {
    path: Option<PathBuf>,
    settings: Settings,
}

impl Global for SettingsStore {}

pub fn init(path: Option<PathBuf>, cx: &mut App) {
    let settings = path.as_deref().map(Settings::load).unwrap_or_default();
    cx.set_global(SettingsStore { path, settings });
    apply_theme(None, cx);
}

pub fn get(cx: &App) -> &Settings {
    &cx.global::<SettingsStore>().settings
}

/// Changes the settings and saves them if they changed.
pub fn update(cx: &mut App, change: impl FnOnce(&mut Settings)) {
    let store = cx.global_mut::<SettingsStore>();
    let before = store.settings.clone();
    change(&mut store.settings);
    if store.settings == before {
        return;
    }
    if let Some(path) = &store.path
        && let Err(err) = store.settings.save(path)
    {
        eprintln!("cannot save settings to {}: {err}", path.display());
    }
}

/// Applies the theme preference; `System` reads the window's (or the app's) appearance.
pub fn apply_theme(window: Option<&mut Window>, cx: &mut App) {
    match get(cx).theme {
        ThemePreference::System => Theme::sync_system_appearance(window, cx),
        ThemePreference::Light => Theme::change(ThemeMode::Light, window, cx),
        ThemePreference::Dark => Theme::change(ThemeMode::Dark, window, cx),
    }
}

pub fn set_theme(theme: ThemePreference, window: &mut Window, cx: &mut App) {
    update(cx, |settings| settings.theme = theme);
    apply_theme(Some(window), cx);
}

#[cfg(test)]
mod tests {
    use super::{Settings, ThemePreference};

    #[test]
    fn round_trips_and_tolerates_bad_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/settings.json");
        assert_eq!(Settings::load(&path), Settings::default(), "a missing file gives defaults");

        let settings = Settings {
            theme: ThemePreference::Dark,
            profiles_dir: Some("/profiles".into()),
            sidebar_width: Some(300.),
            issues_open: true,
            issues_height: Some(200.),
            window_size: Some((1000., 700.)),
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);

        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
        // Unknown and missing fields are fine: files from other versions still load.
        std::fs::write(&path, r#"{ "theme": "light", "future": 1 }"#).unwrap();
        assert_eq!(Settings::load(&path).theme, ThemePreference::Light);
    }
}
