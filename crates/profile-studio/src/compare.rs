//! A read-only comparison of two presets, using their current edit sessions.

use std::sync::Arc;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableEvent, TableState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use profile_core::{KeyTrace, Origin, PresetId, RawValue};

use crate::catalog::Catalog;
use crate::edits::Edits;
use crate::values::{Layer, Source};

#[derive(Clone)]
struct PresetChoice {
    index: usize,
    title: SharedString,
}

impl SelectItem for PresetChoice {
    type Value = usize;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &usize {
        &self.index
    }
}

struct DiffRow {
    key: String,
    category: String,
    label: String,
    left: Vec<Layer>,
    right: Vec<Layer>,
    different: bool,
}

impl DiffRow {
    fn cell(&self, column: usize) -> String {
        let value = |layers: &[Layer]| layers.first().map_or("(missing)", |layer| layer.value.as_str()).to_owned();
        let source = |layers: &[Layer]| layers.first().map_or_else(|| "—".into(), |layer| layer.source.label());
        match column {
            0 => self.category.clone(),
            1 => self.label.clone(),
            2 => self.key.clone(),
            3 => value(&self.left),
            4 => value(&self.right),
            5 => source(&self.left),
            _ => source(&self.right),
        }
    }
}

/// Preserve array shape in the display because diff compares RawValue, not parsed numbers.
fn display_value(value: &RawValue) -> String {
    match value {
        RawValue::Str(value) if value.is_empty() => "\"\"".into(),
        RawValue::Str(value) => value.clone(),
        RawValue::List(items) => serde_json::to_string(items).expect("strings serialize"),
    }
}

fn build_rows(catalog: &Catalog, edits: &Edits, left: usize, right: usize) -> Vec<DiffRow> {
    let repo = &catalog.repo;
    let left_id = repo.presets()[left].id;
    let right_id = repo.presets()[right].id;
    let own = |index: usize| edits.session(index).map_or(&repo.presets()[index].params, |session| session.params());
    let layers = |id: PresetId, trace: Option<KeyTrace<'_>>, unit: &str| {
        let chain = repo.chain(id);
        trace
            .into_iter()
            .flat_map(|trace| trace.layers)
            .map(|layer| {
                let source = match layer.origin {
                    Origin::Default => Source::Default,
                    Origin::Preset(link) => Source::Chain(
                        chain.iter().position(|&entry| entry == link).expect("source belongs to chain"),
                        repo.preset(link).name().to_owned(),
                    ),
                };
                let mut value = display_value(layer.value);
                if !unit.is_empty() {
                    value.push(' ');
                    value.push_str(unit);
                }
                Layer { source, value }
            })
            .collect()
    };
    let mut rows: Vec<_> = repo
        .diff_with(left_id, own(left), right_id, own(right))
        .expect("the picker only offers presets of the same kind")
        .into_iter()
        .map(|diff| {
            let different = diff.is_different();
            let option = repo.schema().and_then(|schema| schema.option(diff.key));
            let unit = option.map_or("", |option| option.sidetext.as_str());
            DiffRow {
                key: diff.key.to_owned(),
                category: option
                    .map_or(
                        "Not in schema",
                        |option| {
                            if option.category.is_empty() { "Other" } else { &option.category }
                        },
                    )
                    .to_owned(),
                label: option.map_or(diff.key, |option| option.display_label()).to_owned(),
                left: layers(left_id, diff.left, unit),
                right: layers(right_id, diff.right, unit),
                different,
            }
        })
        .collect();
    rows.sort_by_key(|row| {
        (row.category == "Not in schema", row.category.clone(), row.label.to_lowercase(), row.key.clone())
    });
    rows
}

const COLUMNS: [(&str, &str, f32); 7] = [
    ("category", "Category", 130.),
    ("label", "Option", 190.),
    ("key", "Key", 200.),
    ("left", "Left value", 200.),
    ("right", "Right value", 200.),
    ("left-source", "Left source", 180.),
    ("right-source", "Right source", 180.),
];

#[derive(Default)]
pub struct DiffDelegate {
    rows: Vec<DiffRow>,
    visible: Vec<usize>,
    sort: Option<(usize, ColumnSort)>,
    /// Keep the selected key stable when filtering, sorting or rebuilding changes row indexes.
    selected: Option<String>,
}

impl DiffDelegate {
    fn row(&self, index: usize) -> Option<&DiffRow> {
        self.visible.get(index).map(|&index| &self.rows[index])
    }

