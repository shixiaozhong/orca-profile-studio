//! The "Values" tab of a preset: every effective key, the layer of the inheritance chain it
//! comes from, the values it overrides, and an editor for the preset's own values.

use std::cmp::Ordering;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableEvent, TableState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use profile_core::{Origin, RawValue, Severity};

use crate::catalog::Catalog;
use crate::details::{format_value, severity_color};
use crate::edits::Edits;
use crate::value_editor::{ValueEditor, ValueEditorEvent};

actions!(values, [UndoEdit, RedoEdit, SaveEdits]);

const CONTEXT: &str = "ValuesView";

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-z", UndoEdit, Some(CONTEXT)),
        KeyBinding::new("secondary-shift-z", RedoEdit, Some(CONTEXT)),
        KeyBinding::new("secondary-s", SaveEdits, Some(CONTEXT)),
    ]);
}

/// The preset of the inheritance chain a value comes from, ordered from the preset itself up.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Source {
    /// Position in the chain (0 = the preset itself).
    pub depth: usize,
    pub name: String,
}

impl Source {
    pub fn label(&self) -> String {
        match self.depth {
            0 => t!("common.this_file"),
            depth => format!("[{depth}] {}", self.name),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Layer {
    pub source: Source,
    pub value: String,
}

/// One key of a preset, flattened to display strings.
#[derive(Debug, Clone)]
pub struct ValueRow {
    pub key: String,
    /// Nearest first; the first layer is the effective value.
    pub layers: Vec<Layer>,
    pub issues: Vec<(Severity, String)>,
    /// The effective value as stored, which an override starts from.
    pub effective: RawValue,
    /// The preset's own (edited) value, if it sets the key.
    pub own: Option<RawValue>,
    /// Differs from the saved file.
    pub modified: bool,
}

impl ValueRow {
    pub fn source(&self) -> &Source {
        &self.layers[0].source
    }

    pub fn value(&self) -> &str {
        &self.layers[0].value
    }

    pub fn severity(&self) -> Option<Severity> {
        self.issues.iter().map(|(severity, _)| *severity).max()
    }

    /// Whether removing the own value leaves an inherited value.
    pub fn inherits(&self) -> bool {
        self.layers.iter().any(|layer| !is_own(&layer.source))
    }

    fn source_text(&self) -> String {
        let source = self.source().label();
        if self.modified { t!("values.edited_source", source = source) } else { source }
    }
}

fn is_own(source: &Source) -> bool {
    source.depth == 0
}

/// Builds the rows of a preset with its edited values, sorted by key.
pub fn build_rows(catalog: &Catalog, edits: &Edits, preset: usize) -> Vec<ValueRow> {
    let repo = &catalog.repo;
    let id = repo.presets()[preset].id;
    let chain = repo.chain(id);
    let own = edits.session(preset).map_or(&repo.preset(id).params, |session| session.params());
    // Compare with what the parent inherits now, including unsaved edits of the ancestors.
    let issues = repo.preset(id).parent.map_or_else(Vec::new, |parent| {
        repo.redundant_overrides_against(id, own, |key| edits.effective(parent.index(), key).map(|(value, _)| value))
    });

    let mut rows: Vec<ValueRow> = repo
        .trace_with(id, own)
        .into_iter()
        .map(|trace| {
            let layers = trace
                .layers
                .iter()
                .map(|layer| {
                    let Origin::Preset(link) = layer.origin;
                    let depth = chain.iter().position(|&entry| entry == link).unwrap_or(0);
                    let source = Source { depth, name: repo.preset(link).name().to_owned() };
                    Layer { source, value: format_value(layer.value) }
                })
                .collect();
            let key_issues = issues
                .iter()
                .filter(|issue| issue.key() == Some(trace.key))
                .map(|issue| (issue.severity(), crate::issue_text::issue(issue)))
                .collect();
            ValueRow {
                key: trace.key.to_owned(),
                layers,
                issues: key_issues,
                effective: trace.effective().value.clone(),
                own: own.get(trace.key).cloned(),
                modified: edits.session(preset).is_some_and(|session| session.is_modified(trace.key)),
            }
        })
        .collect();
    rows.sort_by(default_order);
    rows
}

fn default_order(a: &ValueRow, b: &ValueRow) -> Ordering {
    a.key.cmp(&b.key)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Case-insensitive text matched against the key.
    pub query: String,
    pub only_this_file: bool,
}

impl Filter {
    pub fn matches(&self, row: &ValueRow) -> bool {
        if self.only_this_file && !is_own(row.source()) {
            return false;
        }
        let query = self.query.trim().to_lowercase();
        row.key.to_lowercase().contains(&query)
    }
}

fn columns() -> [(&'static str, String, f32); 3] {
    [("key", t!("common.key"), 300.), ("value", t!("common.value"), 340.), ("source", t!("common.source"), 220.)]
}

pub struct ValuesDelegate {
    rows: Vec<ValueRow>,
    /// Indexes into `rows` that pass the filter, in display order.
    visible: Vec<usize>,
    sort: Option<(usize, ColumnSort)>,
}

impl ValuesDelegate {
    fn new(rows: Vec<ValueRow>) -> Self {
        let visible = (0..rows.len()).collect();
        ValuesDelegate { rows, visible, sort: None }
    }

    pub fn visible_rows(&self) -> impl Iterator<Item = &ValueRow> {
        self.visible.iter().map(|&index| &self.rows[index])
    }

    pub fn row(&self, visible_ix: usize) -> Option<&ValueRow> {
        self.visible.get(visible_ix).map(|&index| &self.rows[index])
    }

    fn find(&self, key: &str) -> Option<&ValueRow> {
        self.rows.iter().find(|row| row.key == key)
    }

    fn apply(&mut self, filter: &Filter) {
        self.visible = (0..self.rows.len()).filter(|&index| filter.matches(&self.rows[index])).collect();
        self.sort_visible();
    }

    fn sort_visible(&mut self) {
        let rows = &self.rows;
        match self.sort {
            Some((col_ix, sort @ (ColumnSort::Ascending | ColumnSort::Descending))) => {
                self.visible.sort_by(|&a, &b| {
                    let order = match col_ix {
                        2 => rows[a].source().cmp(rows[b].source()),
                        _ => {
                            cell_text(&rows[a], col_ix).to_lowercase().cmp(&cell_text(&rows[b], col_ix).to_lowercase())
                        }
                    };
                    let order = order.then_with(|| default_order(&rows[a], &rows[b]));
                    if sort == ColumnSort::Descending { order.reverse() } else { order }
                });
            }
            _ => self.visible.sort(),
        }
    }
}

fn cell_text(row: &ValueRow, col_ix: usize) -> String {
    match col_ix {
        0 => row.key.clone(),
        // Multi-line values (G-code) are shown on one line in the table.
        1 => row.value().replace('\n', " ⏎ "),
        _ => row.source_text(),
    }
}

impl TableDelegate for ValuesDelegate {
    fn columns_count(&self, _: &App) -> usize {
        columns().len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.visible.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let (key, name, width) = columns()[col_ix].clone();
        Column::new(key, name).width(px(width)).sortable()
    }

    fn perform_sort(&mut self, col_ix: usize, sort: ColumnSort, _: &mut Window, _: &mut Context<TableState<Self>>) {
        self.sort = Some((col_ix, sort));
        self.sort_visible();
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(row) = self.row(row_ix) else { return div().into_any_element() };
        let muted = cx.theme().muted_foreground;
        let text = div().min_w_0().truncate().child(cell_text(row, col_ix));
        match col_ix {
            0 => h_flex()
                .gap_1()
                .min_w_0()
                .when(row.modified, |cell| {
                    cell.child(Icon::new(IconName::Pencil).xsmall().text_color(cx.theme().primary))
                })
                .when_some(row.severity().filter(|&severity| severity >= Severity::Warning), |cell, severity| {
                    let color = severity_color(severity, cx).unwrap_or(muted);
                    cell.child(Icon::new(IconName::TriangleAlert).xsmall().text_color(color))
                })
                .child(text)
                .into_any_element(),
            2 if row.modified => text.text_color(cx.theme().primary).into_any_element(),
            _ => text.into_any_element(),
        }
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        self.row(row_ix).map(|row| cell_text(row, col_ix)).unwrap_or_default()
    }
}

pub struct ValuesView {
    catalog: Arc<Catalog>,
    edits: Entity<Edits>,
    preset: usize,
    search: Entity<InputState>,
    table: Entity<TableState<ValuesDelegate>>,
    filter: Filter,
    /// Key of the selected row; survives filtering.
    selected: Option<String>,
    editor: Option<Entity<ValueEditor>>,
    editor_subscription: Option<Subscription>,
    status: Option<(bool, String)>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

/// Asks the window to compare this preset with others.
pub struct CompareRequest(pub usize);

impl EventEmitter<CompareRequest> for ValuesView {}

impl ValuesView {
    pub fn new(
        catalog: &Arc<Catalog>,
        edits: &Entity<Edits>,
        preset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let rows = build_rows(catalog, edits.read(cx), preset);
        let table = cx.new(|cx| TableState::new(ValuesDelegate::new(rows), window, cx).row_selectable(true));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("values.filter")));
        let subscriptions = vec![
            cx.subscribe_in(&search, window, |this, search, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.filter.query = search.read(cx).value().to_string();
                    this.apply_filter(cx);
                }
            }),
            cx.subscribe_in(&table, window, |this, table, event: &TableEvent, window, cx| {
                if let TableEvent::SelectRow(row_ix) = event {
                    this.selected = table.read(cx).delegate().row(*row_ix).map(|row| row.key.clone());
                    this.sync_editor(window, cx);
                    cx.notify();
                }
            }),
            // Edits, undo and redo change the values: rebuild the rows, then the editor.
            cx.observe_in(edits, window, |this, _, window, cx| {
                this.rebuild(cx);
                this.sync_editor(window, cx);
            }),
        ];
        ValuesView {
            catalog: catalog.clone(),
            edits: edits.clone(),
            preset,
            search,
            table,
            filter: Filter::default(),
            selected: None,
            editor: None,
            editor_subscription: None,
            status: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    pub fn preset(&self) -> usize {
        self.preset
    }

    pub fn has_unapplied_input(&self, cx: &App) -> bool {
        self.editor.as_ref().is_some_and(|editor| editor.read(cx).has_unapplied_input(cx))
    }

    /// Keep the retained controls (including unapplied text), filter, sort and selected key.
    pub fn refresh(&mut self, catalog: &Arc<Catalog>, preset: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.catalog = catalog.clone();
        self.preset = preset;
        self.search.update(cx, |state, cx| state.set_placeholder(t!("values.filter"), window, cx));
        self.rebuild(cx);
        self.sync_editor(window, cx);
        if let Some(editor) = &self.editor {
            editor.update(cx, |_, cx| cx.notify());
        }
    }

    /// Filters the table to `key` and selects its row, e.g. after a global search.
    pub fn reveal_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        // set_value does not emit a change event, so update the filter directly.
        self.search.update(cx, |search, cx| search.set_value(key.to_owned(), window, cx));
        self.filter.query = key.to_owned();
        self.selected = Some(key.to_owned());
        self.apply_filter(cx);
        self.sync_editor(window, cx);
    }

    #[cfg(test)]
    pub fn table(&self) -> &Entity<TableState<ValuesDelegate>> {
        &self.table
    }

    /// Rebuilds the rows after an edit, keeping filter, sort and selection.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let rows = build_rows(&self.catalog, self.edits.read(cx), self.preset);
        self.table.update(cx, |table, _| table.delegate_mut().rows = rows);
        self.apply_filter(cx);
    }

    fn apply_filter(&mut self, cx: &mut Context<Self>) {
        let filter = self.filter.clone();
        let selected = self.selected.clone();
        self.table.update(cx, |table, cx| {
            table.delegate_mut().apply(&filter);
            let position = selected.and_then(|key| table.delegate().visible_rows().position(|row| row.key == key));
            match position {
                Some(row_ix) => table.set_selected_row(row_ix, cx),
                None => table.clear_selection(cx),
            }
            table.refresh(cx);
        });
        cx.notify();
    }

    fn toggle(&mut self, update: impl FnOnce(&mut Filter, bool), checked: bool, cx: &mut Context<Self>) {
        update(&mut self.filter, checked);
        self.apply_filter(cx);
    }

    fn selected_row(&self, cx: &App) -> Option<ValueRow> {
        let key = self.selected.as_ref()?;
        self.table.read(cx).delegate().find(key).cloned()
    }

    /// Sets (or with `None` removes) a value of the preset in its edit session.
    fn set_value(&mut self, key: &str, value: Option<RawValue>, cx: &mut Context<Self>) {
        let preset = self.preset;
        self.status = None;
        self.edits.update(cx, |edits, cx| {
            if edits.session_mut(preset).set(key, value) {
                cx.notify();
            }
        });
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        self.history(false, cx);
    }
    fn redo(&mut self, cx: &mut Context<Self>) {
        self.history(true, cx);
    }

    fn history(&mut self, redo: bool, cx: &mut Context<Self>) {
        let result = self.edits.update(cx, |edits, cx| {
            let result = edits.history(self.preset, redo);
            cx.notify();
            result
        });
        match result {
            Ok((key, count)) => {
                self.select(key, cx);
                self.status = (count > 0).then(|| (true, crate::i18n::history_message(redo, count)));
            }
            Err(message) => self.status = Some((false, message)),
        }
        cx.notify();
    }

    /// Selects the key an undo or redo changed, so the change is visible.
    fn select(&mut self, key: Option<String>, cx: &mut Context<Self>) {
        if key.is_some() {
            self.selected = key;
            self.status = None;
            self.apply_filter(cx);
        }
    }

    /// Retain input until its key or baseline changes. Construction happens only in handlers.
    fn sync_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.selected_row(cx) else {
            self.editor = None;
            return;
        };
        let Some(own) = row.own.clone() else {
            self.editor = None;
            return;
        };
        if self.editor.as_ref().is_some_and(|editor| {
            let editor = editor.read(cx);
            editor.key() == row.key && editor.source() == &own
        }) {
            return;
        }
        let key = row.key.clone();
        let editor = cx.new(|cx| ValueEditor::new(row.key, own, true, window, cx));
        self.editor_subscription = Some(cx.subscribe(&editor, move |this, _, event: &ValueEditorEvent, cx| {
            if let ValueEditorEvent::Apply(value) = event {
                this.set_value(&key, Some(value.clone()), cx);
            }
        }));
        self.editor = Some(editor);
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let table = self.table.read(cx);
        let shown = table.delegate().visible.len();
        let total = table.delegate().rows.len();
        let count = SharedString::from(t!("values.count", shown = shown, total = total));
        let edits = self.edits.read(cx);
        let session = edits.session(self.preset);
        let can_undo = session.is_some_and(|session| session.can_undo());
        let can_redo = session.is_some_and(|session| session.can_redo());
        let pending: usize =
            edits.dirty().iter().filter_map(|&preset| edits.session(preset)).map(|s| s.pending().len()).sum();
        h_flex()
            .gap_3()
            .flex_wrap()
            .pb_2()
            .items_center()
            .child(
                Input::new(&self.search)
                    .id("values-search")
                    .aria_label(t!("values.filter_label"))
                    .small()
                    .prefix(Icon::new(IconName::Search).small())
                    .cleanable(true)
                    .w(rems(15.)),
            )
            .child(
                Checkbox::new("only-this-file")
                    .label(t!("values.only_own"))
                    .checked(self.filter.only_this_file)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.toggle(|filter, checked| filter.only_this_file = checked, *checked, cx)
                    })),
            )
            .child(
                div()
                    .id("values-count")
                    .test_support()
                    .aria_label(count.clone())
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(count),
            )
            .child(div().flex_1())
            .child(
                Button::new("compare-with")
                    .small()
                    .label(t!("values.compare"))
                    .on_click(cx.listener(|this, _, _, cx| cx.emit(CompareRequest(this.preset)))),
            )
            .child(
                Button::new("undo")
                    .small()
                    .accessibility_label(t!("values.undo"))
                    .icon(IconName::Undo2)
                    .tooltip(t!("values.undo_shortcut", key = crate::i18n::shortcut("secondary-z")))
                    .disabled(!can_undo)
                    .on_click(cx.listener(|this, _, _, cx| this.undo(cx))),
            )
            .child(
                Button::new("redo")
                    .small()
                    .accessibility_label(t!("values.redo"))
                    .icon(IconName::Redo2)
                    .tooltip(t!("values.redo_shortcut", key = crate::i18n::shortcut("secondary-shift-z")))
                    .disabled(!can_redo)
                    .on_click(cx.listener(|this, _, _, cx| this.redo(cx))),
            )
            .child(
                Button::new("save")
                    .small()
                    .icon(IconName::Save)
                    .label(if pending == 0 {
                        t!("common.save")
                    } else {
                        if pending == 1 {
                            t!("values.save_changes.one", pending = pending)
                        } else {
                            t!("values.save_changes.other", pending = pending)
                        }
                    })
                    .disabled(pending == 0)
                    .on_click(cx.listener(|this, _, window, cx| this.confirm_save(window, cx))),
            )
    }

    fn confirm_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        crate::save::confirm(&self.edits, window, cx, move |status, cx| {
            view.update(cx, |view, cx| {
                view.status = Some(status);
                cx.notify();
            })
            .ok();
        });
    }

    /// The area below the table; its accessible label is the key it shows.
    fn panel(&self, label: SharedString, content: Div, cx: &App) -> AnyElement {
        div()
            .id("value-detail")
            .test_support()
            .aria_label(label)
            .h_72()
            .max_h(relative(0.5))
            .flex_shrink_0()
            .mt_2()
            .pt_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .overflow_y_scroll()
            .text_sm()
            .child(content)
            .into_any_element()
    }

    fn render_editor(&self, row: &ValueRow, cx: &mut Context<Self>) -> Div {
        let muted = cx.theme().muted_foreground;
        let key = row.key.clone();
        let mut section =
            v_flex().gap_1().pt_1().child(div().text_xs().font_semibold().text_color(muted).child(t!("values.edit")));
        let Some(_) = &row.own else {
            let effective = row.effective.clone();
            return section.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_color(muted).child(t!("values.inherited_help")))
                    .child(Button::new("override").small().label(t!("values.override")).on_click(
                        cx.listener(move |this, _, _, cx| this.set_value(&key, Some(effective.clone()), cx)),
                    )),
            );
        };
        section = section.children(self.editor.clone());
        let reset_label = if row.inherits() { t!("values.reset") } else { t!("values.remove") };
        section.child(
            h_flex().child(
                Button::new("reset")
                    .small()
                    .ghost()
                    .label(reset_label)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_value(&key, None, cx))),
            ),
        )
    }

    /// The selected key's details and editor, and the label of the panel showing them.
    fn render_detail(&self, cx: &mut Context<Self>) -> (SharedString, Div) {
        let muted = cx.theme().muted_foreground;
        let Some(row) = self.selected_row(cx) else {
            let hint = t!("values.select_hint");
            return (SharedString::default(), div().text_color(muted).child(hint));
        };
        let content = v_flex()
            .gap_1()
            .child(
                h_flex().gap_2().child(div().font_semibold().child(row.key.clone())).when(row.modified, |line| {
                    line.child(div().text_color(cx.theme().primary).child(t!("common.edited")))
                }),
            )
            .child(self.render_editor(&row, cx))
            .child(div().pt_1().text_xs().font_semibold().text_color(muted).child(t!("values.layers")))
            .children(row.layers.iter().enumerate().map(|(ix, layer)| {
                let source = layer.source.label();
                h_flex()
                    .id(("layer", ix))
                    .test_support()
                    .aria_label(SharedString::from(format!("{source}: {}", layer.value)))
                    .gap_3()
                    .items_start()
                    .when(ix > 0, |line| line.text_color(muted).line_through())
                    .child(div().w(rems(15.)).flex_shrink_0().truncate().child(source))
                    .child(div().flex_1().min_w_0().child(truncate_chars(&layer.value, 2000)))
            }))
            .when(!row.issues.is_empty(), |panel| {
                panel
                    .child(div().pt_1().text_xs().font_semibold().text_color(muted).child(t!("common.issues")))
                    .children(row.issues.iter().map(|(severity, message)| {
                        let color = severity_color(*severity, cx).unwrap_or(muted);
                        h_flex()
                            .gap_2()
                            .child(Icon::new(IconName::TriangleAlert).small().text_color(color))
                            .child(format!("{}: {message}", crate::i18n::severity(*severity)))
                    }))
            });
        (row.key.clone().into(), content)
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => t!("values.truncated", text = &text[..cut], count = text.chars().count()),
        None => text.to_owned(),
    }
}

