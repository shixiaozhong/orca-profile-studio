//! A read-only comparison of up to eight presets of one kind, using their current edit sessions.
//! The first preset is the base: cells that differ from it are highlighted.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
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

/// The most presets one comparison shows.
pub const MAX_PRESETS: usize = 8;

/// Where the comparison was opened from; "Back" returns there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOrigin {
    Values,
    Batch,
    Presets,
}

pub enum CompareEvent {
    Close,
}

impl EventEmitter<CompareEvent> for CompareView {}

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
    /// Every layer of each compared preset, in column order.
    cells: Vec<Vec<Layer>>,
    /// Whether each column's effective value differs from the first column's.
    differs: Vec<bool>,
    different: bool,
}

impl DiffRow {
    fn cell(&self, column: usize) -> String {
        if column == 0 {
            return self.key.clone();
        }
        self.cells[column - 1].first().map_or_else(|| t!("common.missing"), |layer| layer.value.clone())
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

fn build_rows(catalog: &Catalog, edits: &Edits, presets: &[usize]) -> Vec<DiffRow> {
    let repo = &catalog.repo;
    let own = |index: usize| edits.session(index).map_or(&repo.presets()[index].params, |session| session.params());
    let layers = |id: PresetId, trace: Option<KeyTrace<'_>>| {
        let chain = repo.chain(id);
        trace
            .into_iter()
            .flat_map(|trace| trace.layers)
            .map(|layer| {
                let Origin::Preset(link) = layer.origin;
                let depth = chain.iter().position(|&entry| entry == link).expect("source belongs to chain");
                let source = Source { depth, name: repo.preset(link).name().to_owned() };
                Layer { source, value: display_value(layer.value) }
            })
            .collect()
    };
    let ids: Vec<_> = presets.iter().map(|&index| (repo.presets()[index].id, own(index))).collect();
    let mut rows: Vec<_> = repo
        .diff_with(&ids)
        .expect("comparisons only hold presets of one kind")
        .into_iter()
        .map(|diff| DiffRow {
            key: diff.key.to_owned(),
            differs: (0..ids.len()).map(|column| diff.differs_from_first(column)).collect(),
            different: diff.is_different(),
            cells: diff.traces.into_iter().zip(&ids).map(|(trace, &(id, _))| layers(id, trace)).collect(),
        })
        .collect();
    rows.sort_by(|a, b| a.key.cmp(&b.key));
    rows
}

#[derive(Default)]
pub struct DiffDelegate {
    /// Column headers after the key: the preset names, with the vendor when a name repeats.
    headers: Vec<String>,
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
            .filter(|(_, row)| (!only_differences || row.different) && row.key.to_lowercase().contains(&query))
            .map(|(index, _)| index)
            .collect();
        self.sort_visible();
    }

    fn sort_visible(&mut self) {
        let rows = &self.rows;
        match self.sort {
            Some((column, sort @ (ColumnSort::Ascending | ColumnSort::Descending))) if column <= self.headers.len() => {
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
        1 + self.headers.len()
    }
    fn rows_count(&self, _: &App) -> usize {
        self.visible.len()
    }
    fn column(&self, index: usize, _: &App) -> Column {
        match index {
            0 => Column::new("key", t!("common.key")).width(px(240.)).sortable(),
            index => Column::new(SharedString::from(format!("preset-{index}")), self.headers[index - 1].clone())
                .width(px(220.))
                .sortable(),
        }
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
        let differs = column > 0 && row.differs[column - 1];
        div()
            .min_w_0()
            .truncate()
            .when(differs, |cell| cell.text_color(cx.theme().warning).font_semibold())
            .child(row.cell(column).replace('\n', " ⏎ "))
    }
    fn cell_text(&self, row: usize, column: usize, _: &App) -> String {
        self.row(row).map_or_else(String::new, |row| row.cell(column))
    }
}

pub struct CompareView {
    catalog: Arc<Catalog>,
    edits: Entity<Edits>,
    /// Compared presets in column order; the first is the base.
    presets: Vec<usize>,
    origin: CompareOrigin,
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
        presets: Vec<usize>,
        origin: CompareOrigin,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picker = cx.new(|cx| SelectState::new(SearchableVec::new(Vec::new()), None, window, cx).searchable(true));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("values.filter")));
        let table = cx.new(|cx| TableState::new(DiffDelegate::default(), window, cx).row_selectable(true));
        let subscriptions = vec![
            cx.subscribe_in(
                &picker,
                window,
                |this, _, event: &SelectEvent<SearchableVec<PresetChoice>>, window, cx| {
                    let SelectEvent::Confirm(Some(preset)) = event else { return };
                    if this.presets.len() < MAX_PRESETS && !this.presets.contains(preset) {
                        this.presets.push(*preset);
                    }
                    this.sync(window, cx);
                },
            ),
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
        let mut view = Self {
            catalog: catalog.clone(),
            edits: edits.clone(),
            presets,
            origin,
            picker,
            search,
            table,
            only_differences: true,
            _subscriptions: subscriptions,
        };
        view.sync(window, cx);
        view
    }

