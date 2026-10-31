// Release builds on Windows start without a console window; debug builds keep it for stderr.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

rust_i18n::i18n!("locales", fallback = "en");

// GPUI elements accept owned strings; rust-i18n returns a Cow.
macro_rules! t { ($($args:tt)*) => { rust_i18n::t!($($args)*).into_owned() }; }
mod i18n;

mod app;
mod batch;
mod batch_plan;
mod catalog;
mod compare;
mod details;
mod edits;
mod feedback;
mod issue_text;
mod issues;
mod lifecycle;
mod manage;
mod models;
mod palette;
mod preset_picker;
mod references;
mod save;
mod settings;
mod tree;
mod value_editor;
mod values;

use std::path::{Path, PathBuf};

use gpui_kit::*;

fn main() {
    gpui_kit::application().with_assets(gpui_kit::assets::AllAssets).run(move |cx| {
        gpui_kit::init(cx);
        settings::init(settings::default_path(), cx);
        values::init(cx);
        app::init(cx);
        // Keys are bound before the menus are built, so the menus show the shortcuts.
        set_menus(cx);

        let settings = settings::get(cx);
        let profiles_dir = initial_profiles_dir(settings.open_last_dir, settings.profiles_dir.clone());
        app::open_window(profiles_dir, cx);
        // Quit with the last window, not the first.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}

/// The first argument, else (when the setting asks for it) the directory
/// loaded last or an OrcaSlicer checkout next to this repository (the path it was built from, so a
/// copied app bundle loses it); else none. Apps started from Finder have no terminal environment.
fn initial_profiles_dir(open_last: bool, last: Option<PathBuf>) -> Option<PathBuf> {
    std::env::args_os()
        .skip(1)
        // Older macOS versions pass a process serial number to apps launched from Finder.
        .find(|arg| !arg.to_string_lossy().starts_with("-psn_"))
        .map(PathBuf::from)
        .or_else(|| last.filter(|dir| open_last && dir.is_dir()))
        .or_else(|| {
            let sibling = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../OrcaSlicer/resources/profiles");
            (open_last && sibling.is_dir()).then_some(sibling)
        })
}

pub fn set_menus(cx: &mut App) {
    let mut file = vec![
        MenuItem::action(t!("menu.new_window"), app::NewWindow),
        MenuItem::action(t!("menu.open_folder"), app::OpenFolder),
        MenuItem::action(t!("menu.close_window"), lifecycle::CloseWindow),
    ];
    let mut menus = Vec::new();
    // macOS keeps Quit in the application menu; other platforms end the File menu with Exit.
    if cfg!(target_os = "macos") {
        menus.push(Menu {
            name: "Orca Profile Studio".into(),
            items: vec![MenuItem::action(t!("menu.quit"), lifecycle::Quit)],
            disabled: false,
        });
    } else {
        file.extend([MenuItem::separator(), MenuItem::action(t!("menu.exit"), lifecycle::Quit)]);
    }
    menus.push(Menu { name: t!("common.file").into(), items: file, disabled: false });
    cx.set_menus(menus);
}
