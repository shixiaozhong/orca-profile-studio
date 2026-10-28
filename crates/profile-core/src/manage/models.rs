//! Machine models have their own identity and fields; file transactions are shared with presets.
use super::{ManageError, ManagementPlan, array, check_filename, check_name, invalid, set_meta};
use crate::{Document, PresetKind, RawValue, Repository, VendorId};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Component, Path},
};

/// Editable machine-model fields. Unknown fields of a copied or edited source are retained.
#[derive(Clone)]
pub struct ModelDraft {
    vendor: VendorId,
    source: Option<usize>,
    name: String,
    file_name: String,
    fields: BTreeMap<String, String>,
}
impl ModelDraft {
    /// Fields set through [`Self::with_field`]; the name is a separate draft member.
    pub const FIELDS: [&'static str; 8] = [
        "model_id",
        "nozzle_diameter",
        "family",
        "machine_tech",
        "bed_model",
        "bed_texture",
        "hotend_model",
        "default_materials",
    ];
    pub fn new(vendor: VendorId, name: impl Into<String>, file_name: impl Into<String>) -> Self {
        Self {
            vendor,
            source: None,
            name: name.into(),
            file_name: file_name.into(),
            fields: BTreeMap::from([("machine_tech".into(), "FFF".into()), ("nozzle_diameter".into(), "0.4".into())]),
        }
    }
    pub fn copy(repo: &Repository, source: usize, name: impl Into<String>, file_name: impl Into<String>) -> Self {
        Self {
            source: Some(source),
            fields: BTreeMap::new(),
            ..Self::new(repo.models()[source].vendor, name, file_name)
        }
    }
    pub fn with_field(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }
}

/// A validated model operation, backed by the same snapshot and rollback transaction as M10.
pub struct ModelPlan {
    files: ManagementPlan,
}
impl ModelPlan {
    pub fn into_file_plan(self) -> ManagementPlan {
        self.files
    }
    pub fn files(&self) -> &ManagementPlan {
        &self.files
    }
    pub fn apply(&self) -> Result<(), ManageError> {
        self.files.apply()
    }

    pub fn create(repo: &Repository, mut draft: ModelDraft) -> Result<Self, ManageError> {
        // Copy defaults come from the source, not from the empty-model defaults.
        if let Some(source) = draft.source {
            let model = repo.models().get(source).ok_or_else(|| invalid("Source model no longer exists; Reload"))?;
            if model.vendor != draft.vendor {
                return Err(invalid("Copy a model within its vendor"));
            }
        }
        Self::build(repo, None, &mut draft)
    }

    /// Changes fields and, when requested, the file/name and every own printer_model reference.
    pub fn update(repo: &Repository, model: usize, mut draft: ModelDraft) -> Result<Self, ManageError> {
        let old = repo.models().get(model).ok_or_else(|| invalid("Model no longer exists; Reload"))?;
        if draft.vendor != old.vendor {
            return Err(invalid("Moving a model to another vendor is not supported"));
        }
        draft.source = Some(model);
        Self::build(repo, Some(model), &mut draft)
    }

