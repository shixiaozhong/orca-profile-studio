//! A reusable, virtualized selection table for preset management and batch editing.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::button::Button;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IndexPath, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use profile_core::{PresetId, PresetKind};

use crate::catalog::Catalog;

#[derive(Clone)]
#[non_exhaustive]
pub struct Choice {
    pub value: usize,
    pub title: SharedString,
}

impl SelectItem for Choice {
    type Value = usize;
    fn title(&self) -> SharedString {
        self.title.clone()
    }
    fn value(&self) -> &usize {
        &self.value
    }
}

pub struct PresetSelection {
    catalog: Arc<Catalog>,
    visible: Vec<usize>,
    selected: BTreeSet<PathBuf>,
}

impl PresetSelection {
    pub fn selected(&self) -> Vec<PresetId> {
        self.catalog.repo.presets().iter().filter(|p| self.selected.contains(&p.path)).map(|p| p.id).collect()
    }
}

impl TableDelegate for PresetSelection {
    fn columns_count(&self, _: &App) -> usize {
        5
    }
    fn rows_count(&self, _: &App) -> usize {
        self.visible.len()
    }
    fn column(&self, col: usize, _: &App) -> Column {
        let (key, label, width) = [
            ("selected", "Select", 70.),
            ("vendor", "Vendor", 140.),
            ("name", "Preset", 280.),
            ("parent", "Inherits", 240.),
            ("path", "File", 320.),
        ][col];
        Column::new(key, label).width(px(width))
    }
    fn cell_text(&self, row: usize, col: usize, _: &App) -> String {
        let Some(&index) = self.visible.get(row) else { return String::new() };
        let repo = &self.catalog.repo;
        let preset = &repo.presets()[index];
        match col {
            0 => self.selected.contains(&preset.path).to_string(),
            1 => repo.vendor(preset.vendor).name.clone(),
            2 => preset.name().into(),
            3 => preset.meta.inherits.clone().unwrap_or_default(),
            _ => preset.path.display().to_string(),
        }
    }
    fn render_td(
        &mut self,
        row: usize,
        col: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let preset = &self.catalog.repo.presets()[self.visible[row]];
        if col == 0 {
            let path = preset.path.clone();
            Checkbox::new(SharedString::from(format!("choose-{}", path.display())))
                .label("Select")
                .accessibility_label(format!("Select {}", preset.name()))
                .checked(self.selected.contains(&path))
                .on_click(cx.listener(move |table, checked: &bool, _, cx| {
                    if *checked {
                        table.delegate_mut().selected.insert(path.clone());
                    } else {
                        table.delegate_mut().selected.remove(&path);
                    }
                    cx.notify();
                }))
                .into_any_element()
        } else {
            div().text_sm().child(self.cell_text(row, col, cx)).into_any_element()
        }
    }
}

