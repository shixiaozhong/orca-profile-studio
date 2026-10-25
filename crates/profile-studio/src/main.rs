mod app;
mod catalog;
mod compare;
mod details;
mod edits;
mod feedback;
mod issues;
mod lifecycle;
mod palette;
mod settings;
mod tree;
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
        cx.set_menus([
            Menu {
                name: "Orca Profile Studio".into(),
                items: vec![MenuItem::action("Quit Orca Profile Studio", lifecycle::Quit)],
                disabled: false,
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("New Window…", app::NewWindow),
                    MenuItem::action("Close Window", lifecycle::CloseWindow),
                ],
                disabled: false,
            },
        ]);

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