impl Render for ValuesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self.status.clone().map(|(ok, message)| {
            let color = if ok { cx.theme().success } else { cx.theme().danger };
            div()
                .id("save-status")
                .test_support()
                .aria_label(SharedString::from(message.clone()))
                .pb_1()
                .text_sm()
                .text_color(color)
                .child(message)
        });
        let (label, content) = self.render_detail(cx);
        let detail = self.panel(label, content, cx);
        let empty = self.table.read(cx).delegate().visible.is_empty();
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &UndoEdit, _, cx| this.undo(cx)))
            .on_action(cx.listener(|this, _: &RedoEdit, _, cx| this.redo(cx)))
            .on_action(cx.listener(|this, _: &SaveEdits, window, cx| this.confirm_save(window, cx)))
            .size_full()
            .child(self.render_toolbar(cx))
            .children(status)
            .child(
                div()
                    .id("values-table")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .when(!empty, |body| body.child(DataTable::new(&self.table).stripe(true)))
                    .when(empty, |body| {
                        body.child(
                            div()
                                .id("values-empty")
                                .test_support()
                                .aria_label(t!("values.empty"))
                                .p_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(t!("values.empty_help")),
                        )
                    }),
            )
            .child(detail)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Filter, Layer, Source, ValueRow};
    use crate::value_editor::shaped;
    use profile_core::RawValue;

    fn source(depth: usize, name: &str) -> Source {
        Source { depth, name: name.to_owned() }
    }

    fn row(key: &str, source: Source) -> ValueRow {
        ValueRow {
            key: key.to_owned(),
            layers: vec![Layer { source, value: "1".to_owned() }],
            issues: Vec::new(),
            effective: RawValue::Str("1".into()),
            own: None,
            modified: false,
        }
    }

    #[test]
    fn filter_by_text_and_source() {
        let _locale = crate::i18n::test_locale();
        let own = row("nozzle_temperature", source(0, "self"));
        let inherited = row("filament_type", source(2, "base"));
        let other = row("filament_tower_interface_print_temp", source(1, "parent"));
        let shown = |filter: &Filter| {
            [&own, &inherited, &other]
                .into_iter()
                .filter(|row| filter.matches(row))
                .map(|row| row.key.as_str())
                .collect::<Vec<_>>()
        };

        assert_eq!(shown(&Filter::default()).len(), 3);
        assert_eq!(shown(&Filter { query: "TEMP".into(), ..Default::default() }), [&own.key, &other.key]);
        assert_eq!(shown(&Filter { query: "type".into(), ..Default::default() }), [&inherited.key]);
        assert_eq!(shown(&Filter { only_this_file: true, ..Default::default() }), [&own.key]);
    }

    #[test]
    fn sources_order_from_the_preset_down() {
        let _locale = crate::i18n::test_locale();
        assert!(source(0, "z") < source(1, "a"));
        assert_eq!(source(0, "x").label(), "this file");
        assert_eq!(source(2, "base").label(), "[2] base");
    }

    #[test]
    fn edits_keep_the_shape_of_the_value() {
        let _locale = crate::i18n::test_locale();
        let one = RawValue::List(vec!["220".into()]);
        assert_eq!(shaped(&one, vec!["230".into()]), RawValue::List(vec!["230".into()]));
        assert_eq!(shaped(&RawValue::Str("0.2".into()), vec!["0.3".into()]), RawValue::Str("0.3".into()));
        let two = shaped(&RawValue::Str("a".into()), vec!["a".into(), "b".into()]);
        assert_eq!(two, RawValue::List(vec!["a".into(), "b".into()]));
    }

    #[test]
    fn inherits_when_another_layer_sets_the_key() {
        let _locale = crate::i18n::test_locale();
        let mut own_only = row("x", source(0, "self"));
        assert!(!own_only.inherits());
        own_only.layers.push(Layer { source: source(1, "parent"), value: "0".into() });
        assert!(own_only.inherits());
    }
}
