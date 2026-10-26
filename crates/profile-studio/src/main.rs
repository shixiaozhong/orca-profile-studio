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
mod palette;
mod preset_picker;
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

        let profiles_dir = initial_profiles_dir(settings::get(cx).profiles_dir.clone());
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

/// The first argument, else `ORCA_PROFILES_DIR`, else the directory loaded last, else an
/// OrcaSlicer checkout next to this repository.
fn initial_profiles_dir(last: Option<PathBuf>) -> Option<PathBuf> {
    std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("ORCA_PROFILES_DIR").map(PathBuf::from))
        .or_else(|| last.filter(|dir| dir.is_dir()))
        .or_else(|| {
            let sibling = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../OrcaSlicer/resources/profiles");
            sibling.is_dir().then_some(sibling)
        })
}

pub fn set_menus(cx: &mut App) {
    cx.set_menus([
        Menu {
            name: "Orca Profile Studio".into(),
            items: vec![MenuItem::action(t!("menu.quit"), lifecycle::Quit)],
            disabled: false,
        },
        Menu {
            name: t!("common.file").into(),
            items: vec![
                MenuItem::action(t!("menu.new_window"), app::NewWindow),
                MenuItem::action(t!("menu.close_window"), lifecycle::CloseWindow),
            ],
            disabled: false,
        },
    ]);
}
