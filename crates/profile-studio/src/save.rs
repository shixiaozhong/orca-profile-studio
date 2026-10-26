//! The shared save preview and commit for every applied edit in this window.
use crate::details::format_value;
use crate::edits::{Edits, EditsEvent};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;

pub fn confirm(
    edits: &Entity<Edits>,
    window: &mut Window,
    cx: &mut App,
    on_saved: impl Fn((bool, String), &mut App) + 'static,
) {
    if edits.read(cx).dirty().is_empty() || window.has_active_dialog(cx) {
        return;
    }
    let edits = edits.clone();
    let on_saved = std::rc::Rc::new(on_saved);
    window.open_alert_dialog(cx, move |dialog, _, cx| {
        let mut panel = v_flex().gap_1();
        let state = edits.read(cx);
        let catalog = state.catalog();
        let count = state.dirty().len();
        for preset in state.dirty() {
            let info = &catalog.repo.presets()[preset];
            panel = panel.child(div().pt_1().text_color(cx.theme().muted_foreground).child(format!(
                "{} — {}",
                info.name(),
                info.path.display()
            )));
            for change in state.session(preset).unwrap().pending() {
                let show =
                    |value: &Option<profile_core::RawValue>| value.as_ref().map_or("(inherited)".into(), format_value);
                panel = panel.child(div().pl_3().child(format!(
                    "{}: {} → {}",
                    change.key,
                    show(&change.before),
                    show(&change.after)
                )));
            }
        }
        let edits = edits.clone();
        let on_saved = on_saved.clone();
        dialog
            .title("Save changes to disk?")
            .width(px(640.))
            .confirm()
            .description("Only applied edits are included. Existing file formatting is preserved.")
            .child(
                div()
                    .id("save-preview")
                    .test_support()
                    .aria_label("Changes to save")
                    .max_h(rems(24.))
                    .overflow_y_scroll()
                    .text_sm()
                    .child(panel),
            )
            .ok_text(format!("Save {count} file(s)"))
            .on_ok(move |_, _, cx| {
                let catalog = edits.read(cx).catalog().clone();
                let (paths, error) = edits.update(cx, |edits, cx| {
                    let result = edits.save_all();
                    if !result.0.is_empty() {
                        cx.emit(EditsEvent::Saved);
                    }
                    cx.notify();
                    result
                });
                let status = match error {
                    None => (true, format!("Saved {} file(s).", paths.len())),
                    Some((preset, err)) => (
                        false,
                        format!(
                            "Saved {} file(s). Could not save {}: {err}",
                            paths.len(),
                            catalog.repo.presets()[preset].name()
                        ),
                    ),
                };
                on_saved(status, cx);
                true
            })
    });
}