    pub fn presets(&self) -> &[usize] {
        &self.presets
    }

    /// Shows `presets` (at most [`MAX_PRESETS`] of one kind), keeping the filters.
    pub fn open(&mut self, presets: Vec<usize>, origin: CompareOrigin, window: &mut Window, cx: &mut Context<Self>) {
        self.origin = origin;
        if presets != self.presets {
            self.presets = presets;
            self.table.update(cx, |table, _| table.delegate_mut().selected = None);
        }
        self.sync(window, cx);
    }

    /// Maps the presets into a new load by file path, dropping those that no longer exist or
    /// changed kind, and keeps the filters and the selected key.
    pub fn refresh(&mut self, catalog: &Arc<Catalog>, window: &mut Window, cx: &mut Context<Self>) {
        let kind = self.presets.first().map(|&index| self.catalog.repo.presets()[index].kind);
        self.presets = self
            .presets
            .iter()
            .filter_map(|&index| catalog.remap_preset(&self.catalog, index))
            .filter(|&index| Some(catalog.repo.presets()[index].kind) == kind)
            .collect();
        self.catalog = catalog.clone();
        self.sync(window, cx);
    }

    /// Updates the translated placeholders, choices and headers.
    pub fn refresh_language(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync(window, cx);
    }

    #[cfg(test)]
    pub fn table(&self) -> &Entity<TableState<DiffDelegate>> {
        &self.table
    }

    fn remove(&mut self, preset: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.presets.retain(|&index| index != preset);
        self.sync(window, cx);
    }

    /// Rebuilds the choices of presets to add and the table after the presets change.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let repo = &self.catalog.repo;
        let kind = self.presets.first().map(|&index| repo.presets()[index].kind);
        let mut choices: Vec<_> = repo
            .presets()
            .iter()
            .filter(|preset| Some(preset.kind) == kind && !self.presets.contains(&preset.id.index()))
            .map(|preset| PresetChoice {
                index: preset.id.index(),
                title: format!("{} / {} — {}", repo.vendor(preset.vendor).name, preset.name(), preset.path.display())
                    .into(),
            })
            .collect();
        choices.sort_by(|a, b| a.title.cmp(&b.title));
        self.picker.update(cx, |picker, cx| {
            picker.set_items(SearchableVec::new(choices), window, cx);
            picker.set_selected_index(None, window, cx);
        });
        self.search.update(cx, |state, cx| state.set_placeholder(t!("values.filter"), window, cx));
        self.rebuild(cx);
    }

    fn headers(&self) -> Vec<String> {
        let repo = &self.catalog.repo;
        let name = |index: usize| repo.presets()[index].name();
        self.presets
            .iter()
            .map(|&index| {
                let preset = &repo.presets()[index];
                if self.presets.iter().filter(|&&other| name(other) == preset.name()).count() > 1 {
                    format!("{} ({})", preset.name(), repo.vendor(preset.vendor).name)
                } else {
                    preset.name().to_owned()
                }
            })
            .collect()
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let rows = if self.presets.len() < 2 {
            Vec::new()
        } else {
            build_rows(&self.catalog, self.edits.read(cx), &self.presets)
        };
        let headers = self.headers();
        self.table.update(cx, |table, _| {
            let delegate = table.delegate_mut();
            delegate.rows = rows;
            delegate.headers = headers;
        });
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

    /// The compared presets, the first marked as the base, each removable.
    fn render_presets(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let repo = &self.catalog.repo;
        let edits = self.edits.read(cx);
        let muted = cx.theme().muted_foreground;
        let items: Vec<_> = self
            .presets
            .iter()
            .enumerate()
            .map(|(column, &index)| {
                let preset = &repo.presets()[index];
                let mut text = format!("{}. {} / {}", column + 1, repo.vendor(preset.vendor).name, preset.name());
                if edits.is_dirty(index) {
                    text.push_str(&t!("compare.unsaved"));
                }
                let name = preset.name().to_owned();
                h_flex()
                    .gap_1()
                    .min_w_0()
                    .text_sm()
                    .child(
                        div()
                            .id(("compare-preset", index))
                            .test_support()
                            .aria_label(text.clone())
                            .min_w_0()
                            .truncate()
                            .child(text),
                    )
                    .when(column == 0, |item| item.child(div().text_color(muted).child(t!("compare.base"))))
                    .child(
                        Button::new(("compare-remove", index))
                            .ghost()
                            .xsmall()
                            .icon(IconName::X)
                            .accessibility_label(t!("compare.remove", name = name))
                            .tooltip(t!("compare.remove", name = name))
                            .on_click(cx.listener(move |this, _, window, cx| this.remove(index, window, cx))),
                    )
            })
            .collect();
        let full = self.presets.len() >= MAX_PRESETS;
        let picker = Select::new(&self.picker)
            .id("compare-picker")
            .accessibility_label(t!("compare.preset"))
            .placeholder(if full { t!("compare.full", arg0 = MAX_PRESETS) } else { t!("compare.choose") })
            .search_placeholder(t!("compare.search"))
            .disabled(full || self.presets.is_empty())
            .small()
            .w_full()
            .menu_width(rems(37.5));
        v_flex()
            .gap_1()
            .flex_shrink_0()
            .child(v_flex().id("compare-presets").max_h_32().overflow_y_scroll().children(items))
            // Select's root fills its parent; constrain its height so the table can use the rest.
            .child(div().h_8().max_w(rems(37.5)).child(picker))
    }

    fn render_detail(&self, cx: &App) -> AnyElement {
        let table = self.table.read(cx);
        let delegate = table.delegate();
        let row = delegate.selected.as_ref().and_then(|key| delegate.rows.iter().find(|row| row.key == *key));
        let Some(row) = row else {
            return div().p_3().text_sm().child(t!("compare.select_hint")).into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let panels = row.cells.iter().enumerate().map(|(column, layers)| {
            let header = delegate.headers[column].clone();
            let id = SharedString::from(format!("layers-{column}"));
            v_flex()
                .w_64()
                .flex_shrink_0()
                .gap_1()
                .child(
                    div()
                        .font_semibold()
                        .truncate()
                        .when(row.differs[column], |title| title.text_color(cx.theme().warning))
                        .child(header),
                )
                .when(layers.is_empty(), |panel| panel.child(t!("common.missing")))
                .children(layers.iter().enumerate().map(|(index, layer)| {
                    let text = format!("{}: {}", layer.source.label(), layer.value);
                    div()
                        .id((id.clone(), index))
                        .test_support()
                        .aria_label(text.clone())
                        .when(index > 0, |line| line.text_color(muted))
                        .child(text)
                }))
        });
        v_flex()
            .id("compare-detail")
            .test_support()
            .aria_label(row.key.clone())
            .h_56()
            .max_h(relative(0.4))
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
                if row.different { t!("compare.different") } else { t!("compare.equal") }
            )))
            .child(h_flex().id("compare-layers").items_start().gap_4().overflow_x_scroll().children(panels))
            .into_any_element()
    }
}