    fn apply(&mut self, query: &str, only_differences: bool) {
        let query = query.trim().to_lowercase();
        self.visible = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                (!only_differences || row.different)
                    && [&row.key, &row.label, &row.category].iter().any(|text| text.to_lowercase().contains(&query))
            })
            .map(|(index, _)| index)
            .collect();
        self.sort_visible();
    }

    fn sort_visible(&mut self) {
        let rows = &self.rows;
        match self.sort {
            Some((column, sort @ (ColumnSort::Ascending | ColumnSort::Descending))) => {
                self.visible.sort_by(|&a, &b| {
                    let order = rows[a]
                        .cell(column)
                        .to_lowercase()
                        .cmp(&rows[b].cell(column).to_lowercase())
                        .then_with(|| a.cmp(&b));
                    if sort == ColumnSort::Descending { order.reverse() } else { order }
                });
            }
            _ => self.visible.sort(),
        }
    }
}

fn restore_selection(table: &mut TableState<DiffDelegate>, cx: &mut Context<TableState<DiffDelegate>>) {
    let delegate = table.delegate();
    let selected = delegate
        .selected
        .as_ref()
        .and_then(|key| delegate.visible.iter().position(|&index| delegate.rows[index].key == *key));
    if let Some(index) = selected {
        table.set_selected_row(index, cx);
    } else {
        table.clear_selection(cx);
    }
}

impl TableDelegate for DiffDelegate {
    fn columns_count(&self, _: &App) -> usize {
        COLUMNS.len()
    }
    fn rows_count(&self, _: &App) -> usize {
        self.visible.len()
    }
    fn column(&self, index: usize, _: &App) -> Column {
        let (key, label, width) = COLUMNS[index];
        Column::new(key, label).width(px(width)).sortable()
    }
    fn perform_sort(
        &mut self,
        column: usize,
        sort: ColumnSort,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.sort = Some((column, sort));
        self.sort_visible();
        // The delegate is borrowed by TableState here; update the table after it is released.
        cx.defer_in(window, |table, _, cx| restore_selection(table, cx));
    }
    fn render_td(
        &mut self,
        row: usize,
        column: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(row) = self.row(row) else { return div() };
        div()
            .min_w_0()
            .truncate()
            .when(row.different && matches!(column, 3 | 4), |cell| cell.text_color(cx.theme().warning).font_semibold())
            .child(row.cell(column).replace('\n', " ⏎ "))
    }
    fn cell_text(&self, row: usize, column: usize, _: &App) -> String {
        self.row(row).map_or_else(String::new, |row| row.cell(column))
    }
}

pub struct CompareView {
    catalog: Arc<Catalog>,
    edits: Entity<Edits>,
    left: usize,
    right: Option<usize>,
    picker: Entity<SelectState<SearchableVec<PresetChoice>>>,
    search: Entity<InputState>,
    table: Entity<TableState<DiffDelegate>>,
    only_differences: bool,
    _subscriptions: Vec<Subscription>,
}

impl CompareView {
    pub fn new(
        catalog: &Arc<Catalog>,
        edits: &Entity<Edits>,
        left: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let repo = &catalog.repo;
        let mut choices: Vec<_> = repo
            .presets()
            .iter()
            .filter(|preset| preset.kind == repo.presets()[left].kind && preset.id.index() != left)
            .map(|preset| PresetChoice {
                index: preset.id.index(),
                title: format!("{} / {} — {}", repo.vendor(preset.vendor).name, preset.name(), preset.path.display())
                    .into(),
            })
            .collect();
        choices.sort_by(|a, b| a.title.cmp(&b.title));
        let picker = cx.new(|cx| SelectState::new(SearchableVec::new(choices), None, window, cx).searchable(true));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Filter by key, option or category"));
        let table = cx.new(|cx| TableState::new(DiffDelegate::default(), window, cx).row_selectable(true));
        let subscriptions = vec![
            cx.subscribe_in(&picker, window, |this, _, event: &SelectEvent<SearchableVec<PresetChoice>>, _, cx| {
                let SelectEvent::Confirm(right) = event;
                this.right = *right;
                this.table.update(cx, |table, _| table.delegate_mut().selected = None);
                this.rebuild(cx);
            }),
            cx.subscribe_in(&search, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.apply_filter(cx);
                }
            }),
            cx.subscribe_in(&table, window, |_, table, event: &TableEvent, _, cx| {
                if let TableEvent::SelectRow(index) = event {
                    table.update(cx, |table, _| {
                        table.delegate_mut().selected = table.delegate().row(*index).map(|row| row.key.clone());
                    });
                    cx.notify();
                }
            }),
            cx.observe(edits, |this, _, cx| this.rebuild(cx)),
        ];
        Self {
            catalog: catalog.clone(),
            edits: edits.clone(),
            left,
            right: None,
            picker,
            search,
            table,
            only_differences: true,
            _subscriptions: subscriptions,
        }
    }

    #[cfg(test)]
    pub fn table(&self) -> &Entity<TableState<DiffDelegate>> {
        &self.table
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let rows =
            self.right.map_or_else(Vec::new, |right| build_rows(&self.catalog, self.edits.read(cx), self.left, right));
        self.table.update(cx, |table, _| table.delegate_mut().rows = rows);
        self.apply_filter(cx);
    }

    fn apply_filter(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().to_string();
        self.table.update(cx, |table, cx| {
            table.delegate_mut().apply(&query, self.only_differences);
            restore_selection(table, cx);
            table.refresh(cx);
        });
        cx.notify();
    }

    fn side_title(&self, side: &str, index: usize, cx: &App) -> String {
        let preset = &self.catalog.repo.presets()[index];
        let dirty = if self.edits.read(cx).is_dirty(index) { " (unsaved changes)" } else { "" };
        format!("{side}: {} / {}{dirty}", self.catalog.repo.vendor(preset.vendor).name, preset.name())
    }

    fn render_detail(&self, cx: &App) -> AnyElement {
        let table = self.table.read(cx);
        let row =
            table.delegate().selected.as_ref().and_then(|key| table.delegate().rows.iter().find(|row| row.key == *key));
        let Some(row) = row else {
            return div().p_3().text_sm().child("Select a key to inspect both inheritance chains.").into_any_element();
        };
        let side = |name: &'static str, layers: &[Layer]| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(div().font_semibold().child(name))
                .when(layers.is_empty(), |panel| panel.child("(missing)"))
                .children(layers.iter().enumerate().map(|(index, layer)| {
                    let text = format!("{}: {}", layer.source.label(), layer.value);
                    div()
                        .id((name, index))
                        .test_support()
                        .aria_label(text.clone())
                        .when(index > 0, |line| line.text_color(cx.theme().muted_foreground))
                        .child(text)
                }))
        };
        v_flex()
            .id("compare-detail")
            .test_support()
            .aria_label(row.key.clone())
            .h_56()
            .flex_shrink_0()
            .overflow_y_scroll()
            .p_3()
            .gap_2()
            .text_sm()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(div().font_semibold().child(format!(
                "{} · {}",
                row.key,
                if row.different { "Different values" } else { "Equal values" }
            )))
            .child(h_flex().items_start().gap_4().child(side("Left", &row.left)).child(side("Right", &row.right)))
            .into_any_element()
    }
}

