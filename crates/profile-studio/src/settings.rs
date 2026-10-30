//! Preferences that persist between runs: theme, last profiles directory, layout, window size.
//!
//! One [`SettingsStore`] global serves every window. Changes go through [`update`], which saves the
//! file right away; a missing or unreadable file means defaults. An unreadable file is copied aside
//! before the first save replaces it, or left alone (no saving) when that copy fails.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::i18n::LanguagePreference;
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
    pub fn label(self) -> String {
        match self {
            ThemePreference::System => t!("settings.system"),
            ThemePreference::Light => t!("settings.light"),
            ThemePreference::Dark => t!("settings.dark"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemePreference,
    pub language: LanguagePreference,
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
    /// A missing file is normal on first launch; other failures are reported by the caller.
    pub fn load(path: &Path) -> io::Result<Settings> {
        match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(err) => Err(err),
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
    settings_path(std::env::consts::OS, |name| std::env::var_os(name).filter(|value| !value.is_empty()))
}

/// macOS: `~/Library/Application Support`; Windows: `%APPDATA%`; others: `$XDG_CONFIG_HOME`, else
/// `~/.config`. `var` reads one environment variable; empty values count as unset.
fn settings_path(os: &str, var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(path) = var("ORCA_STUDIO_SETTINGS") {
        return Some(PathBuf::from(path));
    }
    let base = match os {
        "macos" => PathBuf::from(var("HOME")?).join("Library").join("Application Support"),
        "windows" => PathBuf::from(var("APPDATA")?),
        _ => var("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| Some(PathBuf::from(var("HOME")?).join(".config")))?,
    };
    Some(base.join("orca-profile-studio").join("settings.json"))
}

/// The settings of this run and where they are saved; `path` is `None` in tests.
pub struct SettingsStore {
    path: Option<PathBuf>,
    settings: Settings,
    load_error: Option<String>,
    save_failed: bool,
}

impl Global for SettingsStore {}

pub fn init(path: Option<PathBuf>, cx: &mut App) {
    let (path, settings, load_error) = open(path);
    cx.set_global(SettingsStore { path, settings, load_error, save_failed: false });
    apply_theme(None, cx);
    crate::i18n::apply(get(cx).language);
}

/// Loads `path`; returns where to save (`None` disables saving), the settings and a load error.
fn open(path: Option<PathBuf>) -> (Option<PathBuf>, Settings, Option<String>) {
    let Some(path) = path else { return (None, Settings::default(), None) };
    let err = match Settings::load(&path) {
        Ok(settings) => return (Some(path), settings, None),
        Err(err) => err,
    };
    let backup = path.with_extension("json.invalid");
    match fs::copy(&path, &backup) {
        Ok(_) => {
            let message = t!("settings.invalid_backup", arg0 = path.display(), err = err, arg1 = backup.display());
            (Some(path), Settings::default(), Some(message))
        }
        Err(_) => {
            let message = t!("settings.invalid_readonly", arg0 = path.display(), err = err);
            (None, Settings::default(), Some(message))
        }
    }
}

/// Called while creating the first view, so feedback is delivered after its Root exists.
pub fn report_load_error(cx: &mut App) {
    if let Some(message) = cx.global_mut::<SettingsStore>().load_error.take() {
        crate::feedback::error("settings-load", t!("settings.load_error"), message, cx);
    }
}

pub fn get(cx: &App) -> &Settings {
    &cx.global::<SettingsStore>().settings
}

/// Changes the settings and saves them if they changed.
pub fn update(cx: &mut App, change: impl FnOnce(&mut Settings)) {
    let store = cx.global_mut::<SettingsStore>();
    let before = store.settings.clone();
    change(&mut store.settings);
    if store.settings == before && !store.save_failed {
        return;
    }
    let Some(path) = &store.path else { return };
    match store.settings.save(path) {
        Ok(()) => {
            let recovered = store.save_failed;
            store.save_failed = false;
            if recovered {
                crate::feedback::clear("settings-save", cx);
            }
        }
        Err(err) => {
            let message = t!("settings.write_failed", arg0 = path.display(), err = err);
            store.save_failed = true;
            crate::feedback::error("settings-save", t!("settings.save_error"), message, cx);
        }
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
    use super::{Settings, ThemePreference, open, settings_path};
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn picks_the_settings_file_per_platform() {
        let env = |vars: &'static [(&str, &str)]| {
            move |name: &str| vars.iter().find(|(key, _)| *key == name).map(|(_, value)| OsString::from(value))
        };
        let path = |parts: &[&str]| parts.iter().collect::<PathBuf>();
        let home = env(&[("HOME", "/home/me"), ("APPDATA", "C:/Users/me/AppData/Roaming")]);
        assert_eq!(
            settings_path("macos", home),
            Some(path(&["/home/me", "Library", "Application Support", "orca-profile-studio", "settings.json"]))
        );
        assert_eq!(
            settings_path("windows", home),
            Some(path(&["C:/Users/me/AppData/Roaming", "orca-profile-studio", "settings.json"]))
        );
        assert_eq!(
            settings_path("linux", home),
            Some(path(&["/home/me", ".config", "orca-profile-studio", "settings.json"]))
        );
        let xdg = env(&[("HOME", "/home/me"), ("XDG_CONFIG_HOME", "/xdg")]);
        assert_eq!(settings_path("linux", xdg), Some(path(&["/xdg", "orca-profile-studio", "settings.json"])));
        // Windows does not fall back to HOME; the override wins everywhere.
        assert_eq!(settings_path("windows", env(&[("HOME", "/home/me")])), None);
        assert_eq!(settings_path("macos", env(&[])), None);
        let custom = env(&[("ORCA_STUDIO_SETTINGS", "/tmp/s.json"), ("HOME", "/home/me")]);
        assert_eq!(settings_path("windows", custom), Some(PathBuf::from("/tmp/s.json")));
    }

    #[test]
    fn round_trips_and_tolerates_bad_files() {
        let _locale = crate::i18n::test_locale();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/settings.json");
        assert_eq!(Settings::load(&path).unwrap(), Settings::default(), "a missing file gives defaults");

        let settings = Settings {
            theme: ThemePreference::Dark,
            language: crate::i18n::LanguagePreference::Chinese,
            profiles_dir: Some("/profiles".into()),
            sidebar_width: Some(300.),
            issues_open: true,
            issues_height: Some(200.),
            window_size: Some((1000., 700.)),
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), settings);

        std::fs::write(&path, "{ not json").unwrap();
        assert!(Settings::load(&path).is_err());
        // Unknown and missing fields are fine: files from other versions still load.
        std::fs::write(&path, r#"{ "theme": "light", "future": 1 }"#).unwrap();
        assert_eq!(Settings::load(&path).unwrap().theme, ThemePreference::Light);
    }

    #[test]
    fn keeps_an_unreadable_file_before_saving_over_it() {
        let _locale = crate::i18n::test_locale();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ not json").unwrap();
        let (save_to, settings, error) = open(Some(path.clone()));
        assert_eq!(save_to.as_ref(), Some(&path));
        assert_eq!(settings, Settings::default());
        assert!(error.unwrap().contains("settings.json.invalid"));
        assert_eq!(std::fs::read_to_string(dir.path().join("settings.json.invalid")).unwrap(), "{ not json");

        // Without a copy (here: the path is a directory), saving is disabled and the file stays.
        let (save_to, _, error) = open(Some(dir.path().to_path_buf()));
        assert_eq!(save_to, None);
        assert!(error.unwrap().contains("without saving"));

        assert_eq!(open(Some(dir.path().join("missing.json"))).2, None, "a missing file is not an error");
    }
}
