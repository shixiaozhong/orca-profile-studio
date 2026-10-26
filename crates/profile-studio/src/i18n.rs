//! Locale selection and read-only OrcaSlicer terminology. Locale changes never reload profile data.
use gpui_kit::App;
use profile_core::Translations;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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
pub static TEST_SYSTEM_LOCALE: Mutex<String> = Mutex::new(String::new());

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

/// The catalog of the OrcaSlicer checkout. Every repository load asks for it, so the parsed file
/// is kept until its path, size or modification time changes.
pub fn load_translations(profiles: &Path, override_dir: Option<PathBuf>) -> Result<Arc<Translations>, String> {
    let dir = override_dir.map(|dir| dir.join("zh_CN")).unwrap_or_else(|| {
        profiles.parent().and_then(Path::parent).unwrap_or(profiles).join("localization/i18n/zh_CN")
    });
    let mut paths = std::fs::read_dir(&dir)
        .map_err(|err| format!("{}: {err}", dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().ends_with("_zh_CN.po")))
        .collect::<Vec<_>>();
    paths.sort();
    if paths.len() != 1 {
        return Err(format!("{}: expected one *_zh_CN.po file, found {}", dir.display(), paths.len()));
    }
    let path = paths.swap_remove(0);
    let meta = std::fs::metadata(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    let stamp = (path.clone(), meta.len(), meta.modified().ok());
    static CACHE: Mutex<Option<(CatalogStamp, Arc<Translations>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|err| err.into_inner());
    if let Some((cached, translations)) = cache.as_ref()
        && *cached == stamp
    {
        return Ok(translations.clone());
    }
    let translations = Arc::new(Translations::load(&path).map_err(|err| format!("{}: {err}", path.display()))?);
    *cache = Some((stamp, translations.clone()));
    Ok(translations)
}

type CatalogStamp = (PathBuf, u64, Option<std::time::SystemTime>);

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

pub fn history_message(redo: bool, count: usize) -> String {
    match (redo, count == 1) {
        (false, true) => t!("history.undo.one", count = count),
        (false, false) => t!("history.undo.other", count = count),
        (true, true) => t!("history.redo.one", count = count),
        (true, false) => t!("history.redo.other", count = count),
    }
}

pub fn mode(mode: profile_core::OptionMode) -> String {
    match mode {
        profile_core::OptionMode::Simple => t!("mode.simple"),
        profile_core::OptionMode::Advanced => t!("mode.advanced"),
        profile_core::OptionMode::Develop => t!("mode.develop"),
    }
}

#[cfg(test)]
mod tests {
    use super::{LanguagePreference, load_translations, system_locale};
    use std::{collections::BTreeSet, fs, path::Path};

    #[test]
    fn resolves_system_languages_and_reports_missing_or_ambiguous_catalogs() {
        let _locale = super::test_locale();
        for locale in ["zh_CN", "zh-Hans-CN", "zh-TW", "ZH-hk"] {
            assert_eq!(system_locale(Some(locale)), "zh-CN");
        }
        assert_eq!(system_locale(Some("en-US")), "en");
        assert_eq!(system_locale(None), "en");
        assert_eq!(LanguagePreference::English.locale(), "en");
        let dir = tempfile::tempdir().unwrap();
        let profiles = dir.path().join("resources/profiles");
        let translations = dir.path().join("localization/i18n/zh_CN");
        fs::create_dir_all(&profiles).unwrap();
        assert!(load_translations(&profiles, None).is_err());
        fs::create_dir_all(&translations).unwrap();
        fs::write(translations.join("Example_zh_CN.po"), "msgid \"Wall loops\"\nmsgstr \"墙层数\"\n").unwrap();
        assert_eq!(load_translations(&profiles, None).unwrap().get("Wall loops"), Some("墙层数"));
        // Reloading the same unchanged file reuses the parsed catalog; a changed file is read again.
        let first = load_translations(&profiles, None).unwrap();
        assert!(std::sync::Arc::ptr_eq(&first, &load_translations(&profiles, None).unwrap()));
        fs::write(translations.join("Example_zh_CN.po"), "msgid \"Wall loops\"\nmsgstr \"外墙层数\"\n").unwrap();
        assert_eq!(load_translations(&profiles, None).unwrap().get("Wall loops"), Some("外墙层数"));
        fs::write(translations.join("Example_zh_CN.po"), "msgid \"Wall loops\"\nmsgstr \"墙层数\"\n").unwrap();
        assert_eq!(
            load_translations(Path::new("/unrelated/profiles"), Some(dir.path().join("localization/i18n")))
                .unwrap()
                .get("Wall loops"),
            Some("墙层数")
        );
        fs::write(translations.join("Other_zh_CN.po"), "").unwrap();
        assert!(load_translations(&profiles, None).is_err());
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
