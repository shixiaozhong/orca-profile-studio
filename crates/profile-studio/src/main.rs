mod app;
mod catalog;
mod compare;
mod details;
mod edits;
mod issues;
mod palette;
mod tree;
mod values;

use std::path::{Path, PathBuf};

use gpui_kit::*;

actions!(profile_studio, [Quit]);

fn main() {
    let profiles_dir = initial_profiles_dir();
    gpui_kit::application().with_assets(gpui_kit::assets::AllAssets).run(move |cx| {
        gpui_kit::init(cx);
        values::init(cx);
        app::init(cx);
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus([Menu {
            name: "Orca Profile Studio".into(),
            items: vec![MenuItem::action("Quit Orca Profile Studio", Quit)],
            disabled: false,
        }]);

        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("Orca Profile Studio".into()), ..Default::default() }),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| app::StudioApp::new(profiles_dir, window, cx)))
            .expect("failed to open window");
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.activate(true);
    });
}

/// The first argument, else `ORCA_PROFILES_DIR`, else an OrcaSlicer checkout next to this repository.
fn initial_profiles_dir() -> Option<PathBuf> {
    std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("ORCA_PROFILES_DIR").map(PathBuf::from))
        .or_else(|| {
            let sibling = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../OrcaSlicer/resources/profiles");
            sibling.is_dir().then_some(sibling)
        })
}
