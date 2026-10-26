//! Schema-shaped value input shared by single and batch editing.
use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme as _, IndexPath, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use profile_core::RawValue;
/// An enum value offered in the editor: what is stored, and what the GUI calls it.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumChoice {
    value: String,
    label: SharedString,
}

impl gpui_kit::component::select::SelectItem for EnumChoice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &String {
        &self.value
    }
}

/// How a value is edited.
#[derive(Debug, Clone, PartialEq)]
pub enum EditorKind {
    Bool,
    Enum(Vec<EnumChoice>),
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

impl EnumChoice {
    pub fn new(value: String, label: String) -> Self {
        Self { value, label: label.into() }
    }
}

enum Control {
    Bool(bool),
    Enum(Entity<SelectState<SearchableVec<EnumChoice>>>),
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
    pub fn new(
        key: String,
        kind: EditorKind,
        source: RawValue,
        immediate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions = Vec::new();
        let control = match &kind {
            EditorKind::Bool => Control::Bool(source.as_scalar() == Some("1")),
            EditorKind::Enum(choices) => {
                let current = source.as_scalar().and_then(|v| choices.iter().position(|c| c.value == v));
                let state = cx.new(|cx| {
                    SelectState::new(SearchableVec::new(choices.clone()), current.map(IndexPath::new), window, cx)
                });
                subscriptions.push(cx.subscribe(
                    &state,
                    |this, _, event: &SelectEvent<SearchableVec<EnumChoice>>, cx| {
                        if let SelectEvent::Confirm(Some(_)) = event {
                            this.changed(cx);
                        }
                    },
                ));
                Control::Enum(state)
            }
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
    pub fn refresh_language(&mut self, catalog: &crate::catalog::Catalog, window: &mut Window, cx: &mut Context<Self>) {
        let kind = EditorKind::for_option(catalog.repo.schema().and_then(|s| s.option(&self.key))).translated(catalog);
        if let (Control::Enum(state), EditorKind::Enum(choices)) = (&self.control, &kind) {
            let current = state.read(cx).selected_value().cloned();
            state.update(cx, |state, cx| {
                state.set_items(SearchableVec::new(choices.clone()), window, cx);
                if let Some(current) = current {
                    state.set_selected_value(&current, window, cx);
                }
            });
            self.kind = kind;
        }
        cx.notify();
    }
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn source(&self) -> &RawValue {
        &self.source
    }
    pub fn value(&self, cx: &App) -> Option<RawValue> {
        let items = match &self.control {
            Control::Bool(value) => vec![if *value { "1" } else { "0" }.into()],
            Control::Enum(state) => vec![state.read(cx).selected_value()?.clone()],
            Control::Line(state) => vec![state.read(cx).value().to_string()],
            Control::Text(state) => {
                let value = state.read(cx).value();
                if self.kind == EditorKind::Multiline {
                    vec![value.to_string()]
                } else {
                    value.lines().map(str::to_owned).collect()
                }
            }
            Control::Unsupported => return None,
        };
        Some(shaped(&self.source, items))
    }
    pub fn has_unapplied_input(&self, cx: &App) -> bool {
        self.value(cx).is_some_and(|v| v != self.source)
    }
    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(ValueEditorEvent::Changed);
        if self.immediate {
            self.apply(cx);
        }
        cx.notify();
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
            Control::Bool(checked) => Checkbox::new("value-bool")
                .label(t!("editor.enabled"))
                .checked(*checked)
                .on_click(cx.listener(|this, value: &bool, _, cx| {
                    this.control = Control::Bool(*value);
                    this.changed(cx);
                }))
                .into_any_element(),
            Control::Enum(state) => {
                Select::new(state).id("value-enum").accessibility_label(label).w_80().max_w_full().into_any_element()
            }
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
    pub fn translated(self, catalog: &crate::catalog::Catalog) -> Self {
        match self {
            Self::Enum(choices) => Self::Enum(
                choices
                    .into_iter()
                    .map(|choice| EnumChoice::new(choice.value, catalog.translate(&choice.label)))
                    .collect(),
            ),
            other => other,
        }
    }
    pub fn for_option(option: Option<&profile_core::OptionDef>) -> Self {
        match option {
            Some(option) if option.type_ == profile_core::OptionType::Bool => Self::Bool,
            Some(option) if option.type_ == profile_core::OptionType::Enum && !option.enum_keys.is_empty() => {
                Self::Enum(
                    option
                        .enum_keys
                        .iter()
                        .map(|key| {
                            let label = option
                                .enum_values
                                .iter()
                                .position(|v| v == key)
                                .and_then(|ix| option.enum_labels.get(ix))
                                .cloned()
                                .unwrap_or_else(|| key.clone());
                            EnumChoice::new(key.clone(), label)
                        })
                        .collect(),
                )
            }
            Some(option) if option.multiline => Self::Multiline,
            _ => Self::Text,
        }
    }
}
