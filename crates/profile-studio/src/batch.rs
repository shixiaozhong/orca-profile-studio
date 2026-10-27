//! Batch editing: choose presets, prepare one operation, review its effects, then apply to Edits.
use crate::batch_plan::{BatchPlan, Operation};
use crate::catalog::Catalog;
use crate::edits::Edits;
use crate::preset_picker::{Choice, PresetPicker};
use crate::value_editor::{EditorKind, EnumChoice, ValueEditor, ValueEditorEvent};
use crate::values::{RedoEdit, SaveEdits, UndoEdit};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use profile_core::{OptionType, PresetId, PresetKind, RawValue};
use std::sync::Arc;

pub struct PreviewTable {
    plan: Option<Arc<BatchPlan>>,
}
impl TableDelegate for PreviewTable {
    fn columns_count(&self, _: &App) -> usize {
        7
    }
    fn rows_count(&self, _: &App) -> usize {
        self.plan.as_ref().map_or(0, |p| p.rows().len())
    }
    fn column(&self, col: usize, _: &App) -> Column {
        let (key, title, width) = [
            ("preset", t!("common.preset"), 240.),
            ("before", t!("batch.before_source"), 280.),
            ("after", t!("batch.after_source"), 280.),
            ("status", t!("batch.change"), 160.),
            ("issues", t!("batch.new_issues"), 340.),
            ("descendants", t!("batch.descendants"), 150.),
            ("path", t!("common.file"), 320.),
        ][col]
            .clone();
        Column::new(key, title).width(px(width))
    }
    fn cell_text(&self, row: usize, col: usize, _: &App) -> String {
        self.plan.as_ref().and_then(|p| p.rows().get(row)).map_or(String::new(), |r| r.cell(col).into())
    }
    fn render_td(
        &mut self,
        row: usize,
        col: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        if col == 0 {
            let plan = self.plan.as_ref().unwrap().clone();
            let id = SharedString::from(format!("batch-review-{}", plan.rows()[row].cell(6)));
            return Button::new(id)
                .small()
                .ghost()
                .label(format!("{}…", plan.rows()[row].cell(0)))
                .on_click(move |_, window, cx| {
                    let plan = plan.clone();
                    window.open_dialog(cx, move |dialog, _, _| {
                        let row = &plan.rows()[row];
                        let content = v_flex().gap_3().children(
                            [
                                (t!("common.file"), 6),
                                (t!("common.before"), 1),
                                (t!("common.after"), 2),
                                (t!("batch.change"), 3),
                                (t!("batch.new_issues"), 4),
                                (t!("batch.descendants"), 5),
                            ]
                            .into_iter()
                            .map(|(title, col)| {
                                v_flex().gap_1().child(div().font_semibold().child(title)).child(div().child(
                                    if row.cell(col).is_empty() { t!("common.none") } else { row.cell(col).to_owned() },
                                ))
                            }),
                        );
                        dialog.title(row.cell(0).to_owned()).w(px(680.)).child(
                            div()
                                .id("batch-row-detail")
                                .test_support()
                                .aria_label(row.cell(6).to_owned())
                                .max_h(rems(26.))
                                .overflow_y_scroll()
                                .text_sm()
                                .child(content),
                        )
                    });
                })
                .into_any_element();
        }
        div().text_sm().child(self.cell_text(row, col, cx)).into_any_element()
    }
}

pub enum BatchEvent {
    Close,
}
impl EventEmitter<BatchEvent> for BatchView {}

pub struct BatchView {
    catalog: Arc<Catalog>,
    edits: Entity<Edits>,
    picker: Entity<PresetPicker>,
    selected: Vec<PresetId>,
    kind: PresetKind,
    key: String,
    key_select: Entity<SelectState<SearchableVec<EnumChoice>>>,
    operation: Operation,
    operation_select: Entity<SelectState<SearchableVec<Choice>>>,
    editor: Option<Entity<ValueEditor>>,
    editor_subscription: Option<Subscription>,
    preview: Entity<TableState<PreviewTable>>,
    generation: u64,
    task: Option<Task<()>>,
    status: String,
    draft_dirty: bool,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}
