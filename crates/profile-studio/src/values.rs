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
use crate::compare::CompareView;
use crate::details::{format_value, severity_color, with_unit};
use crate::edits::Edits;
pub use crate::value_editor::EditorKind;
use crate::value_editor::{ValueEditor, ValueEditorEvent};

actions!(values, [UndoEdit, RedoEdit, SaveEdits]);

const CONTEXT: &str = "ValuesView";

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-z", UndoEdit, Some(CONTEXT)),
        KeyBinding::new("cmd-shift-z", RedoEdit, Some(CONTEXT)),
        KeyBinding::new("cmd-s", SaveEdits, Some(CONTEXT)),
    ]);
}

/// Where a value comes from, ordered from the preset itself down to the schema defaults.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// Position in the chain (0 = the preset itself) and the name of that preset.
    Chain(usize, String),
    Default,
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Source::Chain(0, _) => "this file".to_owned(),
            Source::Chain(depth, name) => format!("[{depth}] {name}"),
            Source::Default => "default".to_owned(),
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
    pub label: String,
    pub category: String,
    pub tooltip: String,
    /// Type, default and limits from the schema, or a note that the key is not an option.
    pub definition: String,
    /// Nearest first; the first layer is the effective value.
    pub layers: Vec<Layer>,
    pub issues: Vec<(Severity, String)>,
    /// The effective value as stored, which an override starts from.
    pub effective: RawValue,
    /// The preset's own (edited) value, if it sets the key.
    pub own: Option<RawValue>,
    /// Differs from the saved file.
    pub modified: bool,
    pub editor: EditorKind,
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

    /// Whether removing the own value leaves an inherited or default value.
    pub fn inherits(&self) -> bool {
        self.layers.iter().any(|layer| !is_own(&layer.source))
    }

    fn source_text(&self) -> String {
        let source = self.source().label();
        if self.modified { format!("{source} (edited)") } else { source }
    }
}

fn is_own(source: &Source) -> bool {
    matches!(source, Source::Chain(0, _))
}

/// Builds the rows of a preset with its edited values, sorted by category and label.
pub fn build_rows(catalog: &Catalog, edits: &Edits, preset: usize) -> Vec<ValueRow> {
    let repo = &catalog.repo;
    let id = repo.presets()[preset].id;
    let chain = repo.chain(id);
    let schema = repo.schema();
    let own = edits.session(preset).map_or(&repo.preset(id).params, |session| session.params());
    let issues = repo.check_values(id, own);

    let mut rows: Vec<ValueRow> = repo
        .trace_with(id, own)
        .into_iter()
        .map(|trace| {
            let option = schema.and_then(|schema| schema.option(trace.key));
            let unit = option.map_or("", |option| option.sidetext.as_str());
            let layers = trace
                .layers
                .iter()
                .map(|layer| {
                    let source = match layer.origin {
                        Origin::Preset(link) => {
                            let depth = chain.iter().position(|&entry| entry == link).unwrap_or(0);
                            Source::Chain(depth, repo.preset(link).name().to_owned())
                        }
                        Origin::Default => Source::Default,
                    };
                    let value = with_unit(format_value(layer.value), layer.value, unit);
                    Layer { source, value }
                })
                .collect();
            let (label, category, tooltip, definition) = match option {
                Some(option) => {
                    let mut definition = format!("{} · {:?}", option.type_, option.mode).to_lowercase();
                    if option.nullable {
                        definition.push_str(" · nullable");
                    }
                    if option.min.is_some() || option.max.is_some() {
                        let bound =
                            |bound: Option<i64>| bound.map_or_else(|| "…".to_owned(), |bound| bound.to_string());
                        definition.push_str(&format!(" · range {}–{}", bound(option.min), bound(option.max)));
                    }
                    if !option.enum_keys.is_empty() {
                        definition.push_str(&format!(" · one of {}", option.enum_keys.join(", ")));
                    }
                    let category = if option.category.is_empty() { "Other" } else { &option.category };
                    (option.display_label().to_owned(), category.to_owned(), option.tooltip.clone(), definition)
                }
                None => {
                    let note = if schema.is_some() { "not an OrcaSlicer option" } else { "no schema loaded" };
                    (trace.key.to_owned(), "Not in schema".to_owned(), String::new(), note.to_owned())
                }
            };
            let editor = EditorKind::for_option(option);
            let key_issues = issues
                .iter()
                .filter(|issue| issue.key() == Some(trace.key))
                .map(|issue| (issue.severity(), issue.to_string()))
                .collect();
            ValueRow {
                key: trace.key.to_owned(),
                label,
                category,
                tooltip,
                definition,
                layers,
                issues: key_issues,
                effective: trace.effective().value.clone(),
                own: own.get(trace.key).cloned(),
                modified: edits.session(preset).is_some_and(|session| session.is_modified(trace.key)),
                editor,
            }
        })
        .collect();
    rows.sort_by(default_order);
    rows
}

