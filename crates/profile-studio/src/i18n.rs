//! Locale selection for the app's own text. Locale changes never reload profile data.
use gpui_kit::App;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguagePreference {
    #[default]
    System,
    English,
    Chinese,
}
impl LanguagePreference {
    pub fn label(self) -> String {
        match self {
            Self::System => t!("language.system"),
            Self::English => t!("language.english"),
            Self::Chinese => t!("language.chinese"),
        }
    }
    pub fn locale(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Chinese => "zh-CN",
            Self::System => system_locale(detected_locale().as_deref()),
        }
    }
}
/// The operating system's locale. Tests pin it, so the System preference runs the same startup
/// path without depending on the language of the machine running them.
fn detected_locale() -> Option<String> {
    #[cfg(test)]
    return Some(TEST_SYSTEM_LOCALE.lock().unwrap_or_else(|err| err.into_inner()).clone());
    #[cfg(not(test))]
    sys_locale::get_locale()
}

#[cfg(test)]
pub static TEST_SYSTEM_LOCALE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

fn system_locale(locale: Option<&str>) -> &'static str {
    if locale.is_some_and(|s| s.split(['-', '_']).next().is_some_and(|s| s.eq_ignore_ascii_case("zh"))) {
        "zh-CN"
    } else {
        "en"
    }
}
pub fn is_chinese() -> bool {
    &*rust_i18n::locale() == "zh-CN"
}
pub fn apply(language: LanguagePreference) {
    gpui_kit::component::set_locale(language.locale());
}
pub fn set_language(language: LanguagePreference, cx: &mut App) {
    crate::settings::update(cx, |settings| settings.language = language);
    apply(language);
    crate::set_menus(cx);
    // Defer until the action's window update has ended; then update every retained view in place.
    cx.defer(|cx| {
        for handle in cx.windows() {
            handle
                .update(cx, |_, window, cx| {
                    let view =
                        gpui_kit::base::Root::read(window, cx).view().clone().downcast::<crate::app::StudioApp>();
                    if let Ok(view) = view {
                        view.update(cx, |view, cx| view.refresh_language(window, cx));
                    }
                })
                .ok();
        }
        cx.refresh_windows();
    });
}

/// The locale is process-wide; tests that switch it exclude every other test.
#[cfg(test)]
static TEST_LOCALE: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// For tests that read English UI text. They run in parallel with each other.
#[cfg(test)]
pub fn test_locale() -> impl Drop {
    TEST_LOCALE.read().unwrap_or_else(|err| err.into_inner())
}

/// For tests that switch the language. Runs alone, starts in English with an English system
/// locale and restores both when it ends.
#[cfg(test)]
pub fn test_locale_switching() -> impl Drop {
    struct Guard {
        _guard: std::sync::RwLockWriteGuard<'static, ()>,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            TEST_SYSTEM_LOCALE.lock().unwrap_or_else(|err| err.into_inner()).clear();
            gpui_kit::component::set_locale("en");
        }
    }
    let guard = TEST_LOCALE.write().unwrap_or_else(|err| err.into_inner());
    TEST_SYSTEM_LOCALE.lock().unwrap_or_else(|err| err.into_inner()).clear();
    gpui_kit::component::set_locale("en");
    Guard { _guard: guard }
}

pub fn severity(severity: profile_core::Severity) -> String {
    match severity {
        profile_core::Severity::Info => t!("severity.info"),
        profile_core::Severity::Warning => t!("severity.warning"),
        profile_core::Severity::Error => t!("severity.error"),
        profile_core::Severity::Fatal => t!("severity.fatal"),
    }
}
pub fn kind(kind: profile_core::PresetKind) -> String {
    match kind {
        profile_core::PresetKind::Machine => t!("kind.machine"),
        profile_core::PresetKind::Filament => t!("kind.filament"),
        profile_core::PresetKind::Process => t!("kind.process"),
    }
}

/// A key binding as this platform writes it, e.g. `secondary-k` is ⌘K on macOS and Ctrl+K elsewhere.
pub fn shortcut(binding: &str) -> String {
    gpui_kit::component::kbd::Kbd::format(&gpui_kit::Keystroke::parse(binding).expect("valid key binding"))
}

pub fn history_message(redo: bool, count: usize) -> String {
    match (redo, count == 1) {
        (false, true) => t!("history.undo.one", count = count),
        (false, false) => t!("history.undo.other", count = count),
        (true, true) => t!("history.redo.one", count = count),
        (true, false) => t!("history.redo.other", count = count),
    }
}

#[cfg(test)]
mod tests {
    use super::{LanguagePreference, system_locale};
    use std::{collections::BTreeSet, fs, path::Path};

    #[test]
    fn resolves_system_languages() {
        let _locale = super::test_locale();
        for locale in ["zh_CN", "zh-Hans-CN", "zh-TW", "ZH-hk"] {
            assert_eq!(system_locale(Some(locale)), "zh-CN");
        }
        assert_eq!(system_locale(Some("en-US")), "en");
        assert_eq!(system_locale(None), "en");
        assert_eq!(LanguagePreference::English.locale(), "en");
    }

    #[test]
    fn formats_shortcuts_for_this_platform() {
        let expected = if cfg!(target_os = "macos") { ["⌘K", "⇧⌘Z"] } else { ["Ctrl+K", "Ctrl+Shift+Z"] };
        assert_eq!([super::shortcut("secondary-k"), super::shortcut("secondary-shift-z")], expected);
    }

    #[test]
    fn messages_have_complete_locales_matching_placeholders_and_live_keys() {
        let _locale = super::test_locale();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let yaml: serde_json::Value =
            serde_saphyr::from_str(&fs::read_to_string(root.join("locales/app.yml")).unwrap()).unwrap();
        let calls = regex::Regex::new(r#"\bt!\s*\(\s*"([^"]+)""#).unwrap();
        let placeholders = regex::Regex::new(r"%\{([^}]+)\}").unwrap();
        let mut used = BTreeSet::new();
        fn collect(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    collect(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        collect(&root.join("src"), &mut files);
        for file in files {
            let source = fs::read_to_string(&file).unwrap();
            let source =
                source.lines().filter(|line| !line.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
            used.extend(calls.captures_iter(&source).map(|c| c[1].to_owned()));
        }
        let messages = yaml.as_object().unwrap();
        let defined = messages.keys().filter(|k| k.as_str() != "_version").cloned().collect::<BTreeSet<_>>();
        assert_eq!(used.difference(&defined).collect::<Vec<_>>(), Vec::<&String>::new(), "undefined translation keys");
        assert_eq!(defined.difference(&used).collect::<Vec<_>>(), Vec::<&String>::new(), "unused translation keys");
        for key in used {
            let en = messages[&key]["en"].as_str().unwrap_or_else(|| panic!("{key} missing English"));
            let zh = messages[&key]["zh-CN"].as_str().unwrap_or_else(|| panic!("{key} missing Chinese"));
            assert!(!en.is_empty() && !zh.is_empty(), "{key}");
            let params = |s: &str| placeholders.captures_iter(s).map(|c| c[1].to_owned()).collect::<BTreeSet<_>>();
            assert_eq!(params(en), params(zh), "{key}");
        }
    }
}
