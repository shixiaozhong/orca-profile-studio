//! Manage machine-model records separately from presets, using the common file-plan preview.
use crate::{
    catalog::Catalog,
    manage::{ManagementEvent, preview_plan},
    preset_picker::Choice,
};
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IndexPath, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use profile_core::{MachineModel, ManagementPlan, ModelDraft, ModelPlan, preset_file_name};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operation {
    New,
    Copy,
    Edit,
    Delete,
}
impl Operation {
    const ALL: [Self; 4] = [Self::New, Self::Copy, Self::Edit, Self::Delete];
    fn label(self) -> String {
        match self {
            Self::New => t!("models.new"),
            Self::Copy => t!("models.copy"),
            Self::Edit => t!("models.edit"),
            Self::Delete => t!("models.delete"),
        }
    }
}

const FIELDS: [&str; 10] = [
    "name",
    "file_name",
    "model_id",
    "nozzle_diameter",
    "family",
    "machine_tech",
    "bed_model",
    "bed_texture",
    "hotend_model",
    "default_materials",
];
fn label(key: &str) -> String {
    match key {
        "name" => t!("models.name"),
        "file_name" => t!("manage.filename"),
        "model_id" => t!("details.model_id"),
        "nozzle_diameter" => t!("details.nozzles"),
        "family" => t!("details.family"),
        "machine_tech" => t!("details.technology"),
        "bed_model" => t!("details.bed_model"),
        "bed_texture" => t!("details.bed_texture"),
        "hotend_model" => t!("details.hotend"),
        "default_materials" => t!("models.materials"),
        _ => key.into(),
    }
}

pub struct ModelsView {
    catalog: Arc<Catalog>,
    /// The selected record survives repository index changes by file identity.
    selected: Option<PathBuf>,
    vendor: Option<usize>,
    operation: Operation,
    model_select: Entity<SelectState<SearchableVec<Choice>>>,
    vendor_select: Entity<SelectState<SearchableVec<Choice>>>,
    operation_select: Entity<SelectState<SearchableVec<Choice>>>,
    inputs: BTreeMap<&'static str, Entity<InputState>>,
    baseline: BTreeMap<&'static str, String>,
    generation: u64,
    source_changed: bool,
    busy: bool,
    applying: bool,
    task: Option<Task<()>>,
    status: Option<String>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}
