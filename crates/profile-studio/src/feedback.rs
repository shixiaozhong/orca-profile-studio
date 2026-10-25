//! Error feedback shared by operations that may run outside a window update.

use gpui_kit::component::{WindowExt as _, button::Button, notification::Notification};
use gpui_kit::{App, SystemNotification};

struct OperationError;

/// Defer until the caller releases its window; use a stable ID to replace repeated failures.
pub fn error(id: &'static str, title: &'static str, message: String, cx: &mut App) {
    cx.defer(move |cx| {
        let mut delivered = false;
        for handle in cx.windows() {
            if handle
                .update(cx, |_, window, cx| {
                    window.push_notification(
                        Notification::error(message.clone())
                            .id1::<OperationError>(id)
                            .title(title)
                            .autohide(false)
                            .action(|_, _, cx| {
                                Button::new("dismiss-error")
                                    .label("Dismiss")
                                    .on_click(cx.listener(|note, _, window, cx| note.dismiss(window, cx)))
                            }),
                        cx,
                    );
                })
                .is_ok()
            {
                delivered = true;
            }
        }
        // The first window can fail too; there is then no Root in which to show an alert.
        if !delivered {
            eprintln!("{title}: {message}");
            cx.show_system_notification(SystemNotification {
                tag: id.into(),
                title: title.into(),
                body: message.into(),
                actions: Vec::new(),
            });
        }
    });
}

pub fn clear(id: &'static str, cx: &mut App) {
    cx.defer(move |cx| {
        for handle in cx.windows() {
            handle.update(cx, |_, window, cx| window.remove_notification1::<OperationError>(id, cx)).ok();
        }
        cx.dismiss_system_notification(id);
    });
}