pub struct PresetPicker {
    catalog: Arc<Catalog>,
    table: Entity<TableState<PresetSelection>>,
    kind: PresetKind,
    vendor: Option<usize>,
    kind_select: Entity<SelectState<SearchableVec<Choice>>>,
    vendor_select: Entity<SelectState<SearchableVec<Choice>>>,
    search: Entity<InputState>,
    model: Entity<InputState>,
    model_matches: BTreeSet<usize>,
    model_generation: u64,
    model_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl PresetPicker {
    pub fn new(catalog: &Arc<Catalog>, initial: Option<usize>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let kind = initial.map_or(PresetKind::Process, |ix| catalog.repo.presets()[ix].kind);
        let vendor = initial.map(|ix| catalog.repo.presets()[ix].vendor.index());
        let kinds = PresetKind::ALL
            .iter()
            .enumerate()
            .map(|(value, kind)| Choice { value, title: kind.as_str().into() })
            .collect::<Vec<_>>();
        let mut vendors = vec![Choice { value: 0, title: "All vendors".into() }];
        vendors.extend(
            catalog
                .repo
                .vendors()
                .iter()
                .enumerate()
                .map(|(ix, v)| Choice { value: ix + 1, title: v.name.clone().into() }),
        );
        let kind_select =
            cx.new(|cx| SelectState::new(SearchableVec::new(kinds), Some(IndexPath::new(kind as usize)), window, cx));
        let vendor_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(vendors),
                Some(IndexPath::new(vendor.map_or(0, |ix| ix + 1))),
                window,
                cx,
            )
            .searchable(true)
        });
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search presets"));
        let model = cx.new(|cx| InputState::new(window, cx).placeholder("Filter by printer model"));
        let selected = initial.map(|ix| catalog.repo.presets()[ix].path.clone()).into_iter().collect();
        let table = cx.new(|cx| {
            TableState::new(PresetSelection { catalog: catalog.clone(), visible: Vec::new(), selected }, window, cx)
        });
        let subscriptions = vec![
            cx.subscribe(&kind_select, |this, _, event: &SelectEvent<SearchableVec<Choice>>, cx| {
                if let SelectEvent::Confirm(Some(ix)) = event {
                    this.kind = PresetKind::ALL[*ix];
                    this.table.update(cx, |table, _| table.delegate_mut().selected.clear());
                    this.apply_filter(cx);
                }
            }),
            cx.subscribe(&vendor_select, |this, _, event: &SelectEvent<SearchableVec<Choice>>, cx| {
                if let SelectEvent::Confirm(Some(ix)) = event {
                    this.vendor = ix.checked_sub(1);
                    this.apply_filter(cx);
                }
            }),
            cx.subscribe(&search, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.apply_filter(cx);
                }
            }),
            cx.subscribe(&model, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.update_model_filter(cx);
                    this.apply_filter(cx);
                }
            }),
            cx.observe(&table, |_, _, cx| cx.notify()),
        ];
        let mut picker = Self {
            catalog: catalog.clone(),
            table,
            kind,
            vendor,
            kind_select,
            vendor_select,
            search,
            model,
            model_matches: BTreeSet::new(),
            model_generation: 0,
            model_task: None,
            _subscriptions: subscriptions,
        };
        picker.apply_filter(cx);
        picker
    }

    pub fn selected(&self, cx: &App) -> Vec<PresetId> {
        self.table.read(cx).delegate().selected()
    }
    pub fn kind(&self) -> PresetKind {
        self.kind
    }
    pub fn vendor(&self) -> Option<usize> {
        self.vendor
    }

    pub fn refresh(&mut self, catalog: &Arc<Catalog>, window: &mut Window, cx: &mut Context<Self>) {
        let vendor = self.vendor.and_then(|ix| catalog.remap_vendor(&self.catalog, ix));
        self.vendor = vendor;
        self.catalog = catalog.clone();
        self.table.update(cx, |table, _| {
            table.delegate_mut().catalog = catalog.clone();
            table
                .delegate_mut()
                .selected
                .retain(|path| catalog.preset_at(path).is_some_and(|ix| catalog.repo.presets()[ix].kind == self.kind));
        });
        let mut choices = vec![Choice { value: 0, title: "All vendors".into() }];
        choices.extend(
            catalog
                .repo
                .vendors()
                .iter()
                .enumerate()
                .map(|(ix, v)| Choice { value: ix + 1, title: v.name.clone().into() }),
        );
        self.vendor_select.update(cx, |state, cx| {
            state.set_items(SearchableVec::new(choices), window, cx);
            state.set_selected_value(&vendor.map_or(0, |ix| ix + 1), window, cx);
        });
        self.update_model_filter(cx);
        self.apply_filter(cx);
    }

    fn update_model_filter(&mut self, cx: &mut Context<Self>) {
        self.model_matches.clear();
        self.model_generation += 1;
        self.model_task = None;
        let generation = self.model_generation;
        let query = self.model.read(cx).value().trim().to_lowercase();
        if query.is_empty() {
            return;
        }
        let catalog = self.catalog.clone();
        let job = cx.background_executor().spawn(async move {
            let repo = &catalog.repo;
            let mut matches = BTreeSet::new();
            for printer in repo.presets().iter().filter(|p| p.kind == PresetKind::Machine && repo.is_selectable(p.id)) {
                let model = repo.lookup_scalar(printer.id, "printer_model").unwrap_or("");
                if model.to_lowercase().contains(&query) {
                    matches.insert(printer.id.index());
                    for kind in [PresetKind::Process, PresetKind::Filament] {
                        matches.extend(
                            repo.compatibility()
                                .compatible_with_printer(repo, printer.id, kind)
                                .into_iter()
                                .map(PresetId::index),
                        );
                    }
                }
            }
            matches
        });
        self.model_task = Some(cx.spawn(async move |this, cx| {
            let matches = job.await;
            this.update(cx, |this, cx| {
                if this.model_generation == generation {
                    this.model_task = None;
                    this.model_matches = matches;
                    this.apply_filter(cx);
                }
            })
            .ok();
        }));
    }

    fn apply_filter(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().to_lowercase();
        let model = !self.model.read(cx).value().trim().is_empty();
        let repo = &self.catalog.repo;
        let visible = repo
            .presets()
            .iter()
            .filter(|p| {
                p.kind == self.kind
                    && self.vendor.is_none_or(|v| p.vendor.index() == v)
                    && (!model || self.model_matches.contains(&p.id.index()))
                    && query.split_whitespace().all(|word| {
                        format!("{} {} {}", repo.vendor(p.vendor).name, p.name(), p.path.display())
                            .to_lowercase()
                            .contains(word)
                    })
            })
            .map(|p| p.id.index())
            .collect::<Vec<_>>();
        // Operations act on the selection, so it never holds rows the filters hide. A pending model
        // filter shows nothing yet; prune once its result arrives.
        let pending = model && self.model_task.is_some();
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            if !pending {
                let shown: BTreeSet<&PathBuf> = visible.iter().map(|&ix| &repo.presets()[ix].path).collect();
                delegate.selected.retain(|path| shown.contains(path));
            }
            delegate.visible = visible;
            table.refresh(cx);
        });
        cx.notify();
    }
}

