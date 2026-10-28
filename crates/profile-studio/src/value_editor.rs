//! Value input shared by single and batch editing, shaped by the value itself.
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use profile_core::RawValue;

/// How a value is edited.
#[derive(Debug, Clone, PartialEq)]
enum EditorKind {
    /// Text; lists are edited one item per line.
    Text,
    /// G-code and other multi-line strings, edited as raw text.
    Multiline,
}

/// Builds a value in the shape of `template`: a string stays a string, an array stays an array.
pub fn shaped(template: &RawValue, items: Vec<String>) -> RawValue {
    match (template, items.as_slice()) {
        (RawValue::Str(_), [single]) => RawValue::Str(single.clone()),
        _ => RawValue::List(items),
    }
}

/// The text an editor starts with: list items one per line.
fn editor_text(value: &RawValue) -> String {
    match value {
        RawValue::Str(text) => text.clone(),
        RawValue::List(items) => items.join("\n"),
    }
}

enum Control {
    Line(Entity<InputState>),
    Text(Entity<TextareaState>),
    Unsupported,
}

pub enum ValueEditorEvent {
    Changed,
    Apply(RawValue),
}
impl EventEmitter<ValueEditorEvent> for ValueEditor {}

pub struct ValueEditor {
    key: String,
    kind: EditorKind,
    source: RawValue,
    control: Control,
    immediate: bool,
    _subscriptions: Vec<Subscription>,
}

impl ValueEditor {
    pub fn new(key: String, source: RawValue, immediate: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        let kind = EditorKind::for_value(&source);
        let control = match &kind {
            EditorKind::Multiline if matches!(&source, RawValue::List(items) if items.len() > 1) => {
                Control::Unsupported
            }
            EditorKind::Text if matches!(&source, RawValue::Str(_)) => {
                let state = cx.new(|cx| InputState::new(window, cx).default_value(editor_text(&source)));
                subscriptions.push(cx.subscribe(&state, |this, _, event: &InputEvent, cx| match event {
                    InputEvent::Change => {
                        cx.emit(ValueEditorEvent::Changed);
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } if this.immediate => this.apply(cx),
                    _ => {}
                }));
                Control::Line(state)
            }
            _ => {
                let state = cx.new(|cx| TextareaState::new(window, cx).default_value(editor_text(&source)));
                subscriptions.push(cx.subscribe(&state, |_, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.emit(ValueEditorEvent::Changed);
                        cx.notify();
                    }
                }));
                Control::Text(state)
            }
        };
        Self { key, kind, source, control, immediate, _subscriptions: subscriptions }
    }
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn source(&self) -> &RawValue {
        &self.source
    }
    pub fn value(&self, cx: &App) -> Option<RawValue> {
        let items = match &self.control {
            Control::Line(state) => vec![state.read(cx).value().to_string()],
            Control::Text(state) => {
                let value = state.read(cx).value();
                if value.as_ref() == editor_text(&self.source) {
                    // Joining list items is ambiguous for empty or multiline items. Merely
                    // opening an editor must never rewrite the original array.
                    return Some(self.source.clone());
                }
                if self.kind == EditorKind::Multiline {
                    vec![value.to_string()]
                } else if value.is_empty() {
                    Vec::new()
                } else {
                    value.split('\n').map(str::to_owned).collect()
                }
            }
            Control::Unsupported => return None,
        };
        Some(shaped(&self.source, items))
    }
    pub fn has_unapplied_input(&self, cx: &App) -> bool {
        self.value(cx).is_some_and(|v| v != self.source)
    }
    fn apply(&mut self, cx: &mut Context<Self>) {
        if let Some(value) = self.value(cx) {
            cx.emit(ValueEditorEvent::Apply(value));
        }
    }
}

impl Render for ValueEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let label = t!("editor.value", arg0 = self.key);
        match &self.control {
            Control::Line(state) => h_flex()
                .gap_2()
                .child(Input::new(state).id("value-input").aria_label(label).small().w_80().min_w_0())
                .when(self.immediate, |line| {
                    line.child(
                        Button::new("apply")
                            .small()
                            .label(t!("common.apply"))
                            .on_click(cx.listener(|this, _, _, cx| this.apply(cx))),
                    )
                })
                .into_any_element(),
            Control::Text(state) => v_flex()
                .gap_1()
                .child(Textarea::new(state).aria_label(label).h(rems(7.)))
                .child(
                    h_flex()
                        .gap_2()
                        .when(self.immediate, |line| {
                            line.child(
                                Button::new("apply")
                                    .small()
                                    .label(t!("common.apply"))
                                    .on_click(cx.listener(|this, _, _, cx| this.apply(cx))),
                            )
                        })
                        .when(self.kind != EditorKind::Multiline, |line| {
                            line.child(
                                div().text_xs().text_color(cx.theme().muted_foreground).child(t!("editor.items")),
                            )
                        }),
                )
                .into_any_element(),
            Control::Unsupported => {
                div().text_color(cx.theme().muted_foreground).child(t!("editor.unsupported")).into_any_element()
            }
        }
    }
}

impl EditorKind {
    /// G-code and other values with line breaks are edited as raw text; everything else as text
    /// with list items one per line.
    fn for_value(value: &RawValue) -> Self {
        let multiline = match value {
            RawValue::Str(text) => text.contains('\n'),
            RawValue::List(items) => items.iter().any(|item| item.contains('\n')),
        };
        if multiline { Self::Multiline } else { Self::Text }
    }
}

#[cfg(test)]
mod tests {
    use super::EditorKind;
    use profile_core::RawValue;

    #[test]
    fn editors_follow_the_shape_of_the_value() {
        assert_eq!(EditorKind::for_value(&RawValue::Str("0.2".into())), EditorKind::Text);
        assert_eq!(EditorKind::for_value(&RawValue::List(vec!["A".into(), "B".into()])), EditorKind::Text);
        assert_eq!(EditorKind::for_value(&RawValue::Str("G28\nG1 Z5".into())), EditorKind::Multiline);
        assert_eq!(EditorKind::for_value(&RawValue::List(vec!["M104\nM109".into()])), EditorKind::Multiline);
    }
}