impl BatchView {
    pub fn new(
        catalog: &Arc<Catalog>,
        edits: &Entity<Edits>,
        initial: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picker = cx.new(|cx| {
            let mut picker = PresetPicker::new(catalog, initial, window, cx);
            picker.enable_issue_filter();
            picker
        });
        let kind = picker.read(cx).kind();
        let selected = picker.read(cx).selected(cx);
        let key_select = cx.new(|cx| {
            SelectState::new(SearchableVec::new(Vec::<EnumChoice>::new()), None, window, cx).searchable(true)
        });
        let operation_select =
            cx.new(|cx| SelectState::new(SearchableVec::new(Vec::<Choice>::new()), None, window, cx).searchable(true));
        let preview = cx.new(|cx| TableState::new(PreviewTable { plan: None }, window, cx));
        let subscriptions = vec![
            cx.observe_in(&picker, window, |this, picker, window, cx| {
                let kind = picker.read(cx).kind();
                let selected = picker.read(cx).selected(cx);
                if kind != this.kind || selected != this.selected {
                    let kind_changed = kind != this.kind;
                    this.kind = kind;
                    this.selected = selected;
                    this.invalidate(cx);
                    if kind_changed {
                        this.reset_keys(window, cx);
                    } else if !this.draft_dirty && this.operation == Operation::Set {
                        // Untouched input follows the selection, so it shows a real current value.
                        this.reset_editor(window, cx);
                    }
                }
            }),
            cx.subscribe_in(
                &key_select,
                window,
                |this, _, event: &SelectEvent<SearchableVec<EnumChoice>>, window, cx| {
                    if let SelectEvent::Confirm(Some(key)) = event
                        && *key != this.key
                    {
                        this.key = key.clone();
                        this.operation = Operation::Set;
                        this.reset_operation(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &operation_select,
                window,
                |this, _, event: &SelectEvent<SearchableVec<Choice>>, window, cx| {
                    if let SelectEvent::Confirm(Some(ix)) = event
                        && Operation::ALL[*ix] != this.operation
                    {
                        this.operation = Operation::ALL[*ix];
                        this.reset_editor(window, cx);
                    }
                },
            ),
            cx.observe(edits, |this, _, cx| this.invalidate(cx)),
        ];
        let mut view = Self {
            catalog: catalog.clone(),
            edits: edits.clone(),
            picker,
            selected,
            kind,
            key: String::new(),
            key_select,
            operation: Operation::Set,
            operation_select,
            editor: None,
            editor_subscription: None,
            preview,
            generation: 0,
            task: None,
            status: t!("batch.start"),
            draft_dirty: false,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        view.reset_keys(window, cx);
        view
    }
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus.focus(window, cx);
    }
    /// The presets a typed but unapplied value would change. Choosing a key or operation is not input.
    pub fn unapplied_presets(&self, cx: &App) -> Vec<usize> {
        let typed = self.draft_dirty && self.editor.as_ref().is_some_and(|e| e.read(cx).has_unapplied_input(cx));
        if !typed {
            return Vec::new();
        }
        self.picker.read(cx).selected(cx).into_iter().map(PresetId::index).collect()
    }
    pub fn refresh_language(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker.update(cx, |picker, cx| picker.refresh_language(window, cx));
        self.update_key_choices(window, cx);
        self.update_operation_choices(window, cx);
        if let Some(editor) = &self.editor {
            editor.update(cx, |editor, cx| editor.refresh_language(&self.catalog, window, cx));
        }
        self.invalidate(cx);
    }

    pub fn refresh(&mut self, catalog: &Arc<Catalog>, window: &mut Window, cx: &mut Context<Self>) {
        self.catalog = catalog.clone();
        self.picker.update(cx, |picker, cx| picker.refresh(catalog, window, cx));
        self.selected = self.picker.read(cx).selected(cx);
        self.invalidate(cx);
    }
    fn invalidate(&mut self, cx: &mut Context<Self>) {
        if self.task.is_some() || self.preview.read(cx).delegate().plan.is_some() {
            self.status = t!("batch.stale");
        }
        self.generation += 1;
        self.task = None;
        self.preview.update(cx, |table, cx| {
            table.delegate_mut().plan = None;
            table.refresh(cx);
        });
        cx.notify();
    }
    fn update_key_choices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(schema) = self.catalog.repo.schema() else { return };
        let mut keys = schema.default_preset(self.kind).values.keys().cloned().collect::<Vec<_>>();
        keys.sort();
        if !keys.contains(&self.key) {
            self.key = keys.iter().find(|k| k.as_str() == "wall_loops").or(keys.first()).cloned().unwrap_or_default();
        }
        let choices: Vec<_> = keys
            .into_iter()
            .map(|key| {
                let label = self.catalog.translate(schema.option(&key).map_or(key.as_str(), |o| o.display_label()));
                EnumChoice::new(key.clone(), format!("{label} — {key}"))
            })
            .collect();
        self.key_select.update(cx, |state, cx| {
            state.set_items(SearchableVec::new(choices), window, cx);
            state.set_selected_value(&self.key, window, cx);
        });
    }
    fn reset_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update_key_choices(window, cx);
        self.operation = Operation::Set;
        self.reset_operation(window, cx);
    }
    fn update_operation_choices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let strings = self
            .catalog
            .repo
            .schema()
            .and_then(|s| s.option(&self.key))
            .is_some_and(|o| o.type_ == OptionType::Strings);
        let choices: Vec<_> = Operation::ALL
            .into_iter()
            .enumerate()
            .filter(|(_, op)| !matches!(op, Operation::AddItem | Operation::RemoveItem) || strings)
            .map(|(value, op)| Choice { value, title: op.label().into() })
            .collect();
        self.operation_select.update(cx, |state, cx| {
            state.set_items(SearchableVec::new(choices), window, cx);
            state.set_selected_value(&(self.operation as usize), window, cx);
        });
    }
    fn reset_operation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update_operation_choices(window, cx);
        self.reset_editor(window, cx);
    }
    fn reset_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.invalidate(cx);
        self.editor = None;
        self.editor_subscription = None;
        self.draft_dirty = false;
        if matches!(self.operation, Operation::Remove | Operation::Promote) {
            return;
        }
        let Some(schema) = self.catalog.repo.schema() else { return };
        let Some(default) = schema.default_preset(self.kind).values.get(&self.key) else { return };
        let (kind, source) = if matches!(self.operation, Operation::AddItem | Operation::RemoveItem) {
            (EditorKind::Text, RawValue::Str(String::new()))
        } else {
            // Start from the first selected preset's current value, not the schema default: an
            // untouched Preview then shows what would change instead of a mass reset to defaults.
            let current = self.selected.first().and_then(|id| self.edits.read(cx).effective(id.index(), &self.key));
            (
                EditorKind::for_option(schema.option(&self.key)).translated(&self.catalog),
                current.map_or_else(|| default.clone(), |(v, _)| v),
            )
        };
        let editor = cx.new(|cx| ValueEditor::new(self.key.clone(), kind, source, false, window, cx));
        self.editor_subscription = Some(cx.subscribe(&editor, |this, _, event: &ValueEditorEvent, cx| {
            if matches!(event, ValueEditorEvent::Changed) {
                this.draft_dirty = true;
                this.invalidate(cx);
            }
        }));
        self.editor = Some(editor);
    }
    fn prepare(&mut self, cx: &mut Context<Self>) {
        self.invalidate(cx);
        let generation = self.generation;
        let catalog = self.catalog.clone();
        let baseline = self.edits.read(cx).clone();
        let selected = self.picker.read(cx).selected(cx);
        let key = self.key.clone();
        let operation = self.operation;
        let value = self.editor.as_ref().and_then(|e| e.read(cx).value(cx));
        self.status = t!("batch.preparing");
        let job = cx
            .background_executor()
            .spawn(async move { BatchPlan::build(&catalog, baseline, selected, key, operation, value) });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.task = None;
                match result {
                    Ok(plan) if plan.is_current(this.edits.read(cx)) => {
                        this.status = if plan.impact_count() > 0 {
                            t!(
                                "promote.summary",
                                selected = plan.selected_count(),
                                changed = plan.changed_count(),
                                affected = plan.impact_count()
                            )
                        } else {
                            t!("batch.summary", arg0 = plan.selected_count(), arg1 = plan.changed_count())
                        };
                        this.preview.update(cx, |table, cx| {
                            table.delegate_mut().plan = Some(Arc::new(plan));
                            table.refresh(cx);
                        });
                    }
                    Ok(_) => this.status = t!("batch.changed"),
                    Err(message) => this.status = message,
                }
                cx.notify();
            })
            .ok();
        }));
    }
    fn apply(&mut self, cx: &mut Context<Self>) {
        let Some(plan) = self.preview.read(cx).delegate().plan.clone() else { return };
        let result = self.edits.update(cx, |edits, cx| {
            let result = plan.apply(edits);
            if result.is_ok() {
                cx.notify();
            }
            result
        });
        self.invalidate(cx);
        match result {
            Ok(_) => {
                self.draft_dirty = false;
                self.status = if plan.changed_count() == 1 {
                    t!("batch.applied.one", arg0 = plan.changed_count())
                } else {
                    t!("batch.applied.other", arg0 = plan.changed_count())
                };
            }
            Err(message) => self.status = message,
        }
        cx.notify();
    }
    fn history(&mut self, redo: bool, cx: &mut Context<Self>) {
        let Some(group) = self.edits.read(cx).latest_group(redo) else { return };
        let result = self.edits.update(cx, |edits, cx| {
            let result = edits.history_group(group, redo);
            cx.notify();
            result
        });
        self.status = match result {
            Ok(n) => crate::i18n::history_message(redo, n),
            Err(message) => message,
        };
        cx.notify();
    }
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        crate::save::confirm(&self.edits, window, cx, move |(_, message), cx| {
            view.update(cx, |view, cx| {
                view.status = message;
                cx.notify();
            })
            .ok();
        });
    }
    #[cfg(test)]
    pub fn table(&self) -> &Entity<TableState<PreviewTable>> {
        &self.preview
    }
}
impl Render for BatchView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let edits = self.edits.read(cx);
        let undo = edits.latest_group(false).is_some();
        let redo = edits.latest_group(true).is_some();
        let dirty = !edits.dirty().is_empty();
        let apply = self.preview.read(cx).delegate().plan.as_ref().is_some_and(|p| p.changed_count() > 0);
        v_flex()
            .id("batch-view")
            .test_support()
            .key_context("ValuesView")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &UndoEdit, _, cx| this.history(false, cx)))
            .on_action(cx.listener(|this, _: &RedoEdit, _, cx| this.history(true, cx)))
            .on_action(cx.listener(|this, _: &SaveEdits, window, cx| this.save(window, cx)))
            .size_full()
            .p_3()
            .gap_2()
            .child(
                h_flex()
                    .gap_3()
                    .flex_shrink_0()
                    .child(
                        Button::new("batch-close")
                            .small()
                            .label(t!("common.back_browser"))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(BatchEvent::Close))),
                    )
                    .child(div().font_semibold().child(t!("common.batch_edit"))),
            )
            .child(div().flex_1().min_h_32().child(self.picker.clone()))
            .child(
                div().id("batch-form").max_h(relative(0.4)).flex_shrink_0().overflow_y_scroll().child(
                    v_flex()
                        .gap_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .flex_wrap()
                                .child(div().child(t!("common.key")))
                                .child(
                                    div().w_96().h_8().child(
                                        Select::new(&self.key_select)
                                            .id("batch-key")
                                            .accessibility_label(t!("batch.key"))
                                            .small(),
                                    ),
                                )
                                .child(div().child(t!("batch.operation")))
                                .child(
                                    div().w_56().h_8().child(
                                        Select::new(&self.operation_select)
                                            .id("batch-operation")
                                            .accessibility_label(t!("batch.operation_label"))
                                            .small(),
                                    ),
                                ),
                        )
                        .children(self.editor.clone())
                        .child(div().text_xs().text_color(cx.theme().muted_foreground).child(
                            if self.operation == Operation::Promote {
                                t!("promote.help")
                            } else if self.operation == Operation::Remove {
                                t!("batch.remove_help")
                            } else {
                                t!("batch.array_help")
                            },
                        )),
                ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .flex_shrink_0()
                    .child(
                        Button::new("batch-preview-button")
                            .small()
                            .label(t!("batch.preview"))
                            .disabled(self.task.is_some() || self.selected.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.prepare(cx))),
                    )
                    .child(
                        Button::new("batch-apply")
                            .small()
                            .label(t!("batch.apply"))
                            .disabled(!apply)
                            .on_click(cx.listener(|this, _, _, cx| this.apply(cx))),
                    )
                    .child(
                        Button::new("batch-discard-input")
                            .small()
                            .label(t!("batch.reset"))
                            .disabled(!self.draft_dirty)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.reset_editor(window, cx);
                                this.draft_dirty = false;
                                this.status = t!("batch.input_reset");
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("batch-undo")
                            .small()
                            .label(t!("batch.undo"))
                            .disabled(!undo)
                            .on_click(cx.listener(|this, _, _, cx| this.history(false, cx))),
                    )
                    .child(
                        Button::new("batch-redo")
                            .small()
                            .label(t!("batch.redo"))
                            .disabled(!redo)
                            .on_click(cx.listener(|this, _, _, cx| this.history(true, cx))),
                    )
                    .child(
                        Button::new("batch-save")
                            .small()
                            .label(t!("common.save"))
                            .disabled(!dirty)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
            .child(
                div()
                    .id("batch-status")
                    .test_support()
                    .aria_label(self.status.clone())
                    .text_sm()
                    .flex_shrink_0()
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .id("batch-preview")
                    .test_support()
                    .flex_1()
                    .min_h_24()
                    .child(DataTable::new(&self.preview).stripe(true)),
            )
    }
}