fn default_order(a: &ValueRow, b: &ValueRow) -> Ordering {
    // Keys outside the schema go last.
    let unknown = |row: &ValueRow| row.category == "Not in schema";
    (unknown(a), &a.category, a.label.to_lowercase(), &a.key).cmp(&(
        unknown(b),
        &b.category,
        b.label.to_lowercase(),
        &b.key,
    ))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Case-insensitive text matched against key, label and category.
    pub query: String,
    pub hide_defaults: bool,
    pub only_this_file: bool,
}

impl Filter {
    pub fn matches(&self, row: &ValueRow) -> bool {
        if self.hide_defaults && *row.source() == Source::Default {
            return false;
        }
        if self.only_this_file && !is_own(row.source()) {
            return false;
        }
        let query = self.query.trim().to_lowercase();
        query.is_empty()
            || [&row.key, &row.label, &row.category].iter().any(|text| text.to_lowercase().contains(&query))
    }
}

const COLUMNS: [(&str, &str, f32); 5] = [
    ("category", "Category", 150.),
    ("label", "Option", 220.),
    ("key", "Key", 240.),
    ("value", "Value", 260.),
    ("source", "Source", 200.),
];

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
                        4 => rows[a].source().cmp(rows[b].source()),
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
        0 => row.category.clone(),
        1 => row.label.clone(),
        2 => row.key.clone(),
        // Multi-line values (G-code) are shown on one line in the table.
        3 => row.value().replace('\n', " ⏎ "),
        _ => row.source_text(),
    }
}

impl TableDelegate for ValuesDelegate {
    fn columns_count(&self, _: &App) -> usize {
        COLUMNS.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.visible.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let (key, name, width) = COLUMNS[col_ix];
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
            1 => h_flex()
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
            0 | 2 => text.text_color(muted).into_any_element(),
            4 if row.modified => text.text_color(cx.theme().primary).into_any_element(),
            4 if *row.source() == Source::Default => text.text_color(muted).into_any_element(),
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
    comparison: Option<Entity<CompareView>>,
    comparing: bool,
    _subscriptions: Vec<Subscription>,
}

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
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Filter by key, option or category"));
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
            comparison: None,
            comparing: false,
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
        if let Some(comparison) = &self.comparison {
            comparison.update(cx, |comparison, cx| comparison.refresh(catalog, preset, window, cx));
        }
        self.catalog = catalog.clone();
        self.preset = preset;
        self.rebuild(cx);
        self.sync_editor(window, cx);
    }

    /// Filters the table to `key` and selects its row, e.g. after a global search.
    pub fn reveal_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.comparing = false;
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

    #[cfg(test)]
    pub fn comparison(&self) -> &Entity<CompareView> {
        self.comparison.as_ref().expect("comparison opened")
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
                self.status = (count > 0).then(|| {
                    (true, format!("{} a batch change in {count} presets.", if redo { "Redid" } else { "Undid" }))
                });
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
        let editor = cx.new(|cx| ValueEditor::new(row.key, row.editor, own, true, window, cx));
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
        let count = SharedString::from(format!("{shown} of {total} keys"));
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
                    .aria_label("Filter values")
                    .small()
                    .prefix(Icon::new(IconName::Search).small())
                    .cleanable(true)
                    .w(rems(15.)),
            )
            .child(Checkbox::new("hide-defaults").label("Hide defaults").checked(self.filter.hide_defaults).on_click(
                cx.listener(|this, checked: &bool, _, cx| {
                    this.toggle(|filter, checked| filter.hide_defaults = checked, *checked, cx)
                }),
            ))
            .child(
                Checkbox::new("only-this-file").label("Only this file").checked(self.filter.only_this_file).on_click(
                    cx.listener(|this, checked: &bool, _, cx| {
                        this.toggle(|filter, checked| filter.only_this_file = checked, *checked, cx)
                    }),
                ),
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
            .child(Button::new("compare-with").small().label("Compare with…").on_click(cx.listener(
                |this, _, window, cx| {
                    if this.comparison.is_none() {
                        this.comparison =
                            Some(cx.new(|cx| CompareView::new(&this.catalog, &this.edits, this.preset, window, cx)));
                    }
                    this.comparing = true;
                    cx.notify();
                },
            )))
            .child(
                Button::new("undo")
                    .small()
                    .accessibility_label("Undo edit")
                    .icon(IconName::Undo2)
                    .tooltip("Undo (⌘Z)")
                    .disabled(!can_undo)
                    .on_click(cx.listener(|this, _, _, cx| this.undo(cx))),
            )
            .child(
                Button::new("redo")
                    .small()
                    .accessibility_label("Redo edit")
                    .icon(IconName::Redo2)
                    .tooltip("Redo (⇧⌘Z)")
                    .disabled(!can_redo)
                    .on_click(cx.listener(|this, _, _, cx| this.redo(cx))),
            )
            .child(
                Button::new("save")
                    .small()
                    .icon(IconName::Save)
                    .label(if pending == 0 { "Save…".to_owned() } else { format!("Save {pending} change(s)…") })
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
            v_flex().gap_1().pt_1().child(div().text_xs().font_semibold().text_color(muted).child("Edit"));
        let Some(_) = &row.own else {
            let effective = row.effective.clone();
            return section.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_color(muted).child("Inherited; override to set it in this file."))
                    .child(Button::new("override").small().label("Override").on_click(
                        cx.listener(move |this, _, _, cx| this.set_value(&key, Some(effective.clone()), cx)),
                    )),
            );
        };
        section = section.children(self.editor.clone());
        let reset_label = if row.inherits() { "Reset to inherited" } else { "Remove from file" };
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
            let hint = "Select a row to see every layer of the inheritance chain that sets it, and to edit it.";
            return (SharedString::default(), div().text_color(muted).child(hint));
        };
        let content = v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .child(div().font_semibold().child(row.label.clone()))
                    .child(div().text_color(muted).child(row.key.clone()))
                    .when(row.modified, |line| line.child(div().text_color(cx.theme().primary).child("edited"))),
            )
            .when(!row.tooltip.is_empty(), |panel| panel.child(div().text_color(muted).child(row.tooltip.clone())))
            .child(div().text_xs().text_color(muted).child(row.definition.clone()))
            .child(self.render_editor(&row, cx))
            .child(div().pt_1().text_xs().font_semibold().text_color(muted).child("Layers (nearest first)"))
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
                panel.child(div().pt_1().text_xs().font_semibold().text_color(muted).child("Issues")).children(
                    row.issues.iter().map(|(severity, message)| {
                        let color = severity_color(*severity, cx).unwrap_or(muted);
                        h_flex()
                            .gap_2()
                            .child(Icon::new(IconName::TriangleAlert).small().text_color(color))
                            .child(format!("{severity}: {message}"))
                    }),
                )
            });
        (row.key.clone().into(), content)
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}… ({} chars)", &text[..cut], text.chars().count()),
        None => text.to_owned(),
    }
}