    fn build(repo: &Repository, editing: Option<usize>, draft: &mut ModelDraft) -> Result<Self, ManageError> {
        let mut files = ManagementPlan::new(repo)?;
        check_name(&draft.name)?;
        let vendor = repo.vendor(draft.vendor);
        let filename = check_filename(&draft.file_name)?;
        let path = match editing {
            Some(ix) => repo.models()[ix].path.parent().unwrap().join(filename),
            None => Path::new(&vendor.name).join("machine").join(filename),
        };
        let old_path = editing.map(|ix| &repo.models()[ix].path);
        let folded = path.to_string_lossy().to_lowercase();
        if old_path.is_none_or(|p| p.to_string_lossy().to_lowercase() != folded)
            && (files.snapshot.keys().any(|p| p.to_string_lossy().to_lowercase() == folded)
                || files.root.join(&path).exists())
        {
            return Err(invalid("The destination file already exists"));
        }
        let mut manifest = files.document(&vendor.index_path)?;
        let list_key = key_ci(&manifest, "machine_model_list").unwrap_or_else(|| "machine_model_list".into());
        if !manifest.contains(&list_key) {
            manifest.set(&list_key, &RawValue::List(Vec::new()));
        }
        let entries = array(&manifest, &list_key)?;
        let old_entry = editing.map(|ix| model_entry(repo, ix, &entries)).transpose()?;
        if entries
            .iter()
            .enumerate()
            .any(|(ix, e)| Some(ix) != old_entry && field_ci(e, "name") == Some(draft.name.as_str()))
        {
            return Err(invalid("The model name already exists in this vendor's manifest"));
        }
        if let Some(ix) = editing {
            let old = &repo.models()[ix];
            if repo.models().iter().filter(|m| m.vendor == old.vendor && m.id == old.id).count() != 1 {
                return Err(invalid("The source model name is ambiguous in this vendor"));
            }
        }
        let mut doc = match draft.source {
            Some(ix) => files.document(&repo.models()[ix].path)?,
            None => Document::parse("{\n}\n")?,
        };
        set_meta(&mut doc, "type", Some("machine_model"));
        set_meta(&mut doc, "name", Some(&draft.name));
        for (key, value) in &draft.fields {
            if !ModelDraft::FIELDS.contains(&key.as_str()) {
                return Err(invalid(format!("Unsupported model field {key}")));
            }
            let normalized = if ["nozzle_diameter", "default_materials"].contains(&key.as_str()) {
                plain_list(value, key)?.join(";")
            } else {
                value.trim().to_owned()
            };
            // A cleared field is removed, as the loader treats an absent key; an empty string
            // would only leave `"key": ""` behind.
            set_meta(&mut doc, key, (!normalized.is_empty()).then_some(normalized.as_str()));
        }
        let model_id = string(&doc, "model_id")?;
        if !model_id.is_empty()
            && repo.models().iter().enumerate().any(|(ix, m)| {
                Some(ix) != editing && m.vendor == draft.vendor && m.model_id.as_deref() == Some(&model_id)
            })
        {
            files.notice(true, format!("model_id {model_id:?} is already used by another model in {}", vendor.name));
        }
        if model_id.is_empty() {
            // Existing models may lack one (OrcaSlicer loads them), but a new model needs its own.
            files.notice(
                editing.is_none(),
                "model_id is empty; enter a machine identifier that no other model of this vendor uses.",
            );
        }
        let nozzle = string(&doc, "nozzle_diameter")?;
        let variants = plain_list(&nozzle, "nozzle_diameter")?;
        if variants.is_empty() {
            files.notice(true, "nozzle_diameter must contain at least one nozzle variant.");
        }
        if string(&doc, "machine_tech")?.starts_with("SL") {
            files.notice(true, "SLA machine models are outside this tool's scope.");
        }
        let materials = plain_list(&string(&doc, "default_materials")?, "default_materials")?;
        for material in materials {
            if repo.find(PresetKind::Filament, &material).is_empty() {
                files.notice(
                    false,
                    format!("default_materials names {material:?}, which is not a loaded filament preset."),
                );
            }
        }
        if let Some(ix) = editing {
            let old = &repo.models()[ix];
            for preset in repo.presets().iter().filter(|p| p.vendor == old.vendor && p.kind == PresetKind::Machine) {
                if repo.lookup_scalar(preset.id, "printer_model") == Some(&old.id) {
                    let variant = repo.lookup_scalar(preset.id, "printer_variant").unwrap_or("");
                    // Templates may name a model without choosing a nozzle. Only removal of an
                    // existing valid variant creates a new conflict; preserve baseline issues.
                    if old.variants.iter().any(|v| v == variant) && !variants.iter().any(|v| v == variant) {
                        files.notice(
                            true,
                            format!(
                                "{} uses printer_variant {variant:?}, which is absent from the new model variants.",
                                preset.path.display()
                            ),
                        );
                    }
                }
                if draft.name != old.id
                    && preset.params.get("printer_model").is_some_and(|v| match v {
                        RawValue::Str(v) => v == &old.id,
                        RawValue::List(v) => v.contains(&old.id),
                    })
                {
                    let mut printer = files.document(&preset.path)?;
                    let value = match preset.params.get("printer_model").unwrap() {
                        RawValue::Str(_) => RawValue::Str(draft.name.clone()),
                        RawValue::List(items) => RawValue::List(
                            items.iter().map(|v| if v == &old.id { draft.name.clone() } else { v.clone() }).collect(),
                        ),
                    };
                    printer.set("printer_model", &value);
                    files.put(preset.path.clone(), Some(printer.into_text()))?;
                    files.notice(
                        false,
                        format!("Update printer_model in {} (inheriting children follow it).", preset.path.display()),
                    );
                }
            }
            if old.path != path {
                files.put(old.path.clone(), None)?;
                files.moved.insert(old.path.clone(), path.clone());
            }
        }
        let sub = path.strip_prefix(&vendor.name).map_err(|_| invalid("Invalid vendor path"))?.to_string_lossy();
        if let Some(ix) = old_entry {
            let name_key = object_key(&entries[ix], "name").unwrap();
            let path_key = object_key(&entries[ix], "sub_path").unwrap();
            manifest.array_set_member(&list_key, ix, &name_key, &RawValue::Str(draft.name.clone()))?;
            manifest.array_set_member(&list_key, ix, &path_key, &RawValue::Str(sub.into()))?;
        } else {
            manifest.array_insert(&list_key, entries.len(), &json!({"name":draft.name,"sub_path":sub}))?;
        }
        if let Some(ix) = editing {
            let old = &repo.models()[ix];
            let cover = files.root.join(&vendor.name).join(format!("{}_cover.png", old.id));
            if draft.name != old.id && relative_asset(&format!("{}_cover.png", old.id)) && cover.is_file() {
                files.notice(
                    false,
                    format!(
                        "Cover {0}/{1}_cover.png keeps the old name; rename it to {2}_cover.png yourself, \
                         this tool does not move images.",
                        vendor.name, old.id, draft.name
                    ),
                );
            }
        }
        resource_notices(&mut files, &vendor.name, &draft.name, &doc)?;
        files.put(path, Some(doc.into_text()))?;
        files.put(vendor.index_path.clone(), Some(manifest.into_text()))?;
        files.validate(repo)?;
        Ok(Self { files })
    }

