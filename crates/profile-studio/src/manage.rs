//! Preset management: select files, configure one operation, preview, then write explicitly.

use std::collections::BTreeMap;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariant};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::select::{SearchableVec, Select, SelectEvent, SelectState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use profile_core::{ManagementPlan, NameRule, PresetDraft, RawValue, preset_file_name};

use crate::catalog::Catalog;
use crate::preset_picker::{Choice, PresetPicker};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Operation {
    New,
    Copy,
    BatchCopy,
    Rename,
    Delete,
}
impl Operation {
    const ALL: [Self; 5] = [Self::New, Self::Copy, Self::BatchCopy, Self::Rename, Self::Delete];
    fn label(self) -> &'static str {
        match self {
            Self::New => "New preset",
            Self::Copy => "Copy preset",
            Self::BatchCopy => "Copy group",
            Self::Rename => "Rename preset",
            Self::Delete => "Delete presets",
        }
    }
}

pub enum ManagementEvent {
    Applied,
    Close,
}

pub struct ManagementView {
    catalog: Arc<Catalog>,
    picker: Entity<PresetPicker>,
    operation: Operation,
    operation_select: Entity<SelectState<SearchableVec<Choice>>>,
    name: Entity<InputState>,
    file: Entity<InputState>,
    from: Entity<InputState>,
    to: Entity<InputState>,
    overrides: Entity<TextareaState>,
    inherit_source: bool,
    keep_identifiers: bool,
    rewrite_arrays: bool,
    remember_name: bool,
    status: Option<String>,
    plan: Option<Arc<ManagementPlan>>,
    busy: bool,
    applying: bool,
    generation: u64,
    selection: (Vec<profile_core::PresetId>, profile_core::PresetKind, Option<usize>),
    task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ManagementEvent> for ManagementView {}

impl ManagementView {
    pub fn new(catalog: &Arc<Catalog>, initial: Option<usize>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let picker = cx.new(|cx| PresetPicker::new(catalog, initial, window, cx));
        let choices = Operation::ALL
            .iter()
            .enumerate()
            .map(|(value, op)| Choice { value, title: op.label().into() })
            .collect::<Vec<_>>();
        let operation_select = cx.new(|cx| {
            SelectState::new(SearchableVec::new(choices), Some(IndexPath::new(1)), window, cx).searchable(true)
        });
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("New preset name"));
        let file = cx.new(|cx| InputState::new(window, cx).placeholder("Generated from the new name"));
        let from = cx.new(|cx| InputState::new(window, cx).placeholder("0.4 nozzle"));
        let to = cx.new(|cx| InputState::new(window, cx).placeholder("0.2 nozzle"));
        let overrides = cx.new(|cx| TextareaState::new(window, cx));
        let mut subscriptions =
            vec![cx.subscribe(&operation_select, |this, _, event: &SelectEvent<SearchableVec<Choice>>, cx| {
                if let SelectEvent::Confirm(Some(ix)) = event {
                    this.operation = Operation::ALL[*ix];
                    this.invalidate(cx);
                }
            })];
        for input in [&name, &file, &from, &to] {
            subscriptions.push(cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.invalidate(cx);
                }
            }));
        }
        subscriptions.push(cx.subscribe(&overrides, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.invalidate(cx);
            }
        }));
        let selection = {
            let picker = picker.read(cx);
            (picker.selected(cx), picker.kind(), picker.vendor())
        };
        subscriptions.push(cx.observe(&picker, |this, picker, cx| {
            let picker = picker.read(cx);
            let selection = (picker.selected(cx), picker.kind(), picker.vendor());
            if this.selection != selection {
                this.selection = selection;
                this.invalidate(cx);
            }
        }));
        Self {
            catalog: catalog.clone(),
            picker,
            operation: Operation::Copy,
            operation_select,
            name,
            file,
            from,
            to,
            overrides,
            inherit_source: false,
            keep_identifiers: false,
            rewrite_arrays: false,
            remember_name: true,
            status: None,
            plan: None,
            busy: false,
            applying: false,
            generation: 0,
            selection,
            task: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn refresh(&mut self, catalog: &Arc<Catalog>, window: &mut Window, cx: &mut Context<Self>) {
        self.catalog = catalog.clone();
        self.picker.update(cx, |picker, cx| picker.refresh(catalog, window, cx));
        self.plan = None;
        self.generation += 1;
        cx.notify();
    }

    pub fn is_applying(&self) -> bool {
        self.applying
    }

    fn invalidate(&mut self, cx: &mut Context<Self>) {
        self.plan = None;
        self.status = None;
        self.generation += 1;
        cx.notify();
    }

    fn preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = None;
        self.generation += 1;
        let generation = self.generation;
        let catalog = self.catalog.clone();
        let picker = self.picker.read(cx);
        let ids = picker.selected(cx);
        let vendor = picker.vendor();
        let kind = picker.kind();
        let op = self.operation;
        let name = self.name.read(cx).value().trim().to_owned();
        let file = self.file.read(cx).value().trim().to_owned();
        let file = if file.is_empty() { preset_file_name(&name) } else { file };
        let from = self.from.read(cx).value().to_string();
        let to = self.to.read(cx).value().to_string();
        let raw_values = self.overrides.read(cx).value().to_string();
        let (inherit, keep, rewrite, remember) =
            (self.inherit_source, self.keep_identifiers, self.rewrite_arrays, self.remember_name);
        let job = cx.background_executor().spawn(async move {
            let repo = &catalog.repo;
            let values = if matches!(op, Operation::New | Operation::Copy | Operation::BatchCopy) {
                parse_overrides(&raw_values)?
            } else {
                BTreeMap::new()
            };
            let draft_values = |mut draft: PresetDraft| {
                for (key, value) in &values {
                    draft = draft.with_value(key.clone(), value.clone());
                }
                draft
            };
            let one = || {
                if ids.len() == 1 { Ok(ids[0]) } else { Err("Select exactly one preset for this operation".to_owned()) }
            };
            let result = match op {
                Operation::New => {
                    let vendor = vendor
                        .map(|ix| repo.vendors()[ix].id)
                        .or_else(|| ids.first().map(|id| repo.preset(*id).vendor))
                        .ok_or_else(|| "Choose a vendor or select a parent preset".to_owned())?;
                    let mut draft = PresetDraft::new(vendor, kind, name, file);
                    if inherit {
                        draft = draft.with_parent(one()?);
                    }
                    ManagementPlan::create(repo, draft_values(draft))
                }
                Operation::Copy => ManagementPlan::copy(
                    repo,
                    vec![draft_values(
                        PresetDraft::copy(repo, one()?, name, file).inherit_source(inherit).keep_identifiers(keep),
                    )],
                    None,
                ),
                Operation::BatchCopy => {
                    let mut rule = NameRule::new(from, to);
                    if rewrite {
                        rule = rule.rewrite_array("compatible_printers").rewrite_array("compatible_prints");
                    }
                    let mut drafts = Vec::new();
                    let mut errors = Vec::new();
                    for id in ids {
                        match rule.replace(repo.preset(id).name()) {
                            Ok(name) => drafts.push(draft_values(
                                PresetDraft::copy(repo, id, &name, preset_file_name(&name)).keep_identifiers(keep),
                            )),
                            Err(err) => errors.push(err.to_string()),
                        }
                    }
                    if !errors.is_empty() {
                        return Err(errors.join("\n"));
                    }
                    ManagementPlan::copy(repo, drafts, Some(rule))
                }
                Operation::Rename => ManagementPlan::rename(repo, one()?, &name, &file, remember),
                Operation::Delete => ManagementPlan::delete(repo, &ids),
            };
            result.map(Arc::new).map_err(|err| err.to_string())
        });
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                this.task = None;
                if this.generation != generation {
                    this.status = Some("Inputs changed while preparing the preview. Preview again.".into());
                    cx.notify();
                    return;
                }
                match result {
                    Ok(plan) => {
                        this.plan = Some(plan);
                        this.open_preview(window, cx);
                    }
                    Err(err) => this.status = Some(err),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn open_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(plan) = self.plan.clone() else { return };
        let view = cx.entity();
        let generation = self.generation;
        let destructive = self.operation == Operation::Delete;
        window.open_alert_dialog(cx, move |dialog, window, cx| {
            let mut content = v_flex().gap_2();
            for notice in plan.notices() {
                let color = if notice.is_blocking() { cx.theme().danger } else { cx.theme().warning };
                content = content.child(div().text_color(color).child(format!(
                    "{}: {}",
                    if notice.is_blocking() { "Cannot apply" } else { "Review" },
                    notice.message()
                )));
            }
            for change in plan.changes() {
                let action = match (change.before(), change.after()) {
                    (None, Some(_)) => "Create",
                    (Some(_), None) => "Delete",
                    _ => "Update",
                };
                content = content.child(div().child(format!("{action} {}", change.path().display())));
                for line in change_summary(change.before(), change.after()) {
                    content = content.child(div().pl_3().text_sm().child(line));
                }
            }
            let view = view.clone();
            let plan = plan.clone();
            let ready = plan.is_ready();
            dialog
                .title(if ready { "Apply these file changes?" } else { "Resolve the preview issues" })
                .width(px(760.))
                .confirm()
                .description(if ready {
                    "This writes the listed files and vendor manifests. Files are rechecked before writing."
                } else {
                    "No files have been written. Close this preview and adjust the selection or values."
                })
                .child(
                    div()
                        .id("manage-preview")
                        .test_support()
                        .aria_label(format!("{} file changes", plan.changes().count()))
                        .max_h(window.viewport_size().height * 0.5)
                        .overflow_y_scroll()
                        .child(content),
                )
                .ok_text(if ready { "Apply changes" } else { "Back" })
                .ok_variant(if ready && destructive { ButtonVariant::Danger } else { ButtonVariant::Primary })
                .on_ok(move |_, window, cx| {
                    if ready {
                        view.update(cx, |view, cx| {
                            if view.generation == generation {
                                view.apply(plan.clone(), window, cx);
                            } else {
                                view.status = Some("The selection or profiles changed. Create a new preview.".into());
                                cx.notify();
                            }
                        });
                    }
                    true
                })
        });
    }

    fn apply(&mut self, plan: Arc<ManagementPlan>, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.applying = true;
        let count = plan.changes().count();
        let job = cx.background_executor().spawn(async move { plan.apply().map_err(|err| err.to_string()) });
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                this.applying = false;
                this.task = None;
                this.plan = None;
                crate::feedback::clear("manage-writing", cx);
                match result {
                    Ok(()) => {
                        this.status = Some(format!("Applied changes to {count} file(s)."));
                        cx.emit(ManagementEvent::Applied);
                    }
                    Err(err) => this.status = Some(format!("Could not apply changes: {err}")),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

fn parse_overrides(text: &str) -> Result<BTreeMap<String, RawValue>, String> {
    let mut values = BTreeMap::new();
    for (ix, line) in text.lines().enumerate().filter(|(_, line)| !line.trim().is_empty()) {
        let (key, value) = line.split_once('=').ok_or_else(|| format!("Override line {} needs key = value", ix + 1))?;
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() {
            return Err(format!("Override line {} has no key", ix + 1));
        }
        let raw = if value.starts_with('[') || value.starts_with('"') {
            serde_json::from_str::<RawValue>(value).map_err(|err| format!("{key}: {err}"))?
        } else {
            RawValue::Str(value.into())
        };
        if values.insert(key.into(), raw).is_some() {
            return Err(format!("Override {key} occurs more than once"));
        }
    }
    Ok(values)
}

fn change_summary(before: Option<&str>, after: Option<&str>) -> Vec<String> {
    let object = |text: Option<&str>| {
        text.and_then(|text| serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}')).ok())
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default()
    };
    let (old, new) = (object(before), object(after));
    let keys: std::collections::BTreeSet<_> = old.keys().chain(new.keys()).collect();
    let mut lines = Vec::new();
    for key in keys.into_iter().filter(|key| old.get(*key) != new.get(*key)) {
        if key.ends_with("_list")
            && let (Some(serde_json::Value::Array(before)), Some(serde_json::Value::Array(after))) =
                (old.get(key), new.get(key))
        {
            let mut removed = before.clone();
            let mut added = Vec::new();
            for item in after {
                if let Some(ix) = removed.iter().position(|old| old == item) {
                    removed.remove(ix);
                } else {
                    added.push(item);
                }
            }
            lines.extend(removed.iter().map(|item| format!("{key}: remove {item}")));
            lines.extend(added.iter().map(|item| format!("{key}: add {item}")));
        } else {
            let show = |value: Option<&serde_json::Value>| value.map_or("(missing)".into(), |v| v.to_string());
            lines.push(format!("{key}: {} → {}", show(old.get(key)), show(new.get(key))));
        }
    }
    lines
}

impl Render for ManagementView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let operation = self.operation;
        let single = matches!(operation, Operation::New | Operation::Copy | Operation::Rename);
        let copying = matches!(operation, Operation::Copy | Operation::BatchCopy);
        let field = |label: &'static str, id: &'static str, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .child(div().text_sm().child(label))
                .child(Input::new(input).id(id).aria_label(label).small().w_64())
        };
        let mut form = v_flex().gap_3().p_3().child(div().w_48().h_8().child(
            Select::new(&self.operation_select).id("manage-operation").accessibility_label("Preset operation").small(),
        ));
        if single {
            form = form.child(
                h_flex().gap_3().flex_wrap().child(field("New name", "manage-name", &self.name)).child(field(
                    "File name (.json)",
                    "manage-file",
                    &self.file,
                )),
            );
        }
        if operation == Operation::BatchCopy {
            form = form.child(
                h_flex().gap_3().flex_wrap().child(field("Replace in names", "manage-from", &self.from)).child(field(
                    "With",
                    "manage-to",
                    &self.to,
                )),
            );
        }
        if matches!(operation, Operation::New | Operation::Copy) {
            form = form.child(
                Checkbox::new("manage-inherit-source")
                    .label("Inherit the selected preset")
                    .checked(self.inherit_source)
                    .on_click(cx.listener(|this, value: &bool, _, cx| {
                        this.inherit_source = *value;
                        this.invalidate(cx);
                    })),
            );
        }
        if copying {
            form = form.child(
                Checkbox::new("manage-keep-identifiers")
                    .label("Keep source setting_id and filament_id (review in preview)")
                    .checked(self.keep_identifiers)
                    .on_click(cx.listener(|this, value: &bool, _, cx| {
                        this.keep_identifiers = *value;
                        this.invalidate(cx);
                    })),
            );
        }
        if operation == Operation::BatchCopy {
            form = form.child(
                Checkbox::new("manage-rewrite-arrays")
                    .label("Also replace names in compatible_printers and compatible_prints")
                    .checked(self.rewrite_arrays)
                    .on_click(cx.listener(|this, value: &bool, _, cx| {
                        this.rewrite_arrays = *value;
                        this.invalidate(cx);
                    })),
            );
        }
        if operation == Operation::Rename {
            form = form.child(
                Checkbox::new("manage-remember-name")
                    .label("Record the previous name in renamed_from")
                    .checked(self.remember_name)
                    .on_click(cx.listener(|this, value: &bool, _, cx| {
                        this.remember_name = *value;
                        this.invalidate(cx);
                    })),
            );
        }
        if matches!(operation, Operation::New | Operation::Copy | Operation::BatchCopy) {
            form = form.child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child("Set values on the new presets"))
                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child(
                        "One key = value per line. Arrays use [\"value\", \"value\"]. Identifiers can be set here too.",
                    ))
                    .child(
                        div()
                            .id("manage-overrides")
                            .test_support()
                            .aria_label("Override values")
                            .child(Textarea::new(&self.overrides).aria_label("Values for new presets").h_24()),
                    ),
            );
        }
        if operation == Operation::Delete {
            form = form.child(div().text_sm().child(
                "The preview lists references. Include dependent children in the selection; name-list references are removed only when you confirm.",
            ));
        }

        v_flex()
            .size_full()
            .p_4()
            .gap_2()
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        Button::new("close-management")
                            .label("Back to browser")
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ManagementEvent::Close))),
                    )
                    .child(div().text_lg().child("Manage presets")),
            )
            .child(div().h(rems(17.)).max_h(relative(0.45)).flex_shrink_0().child(self.picker.clone()))
            .child(div().id("manage-form").test_support().flex_1().min_h_0().overflow_y_scroll().child(form))
            .when_some(self.status.clone(), |view, status| {
                view.child(div().id("manage-status").test_support().aria_label(status.clone()).text_sm().child(status))
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("manage-preview-button")
                            .label("Preview changes…")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.preview(window, cx))),
                    )
                    .when(self.busy, |line| line.child(Spinner::new().small()).child("Preparing or writing changes")),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{change_summary, parse_overrides};
    use profile_core::RawValue;

    #[test]
    fn override_input_preserves_value_shapes_and_rejects_ambiguous_lines() {
        let values =
            parse_overrides("wall_loops = 3\nnozzle_diameter = [\"0.2\"]\nprinter_notes = \"one\\ntwo\"").unwrap();
        assert_eq!(values["wall_loops"], RawValue::Str("3".into()));
        assert_eq!(values["nozzle_diameter"], RawValue::List(vec!["0.2".into()]));
        assert_eq!(values["printer_notes"], RawValue::Str("one\ntwo".into()));
        assert!(parse_overrides("a = 1\na = 2").is_err());
        assert!(parse_overrides("a = [2]").is_err());
    }

    #[test]
    fn manifest_preview_only_lists_changed_entries() {
        let before = r#"{"process_list":[{"name":"kept"},{"name":"old"}],"version":"1.0"}"#;
        let after = r#"{"process_list":[{"name":"kept"},{"name":"new"}],"version":"1.0"}"#;
        let lines = change_summary(Some(before), Some(after));
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("remove") && lines[0].contains("old"));
        assert!(lines[1].contains("add") && lines[1].contains("new"));
        assert!(lines.iter().all(|line| !line.contains("kept") && !line.contains("version")));
    }
}
