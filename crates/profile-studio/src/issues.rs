//! The issues panel: every validation issue of the loaded profiles, filterable by severity, code,
//! vendor and text. Selecting a row asks the app to reveal the issue's preset and key.

use std::collections::BTreeMap;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::table::{Column, ColumnSort, DataTable, TableDelegate, TableEvent, TableState};
use gpui_kit::component::{ActiveTheme as _, Icon, IndexPath, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use profile_core::{Issue, Severity};

use crate::catalog::Catalog;
use crate::details::severity_color;

pub enum IssuesEvent {
    /// Show the issue with this index into `Repository::issues`.
    Reveal(usize),
    Close,
}

/// A filter choice: the value it selects and its label.
#[derive(Clone)]
struct Choice<T> {
    value: T,
    label: SharedString,
}

impl<T: Clone + PartialEq + 'static> SelectItem for Choice<T> {
    type Value = T;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &T {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueFilter {
    pub min_severity: Severity,
    pub code: Option<&'static str>,
    /// Vendor index.
    pub vendor: Option<usize>,
    /// Case-insensitive text matched against code, vendor, path and message.
    pub query: String,
}

impl Default for IssueFilter {
    fn default() -> Self {
        // Info issues (obsolete keys and the like) outnumber everything else; start without them.
        IssueFilter { min_severity: Severity::Warning, code: None, vendor: None, query: String::new() }
    }
}

impl IssueFilter {
    pub fn matches(&self, catalog: &Catalog, issue: &Issue) -> bool {
        if issue.severity() < self.min_severity
            || self.code.is_some_and(|code| code != issue.kind.code())
            || self.vendor.is_some_and(|vendor| vendor != issue.vendor.index())
        {
            return false;
        }
        let query = self.query.trim().to_lowercase();
        query.is_empty() || {
            let vendor = &catalog.repo.vendors()[issue.vendor.index()].name;
            let text = format!("{}\n{vendor}\n{}\n{}", issue.kind.code(), issue.path.display(), issue.kind);
            query.split_whitespace().all(|term| text.to_lowercase().contains(term))
        }
    }
}

const COLUMNS: [(&str, &str, f32); 5] = [
    ("severity", "Severity", 96.),
    ("code", "Code", 200.),
    ("vendor", "Vendor", 140.),
    ("file", "File", 320.),
    ("message", "Message", 560.),
];

pub struct IssuesDelegate {
    catalog: Arc<Catalog>,
    /// Indexes into `Repository::issues` that pass the filter, in display order.
    visible: Vec<usize>,
    sort: Option<(usize, ColumnSort)>,
}

impl IssuesDelegate {
    fn issue(&self, row_ix: usize) -> Option<&Issue> {
        self.visible.get(row_ix).map(|&index| &self.catalog.repo.issues()[index])
    }

    fn cell_text(&self, issue: &Issue, col_ix: usize) -> String {
        match col_ix {
            0 => issue.severity().to_string(),
            1 => issue.kind.code().to_owned(),
            2 => self.catalog.repo.vendors()[issue.vendor.index()].name.clone(),
            3 => issue.path.display().to_string(),
            _ => issue.kind.to_string(),
        }
    }

    fn apply(&mut self, filter: &IssueFilter) {
        let issues = self.catalog.repo.issues();
        self.visible = (0..issues.len()).filter(|&index| filter.matches(&self.catalog, &issues[index])).collect();
        self.sort_visible();
    }

    fn sort_visible(&mut self) {
        let issues = self.catalog.repo.issues();
        match self.sort {
            Some((col_ix, sort @ (ColumnSort::Ascending | ColumnSort::Descending))) => {
                let key = |index: usize| match col_ix {
                    // Severity sorts by rank, not alphabetically.
                    0 => format!("{}", issues[index].severity() as u8),
                    _ => self.cell_text(&issues[index], col_ix).to_lowercase(),
                };
                let mut keyed: Vec<(String, usize)> = self.visible.iter().map(|&index| (key(index), index)).collect();
                keyed.sort();
                if sort == ColumnSort::Descending {
                    keyed.reverse();
                }
                self.visible = keyed.into_iter().map(|(_, index)| index).collect();
            }
            // Most severe first, then by code and file.
            _ => self.visible.sort_by_key(|&index| {
                let issue = &issues[index];
                (std::cmp::Reverse(issue.severity()), issue.kind.code(), issue.path.clone(), index)
            }),
        }
    }
}

impl TableDelegate for IssuesDelegate {
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
        let Some(issue) = self.issue(row_ix) else { return div().into_any_element() };
        let text = div().min_w_0().truncate().child(self.cell_text(issue, col_ix));
        let muted = cx.theme().muted_foreground;
        match col_ix {
            // Severity is shown by an icon and its name, never by color alone.
            0 => h_flex()
                .gap_1()
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .xsmall()
                        .text_color(severity_color(issue.severity(), cx).unwrap_or(muted)),
                )
                .child(text)
                .into_any_element(),
            1..=3 => text.text_color(muted).into_any_element(),
            _ => text.into_any_element(),
        }
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        self.issue(row_ix).map(|issue| self.cell_text(issue, col_ix)).unwrap_or_default()
    }
}

