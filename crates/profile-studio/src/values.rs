//! The "Values" tab of a preset: every effective key, the layer of the inheritance chain it
//! comes from, and the values it overrides.

use std::cmp::Ordering;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableEvent, TableState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use profile_core::{Origin, Severity};

use crate::catalog::Catalog;
use crate::details::{format_value, severity_color};

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
}

/// Builds the rows of a preset, sorted by category and label.
pub fn build_rows(catalog: &Catalog, preset: usize) -> Vec<ValueRow> {
    let repo = &catalog.repo;
    let id = repo.presets()[preset].id;
    let chain = repo.chain(id);
    let schema = repo.schema();
    let issues: Vec<_> = catalog.issues_of(preset).collect();

    let mut rows: Vec<ValueRow> = repo
        .trace(id)
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
                    let mut value = format_value(layer.value);
                    if !unit.is_empty() {
                        value = format!("{value} {unit}");
                    }
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
            let key_issues = issues
                .iter()
                .filter(|issue| issue.kind.key() == Some(trace.key))
                .map(|issue| (issue.severity(), issue.kind.to_string()))
                .collect();
            ValueRow { key: trace.key.to_owned(), label, category, tooltip, definition, layers, issues: key_issues }
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
        if self.only_this_file && !matches!(row.source(), Source::Chain(0, _)) {
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
        _ => row.source().label(),
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
                .when_some(row.severity().filter(|&severity| severity >= Severity::Warning), |cell, severity| {
                    let color = severity_color(severity, cx).unwrap_or(muted);
                    cell.child(Icon::new(IconName::TriangleAlert).xsmall().text_color(color))
                })
                .child(text)
                .into_any_element(),
            0 | 2 => text.text_color(muted).into_any_element(),
            4 if *row.source() == Source::Default => text.text_color(muted).into_any_element(),
            _ => text.into_any_element(),
        }
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        self.row(row_ix).map(|row| cell_text(row, col_ix)).unwrap_or_default()
    }
}

pub struct ValuesView {
    preset: usize,
    search: Entity<InputState>,
    table: Entity<TableState<ValuesDelegate>>,
    filter: Filter,
    /// Key of the selected row; survives filtering.
    selected: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl ValuesView {
    pub fn new(catalog: &Arc<Catalog>, preset: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rows = build_rows(catalog, preset);
        let table = cx.new(|cx| TableState::new(ValuesDelegate::new(rows), window, cx).row_selectable(true));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Filter by key, option or category"));
        let subscriptions = vec![
            cx.subscribe_in(&search, window, |this, search, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.filter.query = search.read(cx).value().to_string();
                    this.apply_filter(cx);
                }
            }),
            cx.subscribe_in(&table, window, |this, table, event: &TableEvent, _, cx| {
                if let TableEvent::SelectRow(row_ix) = event {
                    this.selected = table.read(cx).delegate().row(*row_ix).map(|row| row.key.clone());
                    cx.notify();
                }
            }),
        ];
        ValuesView { preset, search, table, filter: Filter::default(), selected: None, _subscriptions: subscriptions }
    }

    pub fn preset(&self) -> usize {
        self.preset
    }

    #[cfg(test)]
    pub fn table(&self) -> &Entity<TableState<ValuesDelegate>> {
        &self.table
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

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let table = self.table.read(cx);
        let shown = table.delegate().visible.len();
        let total = table.delegate().rows.len();
        let count = SharedString::from(format!("{shown} of {total} keys"));
        h_flex()
            .gap_3()
            .pb_2()
            .items_center()
            .child(
                Input::new(&self.search)
                    .id("values-search")
                    .aria_label("Filter values")
                    .small()
                    .prefix(Icon::new(IconName::Search).small())
                    .cleanable(true)
                    .w(px(280.)),
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
            .child(div().flex_1())
            .child(
                div()
                    .id("values-count")
                    .test_support()
                    .aria_label(count.clone())
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(count),
            )
    }

    fn render_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let panel = v_flex()
            .id("value-detail")
            .test_support()
            .h(px(220.))
            .flex_shrink_0()
            .mt_2()
            .pt_2()
            .gap_1()
            .border_t_1()
            .border_color(cx.theme().border)
            .overflow_y_scroll()
            .text_sm();
        let table = self.table.read(cx);
        let row = self.selected.as_ref().and_then(|key| table.delegate().rows.iter().find(|row| &row.key == key));
        let Some(row) = row else {
            let hint = "Select a row to see every layer of the inheritance chain that sets it.";
            return panel.aria_label("").child(div().text_color(muted).child(hint));
        };
        panel
            .aria_label(row.key.clone())
            .child(
                h_flex()
                    .gap_2()
                    .child(div().font_semibold().child(row.label.clone()))
                    .child(div().text_color(muted).child(row.key.clone())),
            )
            .when(!row.tooltip.is_empty(), |panel| panel.child(div().text_color(muted).child(row.tooltip.clone())))
            .child(div().text_xs().text_color(muted).child(row.definition.clone()))
            .child(div().pt_1().text_xs().font_semibold().text_color(muted).child("LAYERS (NEAREST FIRST)"))
            .children(row.layers.iter().enumerate().map(|(ix, layer)| {
                let source = layer.source.label();
                h_flex()
                    .id(("layer", ix))
                    .test_support()
                    .aria_label(SharedString::from(format!("{source}: {}", layer.value)))
                    .gap_3()
                    .items_start()
                    .when(ix > 0, |line| line.text_color(muted).line_through())
                    .child(div().w(px(240.)).flex_shrink_0().truncate().child(source))
                    .child(div().flex_1().min_w_0().child(truncate_chars(&layer.value, 2000)))
            }))
            .when(!row.issues.is_empty(), |panel| {
                panel.child(div().pt_1().text_xs().font_semibold().text_color(muted).child("ISSUES")).children(
                    row.issues.iter().map(|(severity, message)| {
                        let color = severity_color(*severity, cx).unwrap_or(muted);
                        h_flex()
                            .gap_2()
                            .child(Icon::new(IconName::TriangleAlert).small().text_color(color))
                            .child(message.clone())
                    }),
                )
            })
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
        v_flex()
            .size_full()
            .child(self.render_toolbar(cx))
            .child(div().flex_1().min_h_0().child(DataTable::new(&self.table).stripe(true)))
            .child(self.render_detail(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::{Filter, Layer, Source, ValueRow};

    fn row(key: &str, category: &str, source: Source) -> ValueRow {
        ValueRow {
            key: key.to_owned(),
            label: key.replace('_', " "),
            category: category.to_owned(),
            tooltip: String::new(),
            definition: String::new(),
            layers: vec![Layer { source, value: "1".to_owned() }],
            issues: Vec::new(),
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
}