    pub fn delete(repo: &Repository, model: usize) -> Result<Self, ManageError> {
        let old = repo.models().get(model).ok_or_else(|| invalid("Model no longer exists; Reload"))?;
        let mut files = ManagementPlan::new(repo)?;
        for preset in repo.presets().iter().filter(|p| p.vendor == old.vendor && p.kind == PresetKind::Machine) {
            if repo.lookup_scalar(preset.id, "printer_model") == Some(&old.id)
                || preset.params.get("printer_model").is_some_and(|v| match v {
                    RawValue::Str(v) => v == &old.id,
                    RawValue::List(v) => v.contains(&old.id),
                })
            {
                files.notice(
                    true,
                    format!(
                        "{} still references model {:?}; move or remove that printer preset first.",
                        preset.path.display(),
                        old.id
                    ),
                );
            }
        }
        let vendor = repo.vendor(old.vendor);
        let mut manifest = files.document(&vendor.index_path)?;
        let key = key_ci(&manifest, "machine_model_list").ok_or_else(|| invalid("Model manifest is missing"))?;
        let entries = array(&manifest, &key)?;
        manifest.array_remove(&key, model_entry(repo, model, &entries)?)?;
        files.put(old.path.clone(), None)?;
        files.put(vendor.index_path.clone(), Some(manifest.into_text()))?;
        files.validate(repo)?;
        Ok(Self { files })
    }
}