pub struct IssuesView {
    catalog: Arc<Catalog>,
    severity: Entity<SelectState<SearchableVec<Choice<Severity>>>>,
    code: Entity<SelectState<SearchableVec<Choice<Option<&'static str>>>>>,
    vendor: Entity<SelectState<SearchableVec<Choice<Option<usize>>>>>,
    search: Entity<InputState>,
    table: Entity<TableState<IssuesDelegate>>,
    filter: IssueFilter,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<IssuesEvent> for IssuesView {}

impl IssuesView {
    pub fn refresh(&mut self, catalog: &Arc<Catalog>, window: &mut Window, cx: &mut Context<Self>) {
        let mut filter = self.filter.clone();
        filter.vendor = filter.vendor.and_then(|index| {
            catalog.repo.vendors().iter().position(|v| v.index_path == self.catalog.repo.vendors()[index].index_path)
        });
        if filter.code.is_some_and(|code| !catalog.repo.issues().iter().any(|issue| issue.kind.code() == code)) {
            filter.code = None;
        }
        let sort = self.table.read(cx).delegate().sort;
        *self = Self::new(catalog, window, cx);
        self.filter = filter;
        self.search.update(cx, |search, cx| search.set_value(self.filter.query.clone(), window, cx));
        self.severity.update(cx, |select, cx| select.set_selected_value(&self.filter.min_severity, window, cx));
        self.code.update(cx, |select, cx| select.set_selected_value(&self.filter.code, window, cx));
        self.vendor.update(cx, |select, cx| select.set_selected_value(&self.filter.vendor, window, cx));
        self.table.update(cx, |table, _| table.delegate_mut().sort = sort);
        self.apply_filter(cx);
    }