impl Render for ValuesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.comparing {
            return v_flex()
                .size_full()
                .gap_2()
                .child(Button::new("close-comparison").small().label("Back to values").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.comparing = false;
                        cx.notify();
                    },
                )))
                .child(div().flex_1().min_h_0().child(self.comparison.as_ref().unwrap().clone()))
                .into_any_element();
        }
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
                                .aria_label("No keys match the filters")
                                .p_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No keys match the filters. Clear the search or turn off a source filter."),
                        )
                    }),
            )
            .child(detail)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{EditorKind, Filter, Layer, Source, ValueRow};
    use crate::value_editor::shaped;
    use profile_core::RawValue;

    fn row(key: &str, category: &str, source: Source) -> ValueRow {
        ValueRow {
            key: key.to_owned(),
            label: key.replace('_', " "),
            category: category.to_owned(),
            tooltip: String::new(),
            definition: String::new(),
            layers: vec![Layer { source, value: "1".to_owned() }],
            issues: Vec::new(),
            effective: RawValue::Str("1".into()),
            own: None,
            modified: false,
            editor: EditorKind::Text,
        }
    }

    #[test]
    fn filter_by_text_and_source() {
        let own = row("nozzle_temperature", "Temperature", Source::Chain(0, "self".into()));
        let inherited = row("filament_type", "Basic", Source::Chain(2, "base".into()));
        let default = row("filament_tower_interface_print_temp", "Other", Source::Default);
        let shown = |filter: &Filter| {
            [&own, &inherited, &default]
                .into_iter()
                .filter(|row| filter.matches(row))
                .map(|row| row.key.as_str())
                .collect::<Vec<_>>()
        };

        assert_eq!(shown(&Filter::default()).len(), 3);
        assert_eq!(shown(&Filter { query: "TEMP".into(), ..Default::default() }), [&own.key, &default.key]);
        assert_eq!(shown(&Filter { query: "basic".into(), ..Default::default() }), [&inherited.key]);
        assert_eq!(shown(&Filter { hide_defaults: true, ..Default::default() }), [&own.key, &inherited.key]);
        assert_eq!(shown(&Filter { only_this_file: true, ..Default::default() }), [&own.key]);
    }

    #[test]
    fn sources_order_from_the_preset_down() {
        assert!(Source::Chain(0, "z".into()) < Source::Chain(1, "a".into()));
        assert!(Source::Chain(5, "a".into()) < Source::Default);
        assert_eq!(Source::Chain(0, "x".into()).label(), "this file");
        assert_eq!(Source::Chain(2, "base".into()).label(), "[2] base");
    }

    #[test]
    fn edits_keep_the_shape_of_the_value() {
        let one = RawValue::List(vec!["220".into()]);
        assert_eq!(shaped(&one, vec!["230".into()]), RawValue::List(vec!["230".into()]));
        assert_eq!(shaped(&RawValue::Str("0.2".into()), vec!["0.3".into()]), RawValue::Str("0.3".into()));
        let two = shaped(&RawValue::Str("a".into()), vec!["a".into(), "b".into()]);
        assert_eq!(two, RawValue::List(vec!["a".into(), "b".into()]));
    }

    #[test]
    fn inherits_when_another_layer_sets_the_key() {
        let mut own_only = row("x", "Other", Source::Chain(0, "self".into()));
        assert!(!own_only.inherits());
        own_only.layers.push(Layer { source: Source::Default, value: "0".into() });
        assert!(own_only.inherits());
    }
}