fn key_ci(doc: &Document, key: &str) -> Option<String> {
    doc.keys().find(|k| k.eq_ignore_ascii_case(key)).map(str::to_owned)
}
fn object_key(value: &Value, key: &str) -> Option<String> {
    value.as_object()?.keys().find(|k| k.eq_ignore_ascii_case(key)).cloned()
}
fn field_ci<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(object_key(value, key)?).and_then(Value::as_str)
}
fn string(doc: &Document, key: &str) -> Result<String, ManageError> {
    match key_ci(doc, key).and_then(|key| doc.value(&key)) {
        None => Ok(String::new()),
        Some(Value::String(value)) => Ok(value),
        _ => Err(invalid(format!("Model {key} must be a string"))),
    }
}
fn model_entry(repo: &Repository, model: usize, entries: &[Value]) -> Result<usize, ManageError> {
    let model = &repo.models()[model];
    let sub = model.path.strip_prefix(&repo.vendor(model.vendor).name).map_err(|_| invalid("Invalid model path"))?;
    let matching = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| field_ci(e, "sub_path") == sub.to_str())
        .map(|(ix, _)| ix)
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return Err(invalid("Model file must have exactly one manifest entry; Reload and review it"));
    }
    Ok(matching[0])
}

/// The form accepts the ordinary semicolon-separated syntax used by the system profiles. Reject
/// quoted/escaped input rather than claiming the loader's simplified split supports full C syntax.
fn plain_list(value: &str, field: &str) -> Result<Vec<String>, ManageError> {
    if value.contains(['"', '\\']) || value.chars().any(char::is_control) {
        return Err(invalid(format!("Use plain semicolon-separated values for {field}, without quotes or escapes")));
    }
    Ok(value.split(';').map(str::trim).filter(|v| !v.is_empty()).map(str::to_owned).collect())
}
fn relative_asset(name: &str) -> bool {
    !name.is_empty() && !name.contains('\\') && Path::new(name).components().all(|c| matches!(c, Component::Normal(_)))
}
fn resource_notices(files: &mut ManagementPlan, vendor: &str, name: &str, doc: &Document) -> Result<(), ManageError> {
    for key in ["bed_model", "bed_texture", "hotend_model"] {
        let asset = string(doc, key)?;
        if asset.is_empty() {
            continue;
        }
        if !relative_asset(&asset) {
            files.notice(true, format!("{key} must be a relative resource path inside the vendor directory."));
            continue;
        }
        if !files.root.join(vendor).join(&asset).is_file() {
            let fallback =
                if key == "hotend_model" { " OrcaSlicer also falls back to profiles/hotend.stl." } else { "" };
            files.notice(false, format!("{key}: {vendor}/{asset} is absent from this profiles directory. OrcaSlicer first checks its user data vendor directory.{fallback}"));
        }
    }
    let cover = format!("{name}_cover.png");
    if !relative_asset(&cover) || !files.root.join(vendor).join(&cover).is_file() {
        let fallback = files.root.parent().map(|resources| resources.join("web/image/printer").join(&cover));
        let exists = relative_asset(&cover) && fallback.as_ref().is_some_and(|p| p.is_file());
        files.notice(false, if exists {
            format!("Cover {vendor}/{cover} is absent; the setup wizard can use resources/web/image/printer/{cover}.")
        } else {
            format!("Cover {vendor}/{cover} is absent, including the setup wizard fallback resources/web/image/printer/{cover}. No image files will be copied or generated.")
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ModelDraft, ModelPlan};
    use crate::{Document, PresetKind, Repository, Severity};
    use std::fs;

    fn fixture() -> (tempfile::TempDir, Repository) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("profiles");
        fs::create_dir_all(root.join("Acme/machine")).unwrap();
        fs::write(root.join("Acme.json"), "\u{feff}{\r\n  \"name\":\"Acme\", \"version\":\"1.0\",\r\n  \"machine_model_list\":[{\"name\":\"A\", \"sub_path\":\"machine/model.json\", \"extra\":\"keep\"}],\r\n  \"machine_list\":[{\"name\":\"base\",\"sub_path\":\"machine/base.json\"},{\"name\":\"A 0.4\",\"sub_path\":\"machine/a.json\"}]\r\n}").unwrap();
        fs::write(root.join("Acme/machine/model.json"), "\u{feff}{\r\n  \"type\":\"machine_model\", \"name\":\"A\", \"model_id\":\"A-ID\",\r\n  \"nozzle_diameter\":\"0.4;0.6\", \"machine_tech\":\"FFF\", \"custom\": {\"keep\":true}\r\n}").unwrap();
        fs::write(
            root.join("Acme/machine/base.json"),
            r#"{"type":"machine","name":"base","from":"system","instantiation":"false","printer_model":[ "A" ]}"#,
        )
        .unwrap();
        fs::write(root.join("Acme/machine/a.json"), r#"{"type":"machine","name":"A 0.4","from":"system","instantiation":"true","inherits":"base","printer_variant":"0.4","nozzle_diameter":["0.4"]}"#).unwrap();
        let repo = load(&root);
        clean(&repo);
        (dir, repo)
    }
    fn load(path: &std::path::Path) -> Repository {
        Repository::load(path).unwrap()
    }
    fn clean(repo: &Repository) {
        assert!(!repo.issues().iter().any(|i| i.severity() >= Severity::Error), "{:?}", repo.issues());
    }
    fn draft(repo: &Repository, name: &str) -> ModelDraft {
        ModelDraft::copy(repo, 0, name, format!("{name}.json")).with_field("model_id", format!("{name}-ID"))
    }

    #[test]
    fn create_copy_edit_delete_models_keep_unknown_fields_and_format() {
        let (_dir, repo) = fixture();
        let manifest = fs::read_to_string(repo.root().join("Acme.json")).unwrap();
        let plan = ModelPlan::create(&repo, draft(&repo, "B")).unwrap();
        assert!(plan.files().is_ready(), "{:?}", plan.files().notices());
        assert!(!repo.root().join("Acme/machine/B.json").exists());
        plan.apply().unwrap();
        let repo = load(repo.root());
        clean(&repo);
        let ix = repo.models().iter().position(|m| m.id == "B").unwrap();
        assert_eq!(repo.models()[ix].variants, ["0.4", "0.6"], "copy preserves source variants");
        let text = fs::read_to_string(repo.root().join("Acme/machine/B.json")).unwrap();
        assert!(text.contains("\"custom\": {\"keep\":true}"));
        assert!(text.starts_with('\u{feff}') && text.contains("\r\n"));
        let edit = ModelDraft::copy(&repo, ix, "B", "B.json").with_field("family", "Family B");
        ModelPlan::update(&repo, ix, edit).unwrap().apply().unwrap();
        let repo = load(repo.root());
        clean(&repo);
        let changed = fs::read_to_string(repo.root().join("Acme/machine/B.json")).unwrap();
        assert!(changed.contains("Family B"));
        ModelPlan::delete(&repo, ix).unwrap().apply().unwrap();
        let repo = load(repo.root());
        clean(&repo);
        assert_eq!(repo.models().len(), 1);
        assert_eq!(fs::read_to_string(repo.root().join("Acme.json")).unwrap(), manifest);
        assert!(!repo.root().join("Acme/machine/B.json").exists());
        let blank = ModelDraft::new(repo.vendors()[0].id, "Blank", "Blank.json").with_field("model_id", "blank");
        ModelPlan::create(&repo, blank).unwrap().apply().unwrap();
        clean(&load(repo.root()));
    }

    #[test]
    fn rename_updates_own_machine_references_and_inherited_children_without_touching_others() {
        let (_dir, repo) = fixture();
        let child = fs::read_to_string(repo.root().join("Acme/machine/a.json")).unwrap();
        let base = fs::read_to_string(repo.root().join("Acme/machine/base.json")).unwrap();
        let before_model = fs::read_to_string(repo.root().join("Acme/machine/model.json")).unwrap();
        let plan = ModelPlan::update(&repo, 0, ModelDraft::copy(&repo, 0, "Renamed", "renamed.json")).unwrap();
        assert_eq!(plan.files().changes().count(), 4);
        plan.apply().unwrap();
        let loaded = load(repo.root());
        clean(&loaded);
        let id = loaded.find(PresetKind::Machine, "A 0.4")[0];
        assert_eq!(loaded.lookup_scalar(id, "printer_model"), Some("Renamed"));
        assert_eq!(fs::read_to_string(repo.root().join("Acme/machine/a.json")).unwrap(), child);
        assert_eq!(
            fs::read_to_string(repo.root().join("Acme/machine/base.json")).unwrap(),
            base.replace("[ \"A\" ]", "[ \"Renamed\" ]")
        );
        assert_eq!(
            fs::read_to_string(repo.root().join("Acme/machine/renamed.json")).unwrap(),
            before_model.replace("\"name\":\"A\"", "\"name\":\"Renamed\"")
        );
        assert!(!repo.root().join("Acme/machine/model.json").exists());
    }

    #[test]
    fn references_variants_and_duplicates_block_writes_and_materials_are_reviewed() {
        let (_dir, repo) = fixture();
        assert!(!ModelPlan::delete(&repo, 0).unwrap().files().is_ready());
        let variant = ModelPlan::update(
            &repo,
            0,
            ModelDraft::copy(&repo, 0, "A", "model.json").with_field("nozzle_diameter", "0.2"),
        )
        .unwrap();
        assert!(!variant.files().is_ready());
        assert!(variant.files().notices().iter().any(|n| n.is_blocking() && n.message().contains("a.json")));
        let empty = ModelPlan::create(&repo, draft(&repo, "B").with_field("nozzle_diameter", " ; ")).unwrap();
        assert!(!empty.files().is_ready());
        let duplicate = ModelPlan::create(&repo, ModelDraft::copy(&repo, 0, "B", "B.json")).unwrap();
        assert!(!duplicate.files().is_ready());
        assert!(ModelPlan::create(&repo, draft(&repo, "A")).is_err());
        let materials =
            ModelPlan::create(&repo, draft(&repo, "B").with_field("default_materials", "Unknown filament")).unwrap();
        assert!(materials.files().is_ready());
        assert!(
            materials.files().notices().iter().any(|n| !n.is_blocking() && n.message().contains("Unknown filament"))
        );
        for field in ["nozzle_diameter", "default_materials"] {
            assert!(ModelPlan::create(&repo, draft(&repo, "B").with_field(field, "\"bad")).is_err());
        }
        assert!(ModelPlan::create(&repo, ModelDraft::new(repo.vendors()[0].id, "B", "../B.json")).is_err());
    }

    #[test]
    fn model_transactions_reject_stale_files_and_roll_back_failed_writes() {
        let (_dir, repo) = fixture();
        let plan = ModelPlan::update(&repo, 0, ModelDraft::copy(&repo, 0, "Renamed", "Renamed.json")).unwrap();
        let snapshot = super::super::transaction::snapshot(repo.root()).unwrap();
        assert!(plan.files.apply_with_failure(Some(2)).is_err());
        assert_eq!(super::super::transaction::snapshot(repo.root()).unwrap(), snapshot);
        fs::write(
            repo.root().join("Acme/machine/model.json"),
            snapshot[&std::path::PathBuf::from("Acme/machine/model.json")].clone() + "\n",
        )
        .unwrap();
        assert!(plan.apply().is_err());
        assert!(ModelPlan::create(&repo, draft(&repo, "B")).is_err());
    }

    #[test]
    fn model_resources_use_vendor_paths_and_wizard_cover_fallback_without_copying() {
        let (dir, repo) = fixture();
        fs::write(repo.root().join("Acme/bed.stl"), "model").unwrap();
        fs::create_dir_all(dir.path().join("web/image/printer")).unwrap();
        fs::write(dir.path().join("web/image/printer/B_cover.png"), "cover").unwrap();
        let plan = ModelPlan::create(
            &repo,
            draft(&repo, "B")
                .with_field("bed_model", "bed.stl")
                .with_field("bed_texture", "missing.svg")
                .with_field("hotend_model", "missing.stl"),
        )
        .unwrap();
        assert!(plan.files().is_ready());
        let notes = plan.files().notices().iter().map(|n| n.message()).collect::<Vec<_>>().join("\n");
        assert!(!notes.contains("bed_model:"));
        assert!(
            notes.contains("missing.svg")
                && notes.contains("profiles/hotend.stl")
                && notes.contains("can use resources/web/image/printer")
        );
        assert!(plan.files().changes().all(|c| c.path().extension().unwrap() == "json"));
        let invalid = ModelPlan::create(&repo, draft(&repo, "B").with_field("bed_model", "../outside.stl")).unwrap();
        assert!(!invalid.files().is_ready());
    }

    #[test]
    fn case_insensitive_manifest_keys_keep_the_original_spelling() {
        let (_dir, repo) = fixture();
        let path = repo.root().join("Acme.json");
        let original = fs::read_to_string(&path)
            .unwrap()
            .replace("machine_model_list", "MACHINE_MODEL_LIST")
            .replace("\"sub_path\":\"machine/model.json\"", "\"SUB_PATH\":\"machine/model.json\"");
        fs::write(&path, &original).unwrap();
        let repo = load(repo.root());
        ModelPlan::update(&repo, 0, ModelDraft::copy(&repo, 0, "A", "MODEL.json")).unwrap().apply().unwrap();
        let manifest = Document::parse(fs::read_to_string(&path).unwrap()).unwrap();
        assert!(manifest.contains("MACHINE_MODEL_LIST"));
        assert_eq!(manifest.value("MACHINE_MODEL_LIST").unwrap()[0]["SUB_PATH"], "machine/MODEL.json");
        clean(&load(repo.root()));
    }

    #[test]
    fn cleared_fields_are_removed_and_new_models_need_an_identifier() {
        let (_dir, repo) = fixture();
        let path = repo.root().join("Acme/machine/model.json");
        let edit = ModelDraft::copy(&repo, 0, "A", "model.json").with_field("machine_tech", "");
        let plan = ModelPlan::update(&repo, 0, edit).unwrap();
        assert!(plan.files().is_ready(), "{:?}", plan.files().notices());
        plan.apply().unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("machine_tech"), "{text}");
        let repo = load(repo.root());
        // A new or copied model without model_id is blocked; an existing one only gets a review notice.
        let plan =
            ModelPlan::create(&repo, ModelDraft::copy(&repo, 0, "C", "C.json").with_field("model_id", "")).unwrap();
        assert!(!plan.files().is_ready());
        let edit = ModelDraft::copy(&repo, 0, "A", "model.json").with_field("model_id", "");
        let plan = ModelPlan::update(&repo, 0, edit).unwrap();
        assert!(plan.files().is_ready(), "{:?}", plan.files().notices());
        assert!(plan.files().notices().iter().any(|n| !n.is_blocking() && n.message().contains("model_id is empty")));
    }

    #[test]
    fn materials_keep_their_file_order_and_rename_points_at_the_old_cover() {
        let (_dir, repo) = fixture();
        let path = repo.root().join("Acme/machine/model.json");
        let text = fs::read_to_string(&path)
            .unwrap()
            .replace("\"machine_tech\"", "\"default_materials\":\"Zeta;Alpha;Zeta\", \"machine_tech\"");
        fs::write(&path, text).unwrap();
        fs::write(repo.root().join("Acme/A_cover.png"), b"png").unwrap();
        let repo = load(repo.root());
        assert_eq!(repo.models()[0].default_materials, ["Alpha", "Zeta"]);
        assert_eq!(repo.models()[0].default_materials_text.as_deref(), Some("Zeta;Alpha;Zeta"));
        let plan = ModelPlan::update(&repo, 0, ModelDraft::copy(&repo, 0, "Renamed", "model.json")).unwrap();
        assert!(
            plan.files().notices().iter().any(|n| n.message().contains("Acme/A_cover.png keeps the old name")),
            "{:?}",
            plan.files().notices()
        );
        assert!(!plan.files().notices().iter().any(|n| n.is_blocking()), "{:?}", plan.files().notices());
    }
}