impl Render for PresetPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let delegate = self.table.read(cx).delegate();
        let count = format!(
            "{} selected · {} shown{}",
            delegate.selected.len(),
            delegate.visible.len(),
            if self.model_task.is_some() { " · filtering models" } else { "" }
        );
        v_flex()
            .size_full()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .flex_shrink_0()
                    .child(div().w_40().h_8().child(
                        Select::new(&self.kind_select).id("manage-kind").accessibility_label("Preset kind").small(),
                    ))
                    .child(
                        div().w_48().h_8().child(
                            Select::new(&self.vendor_select)
                                .id("manage-vendor")
                                .accessibility_label("Vendor filter")
                                .small(),
                        ),
                    )
                    .child(
                        Input::new(&self.search)
                            .id("manage-search")
                            .aria_label("Search presets to manage")
                            .small()
                            .w_56(),
                    )
                    .child(
                        Input::new(&self.model).id("manage-model").aria_label("Printer model filter").small().w_56(),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_shrink_0()
                    .child(div().id("manage-count").test_support().aria_label(count.clone()).text_sm().child(count))
                    .child(
                        Button::new("manage-select-shown")
                            .small()
                            .disabled(self.model_task.is_some())
                            .label("Select shown")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.table.update(cx, |table, cx| {
                                    let paths: Vec<_> = table
                                        .delegate()
                                        .visible
                                        .iter()
                                        .map(|ix| table.delegate().catalog.repo.presets()[*ix].path.clone())
                                        .collect();
                                    table.delegate_mut().selected.extend(paths);
                                    cx.notify();
                                });
                            })),
                    )
                    .child(Button::new("manage-clear-selection").small().label("Clear selection").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.table.update(cx, |table, cx| {
                                table.delegate_mut().selected.clear();
                                cx.notify();
                            })
                        }),
                    )),
            )
            .child(div().id("manage-preset-table").flex_1().min_h_0().child(DataTable::new(&self.table).stripe(true)))
            .text_color(cx.theme().foreground)
    }
}