impl Render for CompareView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let delegate = self.table.read(cx).delegate();
        let count = format!(
            "{} shown · {} differences · {} keys",
            delegate.visible.len(),
            delegate.rows.iter().filter(|row| row.different).count(),
            delegate.rows.len()
        );
        let empty = delegate.visible.is_empty();
        let empty_message = if self.right.is_none() {
            "Choose a preset above to start comparing."
        } else if self.search.read(cx).value().trim().is_empty() && self.only_differences {
            "No differing values. Turn off Only differences to inspect their sources."
        } else {
            "No keys match the current filters."
        };
        let picker = Select::new(&self.picker)
            .id("compare-picker")
            .accessibility_label("Compare with preset")
            .placeholder("Choose a preset of the same kind…")
            .search_placeholder("Search vendor or preset")
            .w_full()
            .menu_width(rems(37.5));
        let filters = h_flex()
            .gap_3()
            .flex_wrap()
            .child(
                Input::new(&self.search)
                    .id("compare-search")
                    .aria_label("Filter comparison")
                    .small()
                    .cleanable(true)
                    .w_64(),
            )
            .child(Checkbox::new("only-differences").label("Only differences").checked(self.only_differences).on_click(
                cx.listener(|this, checked: &bool, _, cx| {
                    this.only_differences = *checked;
                    this.apply_filter(cx);
                }),
            ))
            .child(div().id("compare-count").test_support().aria_label(count.clone()).text_sm().child(count));
        const NOTE: &str = concat!(
            "Read-only · Includes unsaved changes. Reload to update inherited values. ",
            "String/array differences are included."
        );
        let left_title = self.side_title("Left", self.left, cx);
        let right_title = self.right.map(|right| self.side_title("Right", right, cx));
        v_flex()
            .size_full()
            .gap_2()
            .child(
                div()
                    .id("compare-left-title")
                    .test_support()
                    .aria_label(left_title.clone())
                    .text_sm()
                    .child(left_title),
            )
            // Select's root fills its parent; constrain its height so the table can use the rest.
            .child(div().h_8().flex_shrink_0().child(picker))
            .when_some(right_title, |view, title| {
                view.child(
                    div().id("compare-right-title").test_support().aria_label(title.clone()).text_sm().child(title),
                )
            })
            .child(filters)
            .child(div().text_xs().text_color(cx.theme().muted_foreground).child(NOTE))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(!empty, |body| body.child(DataTable::new(&self.table).stripe(true)))
                    .when(empty, |body| {
                        body.child(
                            div()
                                .id("compare-empty")
                                .test_support()
                                .aria_label(empty_message)
                                .p_3()
                                .text_sm()
                                .child(empty_message),
                        )
                    }),
            )
            .child(self.render_detail(cx))
    }
}