    pub fn new(catalog: &Arc<Catalog>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let repo = &catalog.repo;
        let filter = IssueFilter::default();
        let mut codes: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut vendors: BTreeMap<usize, usize> = BTreeMap::new();
        for issue in repo.issues() {
            *codes.entry(issue.kind.code()).or_default() += 1;
            *vendors.entry(issue.vendor.index()).or_default() += 1;
        }

        let severities = [
            (Severity::Info, "All severities"),
            (Severity::Warning, "Warnings and errors"),
            (Severity::Error, "Errors only"),
            (Severity::Fatal, "Fatal only"),
        ];
        let severity_choices: Vec<_> =
            severities.iter().map(|&(value, label)| Choice { value, label: label.into() }).collect();
        let selected = severities.iter().position(|&(value, _)| value == filter.min_severity).map(IndexPath::new);
        let severity = cx.new(|cx| SelectState::new(SearchableVec::new(severity_choices), selected, window, cx));

        let mut code_choices = vec![Choice { value: None, label: "All codes".into() }];
        code_choices.extend(
            codes.iter().map(|(&code, count)| Choice { value: Some(code), label: format!("{code} ({count})").into() }),
        );
        let code = cx.new(|cx| {
            SelectState::new(SearchableVec::new(code_choices), Some(IndexPath::new(0)), window, cx).searchable(true)
        });

        let mut vendor_choices = vec![Choice { value: None, label: "All vendors".into() }];
        let mut by_name: Vec<_> =
            vendors.iter().map(|(&vendor, &count)| (&repo.vendors()[vendor].name, vendor, count)).collect();
        by_name.sort();
        vendor_choices.extend(
            by_name
                .into_iter()
                .map(|(name, vendor, count)| Choice { value: Some(vendor), label: format!("{name} ({count})").into() }),
        );
        let vendor = cx.new(|cx| {
            SelectState::new(SearchableVec::new(vendor_choices), Some(IndexPath::new(0)), window, cx).searchable(true)
        });

        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Filter by text"));
        let mut delegate = IssuesDelegate { catalog: catalog.clone(), visible: Vec::new(), sort: None };
        delegate.apply(&filter);
        let table = cx.new(|cx| TableState::new(delegate, window, cx).row_selectable(true));

        let subscriptions = vec![
            cx.subscribe(&severity, |this, _, event: &SelectEvent<SearchableVec<Choice<Severity>>>, cx| {
                if let SelectEvent::Confirm(Some(severity)) = event {
                    this.filter.min_severity = *severity;
                    this.apply_filter(cx);
                }
            }),
            cx.subscribe(&code, |this, _, event: &SelectEvent<SearchableVec<Choice<Option<&'static str>>>>, cx| {
                if let SelectEvent::Confirm(Some(code)) = event {
                    this.filter.code = *code;
                    this.apply_filter(cx);
                }
            }),
            cx.subscribe(&vendor, |this, _, event: &SelectEvent<SearchableVec<Choice<Option<usize>>>>, cx| {
                if let SelectEvent::Confirm(Some(vendor)) = event {
                    this.filter.vendor = *vendor;
                    this.apply_filter(cx);
                }
            }),
            cx.subscribe(&search, |this, search, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.filter.query = search.read(cx).value().to_string();
                    this.apply_filter(cx);
                }
            }),
            cx.subscribe(&table, |_, table, event: &TableEvent, cx| {
                if let TableEvent::SelectRow(row_ix) = event
                    && let Some(&index) = table.read(cx).delegate().visible.get(*row_ix)
                {
                    cx.emit(IssuesEvent::Reveal(index));
                }
            }),
        ];
        IssuesView {
            catalog: catalog.clone(),
            severity,
            code,
            vendor,
            search,
            table,
            filter,
            _subscriptions: subscriptions,
        }
    }

    #[cfg(test)]
    pub fn table(&self) -> &Entity<TableState<IssuesDelegate>> {
        &self.table
    }

    fn apply_filter(&mut self, cx: &mut Context<Self>) {
        let filter = self.filter.clone();
        self.table.update(cx, |table, cx| {
            table.delegate_mut().apply(&filter);
            table.clear_selection(cx);
            table.refresh(cx);
        });
        cx.notify();
    }
}

impl Render for IssuesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let shown = self.table.read(cx).delegate().visible.len();
        let total = self.catalog.repo.issues().len();
        let count = SharedString::from(format!("{shown} of {total}"));
        let toolbar = h_flex()
            .gap_2()
            .items_center()
            .child(div().text_sm().font_semibold().child("Issues"))
            .child(
                div()
                    .id("issues-count")
                    .test_support()
                    .aria_label(count.clone())
                    .text_sm()
                    .text_color(muted)
                    .child(count),
            )
            .child(Select::new(&self.severity).id("issues-severity").small().w_48())
            .child(Select::new(&self.code).id("issues-code").small().w_64().menu_width(rems(24.)))
            .child(Select::new(&self.vendor).id("issues-vendor").small().w_48().menu_width(rems(18.)))
            .child(
                Input::new(&self.search)
                    .id("issues-search")
                    .aria_label("Filter issues")
                    .small()
                    .prefix(Icon::new(IconName::Search).small())
                    .cleanable(true)
                    .w_56(),
            )
            .child(div().flex_1())
            .child(
                Button::new("close-issues")
                    .small()
                    .ghost()
                    .icon(IconName::X)
                    .tooltip("Close issues")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(IssuesEvent::Close))),
            );
        let body = if shown == 0 {
            let message = if total == 0 { "No issues" } else { "No issues match the filters" };
            div()
                .id("issues-empty")
                .test_support()
                .aria_label(message)
                .flex_1()
                .p_4()
                .text_sm()
                .text_color(muted)
                .child(message)
                .into_any_element()
        } else {
            div()
                .id("issues-list")
                .flex_1()
                .min_h_0()
                .child(DataTable::new(&self.table).stripe(true))
                .into_any_element()
        };
        v_flex().size_full().gap_2().pt_2().child(toolbar).child(body)
    }
}
