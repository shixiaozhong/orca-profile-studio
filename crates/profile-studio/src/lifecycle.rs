//! Window-scoped close and application-wide quit commands.

use std::path::PathBuf;

use gpui_kit::StatefulInteractiveElement as _;
use gpui_kit::base::Root;
use gpui_kit::component::{WindowExt as _, button::ButtonVariant};
use gpui_kit::{
    AnyWindowHandle, App, InteractiveElement as _, KeyBinding, ParentElement as _, Styled as _, actions, div, px, rems,
};

use crate::app::StudioApp;

actions!(lifecycle, [Quit, CloseWindow]);

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("cmd-q", Quit, None), KeyBinding::new("cmd-w", CloseWindow, None)]);
    cx.on_action(|_: &Quit, cx| cx.defer(|cx| request_quit(cx, None)));
}

type QuitScope = Vec<(AnyWindowHandle, Vec<PathBuf>)>;

fn scope(cx: &mut App) -> QuitScope {
    cx.windows()
        .into_iter()
        .filter_map(|handle| {
            let files = handle
                .update(cx, |_, window, cx| {
                    let view = Root::read(window, cx).view().clone().downcast::<StudioApp>().ok()?;
                    Some(view.read(cx).unsaved_files(cx))
                })
                .ok()??;
            Some((handle, files))
        })
        .collect()
}

fn request_quit(cx: &mut App, approved: Option<QuitScope>) {
    let scope = scope(cx);
    if scope.iter().all(|(_, files)| files.is_empty()) || approved.as_ref() == Some(&scope) {
        for (handle, _) in scope {
            handle
                .update(cx, |_, window, cx| {
                    crate::app::remember_window_size(window, cx);
                    window.remove_window();
                })
                .ok();
        }
        cx.quit();
        return;
    }
    let Some(target) = cx.active_window().or_else(|| scope.first().map(|(handle, _)| *handle)) else { return };
    target
        .update(cx, |_, window, cx| {
            if window.has_active_dialog(cx) {
                return;
            }
            window.open_alert_dialog(cx, move |dialog, _, _| {
                let approved = scope.clone();
                let files =
                    scope.iter().flat_map(|(_, files)| files).map(|path| div().child(path.display().to_string()));
                dialog
                    .title("Discard edits in all windows and quit?")
                    .width(px(640.))
                    .confirm()
                    .description("These files have unsaved edits or unapplied input. Cancel to review and save them.")
                    .child(div().id("quit-files").max_h(rems(24.)).overflow_y_scroll().children(files))
                    .ok_text("Discard edits and quit")
                    .ok_variant(ButtonVariant::Danger)
                    .on_ok(move |_, _, cx| {
                        // Other windows remain usable: recheck before discarding edits not shown here.
                        let approved = approved.clone();
                        cx.defer(move |cx| request_quit(cx, Some(approved)));
                        true
                    })
            });
        })
        .ok();
}