impl EventEmitter<ManagementEvent> for ModelsView {}
impl ModelsView {
    pub fn new(
        catalog: &Arc<Catalog>,
        initial: Option<usize>,
        initial_vendor: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let selected = initial.and_then(|ix| catalog.repo.models().get(ix)).map(|m| m.path.clone());
        let vendor = initial
            .map(|ix| catalog.repo.models()[ix].vendor.index())
            .or(initial_vendor)
            .or_else(|| (!catalog.repo.vendors().is_empty()).then_some(0));
        let operation = if selected.is_some() { Operation::Edit } else { Operation::New };
        let model_select =
            cx.new(|cx| SelectState::new(SearchableVec::new(Vec::<Choice>::new()), None, window, cx).searchable(true));
        let vendor_select =
            cx.new(|cx| SelectState::new(SearchableVec::new(Vec::<Choice>::new()), None, window, cx).searchable(true));
        let operation_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(Vec::<Choice>::new()),
                Some(IndexPath::new(operation as usize)),
                window,
                cx,
            )
            .searchable(true)
        });
        let inputs: BTreeMap<_, _> =
            FIELDS.into_iter().map(|key| (key, cx.new(|cx| InputState::new(window, cx)))).collect();
        let mut subscriptions = vec![
            cx.subscribe_in(
                &model_select,
                window,
                |this, _, event: &SelectEvent<SearchableVec<Choice>>, window, cx| {
                    if let SelectEvent::Confirm(Some(ix)) = event {
                        let selected = this.catalog.repo.models().get(*ix).map(|m| m.path.clone());
                        if selected == this.selected {
                            return;
                        }
                        if this.busy || this.has_unapplied_input(cx) {
                            this.status = Some(t!("models.reset_first"));
                            this.sync_choices(window, cx);
                            return;
                        }
                        this.selected = selected;
                        this.vendor = this.catalog.repo.models().get(*ix).map(|m| m.vendor.index());
                        if this.operation == Operation::New {
                            this.operation = Operation::Edit;
                        }
                        this.sync_choices(window, cx);
                        this.fill(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &vendor_select,
                window,
                |this, _, event: &SelectEvent<SearchableVec<Choice>>, window, cx| {
                    if let SelectEvent::Confirm(Some(ix)) = event {
                        if this.busy || this.operation != Operation::New {
                            this.sync_choices(window, cx);
                            return;
                        }
                        this.vendor = Some(*ix);
                        this.invalidate(cx);
                    }
                },
            ),
            cx.subscribe_in(
                &operation_select,
                window,
                |this, _, event: &SelectEvent<SearchableVec<Choice>>, window, cx| {
                    if let SelectEvent::Confirm(Some(ix)) = event {
                        if Operation::ALL[*ix] == this.operation {
                            return;
                        }
                        if this.busy || this.has_unapplied_input(cx) {
                            this.status = Some(t!("models.reset_first"));
                            this.sync_choices(window, cx);
                            return;
                        }
                        this.operation = Operation::ALL[*ix];
                        this.fill(window, cx);
                    }
                },
            ),
        ];
        for input in inputs.values() {
            subscriptions.push(cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.invalidate(cx);
                }
            }));
        }
        let mut view = Self {
            catalog: catalog.clone(),
            selected,
            vendor,
            operation,
            model_select,
            vendor_select,
            operation_select,
            inputs,
            baseline: BTreeMap::new(),
            generation: 0,
            source_changed: false,
            busy: false,
            applying: false,
            task: None,
            status: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        view.sync_choices(window, cx);
        view.fill(window, cx);
        view
    }
    fn index(&self) -> Option<usize> {
        self.selected.as_ref().and_then(|p| self.catalog.repo.models().iter().position(|m| &m.path == p))
    }
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus.focus(window, cx);
    }
    pub fn is_applying(&self) -> bool {
        self.applying
    }
    pub fn has_unapplied_input(&self, cx: &App) -> bool {
        self.inputs
            .iter()
            .any(|(key, input)| self.baseline.get(key).is_some_and(|v| input.read(cx).value().as_ref() != v))
    }
    pub fn unsaved_path(&self, cx: &App) -> Option<PathBuf> {
        if !self.has_unapplied_input(cx) {
            return None;
        }
        let path = if let Some(selected) =
            self.selected.clone().filter(|_| matches!(self.operation, Operation::Edit | Operation::Delete))
        {
            selected
        } else {
            let vendor = self.vendor.map(|ix| self.catalog.repo.vendors()[ix].name.as_str()).unwrap_or("");
            let name = self.inputs["name"].read(cx).value();
            let entered = self.inputs["file_name"].read(cx).value();
            let file = if !entered.trim().is_empty() {
                entered.to_string()
            } else if name.trim().is_empty() {
                "new-model.json".into()
            } else {
                preset_file_name(&name)
            };
            PathBuf::from(vendor).join("machine").join(file)
        };
        Some(self.catalog.repo.root().join(path))
    }
    pub fn refresh_language(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_choices(window, cx);
    }
    fn sync_choices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let models = self
            .catalog
            .repo
            .models()
            .iter()
            .enumerate()
            .map(|(value, m)| Choice {
                value,
                title: format!("{} / {} — {}", self.catalog.repo.vendor(m.vendor).name, m.id, m.path.display()).into(),
            })
            .collect::<Vec<_>>();
        let selected = self.index();
        self.model_select.update(cx, |state, cx| {
            state.set_items(SearchableVec::new(models), window, cx);
            if let Some(ix) = selected {
                state.set_selected_value(&ix, window, cx);
            } else {
                state.set_selected_index(None, window, cx);
            }
        });
        let vendors = self
            .catalog
            .repo
            .vendors()
            .iter()
            .enumerate()
            .map(|(value, v)| Choice { value, title: v.name.clone().into() })
            .collect::<Vec<_>>();
        self.vendor_select.update(cx, |state, cx| {
            state.set_items(SearchableVec::new(vendors), window, cx);
            if let Some(ix) = self.vendor {
                state.set_selected_value(&ix, window, cx);
            }
        });
        let operations = Operation::ALL
            .iter()
            .enumerate()
            .map(|(value, op)| Choice { value, title: op.label().into() })
            .collect::<Vec<_>>();
        self.operation_select.update(cx, |state, cx| {
            state.set_items(SearchableVec::new(operations), window, cx);
            state.set_selected_value(&(self.operation as usize), window, cx);
        });
        cx.notify();
    }
    fn fill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.index().map(|ix| &self.catalog.repo.models()[ix]);
        let mut fields: BTreeMap<&str, String> = FIELDS.into_iter().map(|key| (key, String::new())).collect();
        if self.operation != Operation::New
            && let Some(model) = model
        {
            fields.insert("name", model.id.clone());
            fields.insert("file_name", model.path.file_name().unwrap().to_string_lossy().into());
            fields.insert("model_id", model.model_id.clone().unwrap_or_default());
            fields.insert("nozzle_diameter", model.variants.join(";"));
            fields.insert("family", model.family.clone().unwrap_or_default());
            fields.insert("machine_tech", model.technology.clone().unwrap_or_else(|| "FFF".into()));
            fields.insert("bed_model", model.bed_model.clone().unwrap_or_default());
            fields.insert("bed_texture", model.bed_texture.clone().unwrap_or_default());
            fields.insert("hotend_model", model.hotend_model.clone().unwrap_or_default());
            // The file's own order; the loaded list is sorted the way OrcaSlicer reads it.
            let materials = model.default_materials_text.clone().unwrap_or_else(|| model.default_materials.join(";"));
            fields.insert("default_materials", materials);
            if self.operation == Operation::Copy {
                fields.insert("name", String::new());
                fields.insert("file_name", String::new());
                fields.insert("model_id", String::new());
            }
        } else {
            fields.insert("nozzle_diameter", "0.4".into());
            fields.insert("machine_tech", "FFF".into());
        }
        for (key, value) in &fields {
            self.inputs[key].update(cx, |state, cx| state.set_value(value.clone(), window, cx));
        }
        self.baseline = fields;
        self.source_changed = false;
        self.invalidate(cx);
    }
    pub fn refresh(&mut self, catalog: &Arc<Catalog>, window: &mut Window, cx: &mut Context<Self>) {
        self.vendor = self.vendor.and_then(|ix| catalog.remap_vendor(&self.catalog, ix));
        let dirty = self.has_unapplied_input(cx);
        // Vendor indexes are local to one load; everything else describes the model file.
        let model = |catalog: &Catalog, ix: usize| MachineModel {
            vendor: Default::default(),
            ..catalog.repo.models()[ix].clone()
        };
        let old = self.index().map(|ix| model(&self.catalog, ix));
        self.catalog = catalog.clone();
        let new = self.index().map(|ix| model(&self.catalog, ix));
        if dirty && old != new {
            self.source_changed = true;
            self.status = Some(t!("models.source_changed"));
        }
        self.generation += 1;
        self.sync_choices(window, cx);
        if !dirty {
            self.fill(window, cx);
        }
        cx.notify();
    }
    fn invalidate(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.status = None;
        cx.notify();
    }
    fn preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if self.source_changed {
            self.status = Some(t!("models.source_changed"));
            cx.notify();
            return;
        }
        let Some(vendor) = self.vendor else {
            self.status = Some(t!("models.choose_vendor"));
            cx.notify();
            return;
        };
        let source = self.index();
        let operation = self.operation;
        if operation != Operation::New && source.is_none() {
            self.status = Some(t!("models.choose_model"));
            cx.notify();
            return;
        }
        let values = self
            .inputs
            .iter()
            .map(|(key, input)| (*key, input.read(cx).value().to_string()))
            .collect::<BTreeMap<_, _>>();
        let catalog = self.catalog.clone();
        let baseline = self.baseline.clone();
        self.busy = true;
        self.generation += 1;
        let generation = self.generation;
        let job = cx.background_executor().spawn(async move {
            let repo = &catalog.repo;
            let name = values["name"].trim();
            let file = values["file_name"].trim();
            let file = if file.is_empty() { preset_file_name(name) } else { file.into() };
            let mut draft = if matches!(operation, Operation::Copy | Operation::Edit) {
                ModelDraft::copy(repo, source.unwrap(), name, file)
            } else {
                ModelDraft::new(repo.vendors()[vendor].id, name, file)
            };
            for key in ModelDraft::FIELDS {
                if operation == Operation::New
                    || values[key] != baseline[key]
                    || (operation == Operation::Copy && key == "model_id")
                {
                    draft = draft.with_field(key, values[key].clone());
                }
            }
            let result = match operation {
                Operation::New | Operation::Copy => ModelPlan::create(repo, draft),
                Operation::Edit => ModelPlan::update(repo, source.unwrap(), draft),
                Operation::Delete => ModelPlan::delete(repo, source.unwrap()),
            };
            result.map(|plan| Arc::new(plan.into_file_plan())).map_err(|err| err.to_string())
        });
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                this.task = None;
                if this.generation != generation {
                    this.status = Some(t!("manage.stale_input"));
                    cx.notify();
                    return;
                }
                match result {
                    Err(err) => this.status = Some(err),
                    Ok(plan) => {
                        let view = cx.entity();
                        preview_plan(plan, operation == Operation::Delete, window, cx, move |plan, window, cx| {
                            view.update(cx, |this, cx| {
                                if this.generation == generation {
                                    this.apply(plan, window, cx);
                                } else {
                                    this.status = Some(t!("manage.stale"));
                                    cx.notify();
                                }
                            });
                        });
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
    fn apply(&mut self, plan: Arc<ManagementPlan>, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.applying = true;
        // Only the exact confirmed form becomes the next baseline. Input is disabled during writes.
        let submitted = self.inputs.iter().map(|(key, input)| (*key, input.read(cx).value().to_string())).collect();
        let new_name = self.inputs["name"].read(cx).value().trim().to_owned();
        let new_path = plan
            .changes()
            .find(|change| {
                change.after().is_some_and(|text| {
                    serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')).ok().is_some_and(
                        |v| {
                            v.as_object().is_some_and(|object| {
                                let field = |key: &str| {
                                    object
                                        .iter()
                                        .find(|(k, _)| k.eq_ignore_ascii_case(key))
                                        .and_then(|(_, v)| v.as_str())
                                };
                                field("name") == Some(new_name.as_str()) && field("type") == Some("machine_model")
                            })
                        },
                    )
                })
            })
            .map(|change| change.path().to_owned());
        let job = cx.background_executor().spawn(async move { plan.apply().map_err(|err| err.to_string()) });
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                this.applying = false;
                this.task = None;
                crate::feedback::clear("manage-writing", cx);
                match result {
                    Ok(()) => {
                        this.baseline = submitted;
                        this.selected = new_path;
                        this.operation = if this.selected.is_some() { Operation::Edit } else { Operation::New };
                        this.status = Some(t!("models.applied"));
                        cx.emit(ManagementEvent::Applied);
                    }
                    Err(err) => this.status = Some(t!("manage.failed", err = err)),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}
impl Render for ModelsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("models-view")
            .test_support()
            .track_focus(&self.focus)
            .size_full()
            .gap_3()
            .p_4()
            .child(
                h_flex()
                    .gap_3()
                    .flex_shrink_0()
                    .child(
                        Button::new("models-close")
                            .small()
                            .label(t!("common.back_browser"))
                            .disabled(self.busy)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ManagementEvent::Close))),
                    )
                    .child(div().text_lg().child(t!("models.title"))),
            )
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .flex_shrink_0()
                    .child(
                        v_flex().gap_1().child(t!("models.select")).child(
                            div().w_96().h_8().child(
                                Select::new(&self.model_select)
                                    .id("models-source")
                                    .accessibility_label(t!("models.select"))
                                    .small()
                                    .disabled(self.busy),
                            ),
                        ),
                    )
                    .child(
                        v_flex().gap_1().child(t!("common.vendor")).child(
                            div().w_56().h_8().child(
                                Select::new(&self.vendor_select)
                                    .id("models-vendor")
                                    .accessibility_label(t!("common.vendor"))
                                    .small()
                                    .disabled(self.busy || self.operation != Operation::New),
                            ),
                        ),
                    )
                    .child(
                        v_flex().gap_1().child(t!("batch.operation")).child(
                            div().w_56().h_8().child(
                                Select::new(&self.operation_select)
                                    .id("models-operation")
                                    .accessibility_label(t!("batch.operation"))
                                    .small()
                                    .disabled(self.busy),
                            ),
                        ),
                    ),
            )
            .child(
                div().id("models-form").test_support().flex_1().min_h_0().overflow_y_scroll().child(
                    v_flex()
                        .gap_3()
                        .child(div().text_sm().text_color(cx.theme().muted_foreground).child(
                            if self.operation == Operation::Delete {
                                t!("models.delete_help")
                            } else {
                                t!("models.help")
                            },
                        ))
                        .when(self.operation != Operation::Delete, |form| {
                            form.child(h_flex().gap_3().flex_wrap().items_start().children(FIELDS.into_iter().map(
                                |key| {
                                    v_flex().w_80().gap_1().child(div().text_sm().child(label(key))).child(
                                        Input::new(&self.inputs[key])
                                            .id(SharedString::from(format!("models-{key}")))
                                            .aria_label(label(key))
                                            .disabled(self.applying)
                                            .small(),
                                    )
                                },
                            )))
                        }),
                ),
            )
            .when_some(self.status.clone(), |view, status| {
                view.child(div().id("models-status").test_support().aria_label(status.clone()).text_sm().child(status))
            })
            .child(
                h_flex()
                    .gap_2()
                    .flex_shrink_0()
                    .child(
                        Button::new("models-preview")
                            .label(t!("manage.preview"))
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.preview(window, cx))),
                    )
                    .child(
                        Button::new("models-reset")
                            .label(t!("batch.reset"))
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.fill(window, cx))),
                    )
                    .when(self.busy, |line| line.child(t!("manage.busy"))),
            )
    }
}