impl Render for CompareView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let delegate = self.table.read(cx).delegate();
        let count = t!(
            "compare.count",
            arg0 = delegate.visible.len(),
            arg1 = delegate.rows.iter().filter(|row| row.different).count(),
            arg2 = delegate.rows.len()
        );
        let empty = delegate.visible.is_empty();
        let empty_message = if self.presets.len() < 2 {
            t!("compare.start")
        } else if self.search.read(cx).value().trim().is_empty() && self.only_differences {
            t!("compare.no_differences")
        } else {
            t!("compare.empty")
        };
        let back = match self.origin {
            CompareOrigin::Values => t!("common.back_values"),
            CompareOrigin::Batch => t!("compare.back_batch"),
            CompareOrigin::Presets => t!("compare.back_presets"),
        };
        let filters = h_flex()
            .gap_3()
            .flex_wrap()
            .flex_shrink_0()
            .child(
                Input::new(&self.search)
                    .id("compare-search")
                    .aria_label(t!("compare.filter"))
                    .small()
                    .cleanable(true)
                    .w_64(),
            )
            .child(
                Checkbox::new("only-differences")
                    .label(t!("compare.only_differences"))
                    .checked(self.only_differences)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.only_differences = *checked;
                        this.apply_filter(cx);
                    })),
            )
            .child(div().id("compare-count").test_support().aria_label(count.clone()).text_sm().child(count));
        v_flex()
            .id("compare-view")
            .size_full()
            .p_3()
            .gap_2()
            .child(
                h_flex()
                    .gap_3()
                    .flex_shrink_0()
                    .child(
                        Button::new("close-comparison")
                            .small()
                            .label(back)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(CompareEvent::Close))),
                    )
                    .child(div().font_semibold().child(t!("compare.title"))),
            )
            .child(self.render_presets(cx))
            .child(filters)
            .child(div().text_xs().text_color(cx.theme().muted_foreground).child(t!("compare.note")))
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
                                .aria_label(empty_message.clone())
                                .p_3()
                                .text_sm()
                                .child(empty_message),
                        )
                    }),
            )
            .child(self.render_detail(cx))
    }
}
