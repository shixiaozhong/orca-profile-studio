use std::fs;

use super::{ManagementPlan, NameRule, PresetDraft};
use crate::{Document, PresetKind, RawValue, Repository, Schema, Severity};

fn fixture() -> (tempfile::TempDir, Repository) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("Acme/process")).unwrap();
    fs::create_dir_all(dir.path().join("Acme/machine")).unwrap();
    fs::write(dir.path().join("Acme.json"), "\u{feff}{\r\n  \"name\": \"Acme\",\r\n  \"version\": \"1.0\",\r\n  \"process_list\": [\r\n    { \"name\": \"base 0.4\", \"sub_path\": \"process/base.json\", \"extra\": \"keep\" },\r\n    { \"name\": \"child 0.4\", \"sub_path\": \"process/child.json\" }\r\n  ],\r\n  \"machine_model_list\": [{\"name\":\"A\",\"sub_path\":\"machine/model.json\"}],\r\n  \"machine_list\": [{\"name\":\"A 0.4\",\"sub_path\":\"machine/a.json\"}]\r\n}").unwrap();
    fs::write(dir.path().join("Acme/process/base.json"), "{\n  \"type\": \"process\",\n  \"name\": \"base 0.4\",\n  \"from\": \"system\",\n  \"instantiation\": \"false\",\n  \"layer_height\": \"0.2\",\n  \"wall_loops\": \"2\"\n}\n").unwrap();
    fs::write(dir.path().join("Acme/process/child.json"), r#"{"type":"process", "name":"child 0.4", "from":"system", "instantiation":"true", "inherits":"base 0.4", "setting_id":"OLD", "compatible_printers":[ "A 0.4" ]}"#).unwrap();
    fs::write(
        dir.path().join("Acme/machine/model.json"),
        r#"{"type":"machine_model","name":"A","nozzle_diameter":"0.4"}"#,
    )
    .unwrap();
    fs::write(dir.path().join("Acme/machine/a.json"), r#"{"TYPE":"machine", "NAME":"A 0.4", "from":"system", "instantiation":"true", "printer_model":"A", "printer_variant":"0.4", "nozzle_diameter":["0.4"]}"#).unwrap();
    let repo = Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap();
    assert!(!repo.issues().iter().any(|i| i.severity() >= Severity::Error));
    (dir, repo)
}

fn clean(repo: &Repository) {
    assert!(!repo.issues().iter().any(|i| i.severity() >= Severity::Error), "{:?}", repo.issues());
}

#[test]
fn copy_group_keeps_parent_order_and_only_changes_requested_values() {
    let (dir, repo) = fixture();
    let base = repo.find(PresetKind::Process, "base 0.4")[0];
    let child = repo.find(PresetKind::Process, "child 0.4")[0];
    let original = fs::read_to_string(dir.path().join("Acme/process/child.json")).unwrap();
    let plan = ManagementPlan::copy(
        &repo,
        vec![
            PresetDraft::copy(&repo, child, "child 0.2", "child02.json")
                .with_value("layer_height", RawValue::Str("0.1".into())),
            PresetDraft::copy(&repo, base, "base 0.2", "base02.json"),
        ],
        None,
    )
    .unwrap();
    assert!(plan.is_ready(), "{:?}", plan.notices());
    assert!(!dir.path().join("Acme/process/child02.json").exists(), "preview is readonly");
    plan.apply().unwrap();
    let loaded = Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap();
    clean(&loaded);
    let new_child = loaded.find(PresetKind::Process, "child 0.2")[0];
    let new_base = loaded.find(PresetKind::Process, "base 0.2")[0];
    assert_eq!(loaded.preset(new_child).parent, Some(new_base));
    assert!(new_base.index() < new_child.index());
    assert_eq!(loaded.lookup_scalar(new_child, "layer_height"), Some("0.1"));
    assert!(loaded.preset(new_child).meta.setting_id.is_none());
    assert_eq!(fs::read_to_string(dir.path().join("Acme/process/child.json")).unwrap(), original);
    let manifest = fs::read_to_string(dir.path().join("Acme.json")).unwrap();
    assert!(manifest.starts_with('\u{feff}') && manifest.contains("\r\n"));
    assert!(manifest.contains("{ \"name\": \"base 0.4\", \"sub_path\": \"process/base.json\", \"extra\": \"keep\" }"));
}

#[test]
fn single_copy_can_inherit_source_or_its_original_parent() {
    let (_dir, repo) = fixture();
    let child = repo.find(PresetKind::Process, "child 0.4")[0];
    for inherit in [false, true] {
        let plan = ManagementPlan::copy(
            &repo,
            vec![PresetDraft::copy(&repo, child, "copy", "copy.json").inherit_source(inherit).keep_identifiers(true)],
            None,
        )
        .unwrap();
        assert!(plan.is_ready());
        let changed = plan.changes().find(|c| c.path().ends_with("copy.json")).unwrap();
        let doc = Document::parse(changed.after().unwrap()).unwrap();
        assert_eq!(doc.value("inherits").unwrap(), if inherit { "child 0.4" } else { "base 0.4" });
        assert_eq!(doc.value("setting_id").unwrap(), "OLD");
        assert!(plan.notices().iter().any(|n| n.message().contains("retained")));
    }
}

#[test]
fn creates_new_preset_from_parent_and_rejects_invalid_values_and_collisions() {
    let (_dir, repo) = fixture();
    let base = repo.find(PresetKind::Process, "base 0.4")[0];
    let vendor = repo.preset(base).vendor;
    let plan = ManagementPlan::create(
        &repo,
        PresetDraft::new(vendor, PresetKind::Process, "new", "new.json").with_parent(base),
    )
    .unwrap();
    assert!(plan.is_ready(), "{:?}", plan.notices());
    for draft in [
        PresetDraft::new(vendor, PresetKind::Process, "bad", "bad.json")
            .with_value("wall_loops", RawValue::Str("bad".into())),
        PresetDraft::copy(&repo, base, "base 0.4", "other.json"),
        PresetDraft::copy(&repo, base, "other", "base.json"),
        PresetDraft::copy(&repo, base, "other", "../escape.json"),
    ] {
        assert!(!ManagementPlan::create(&repo, draft).unwrap().is_ready());
    }
    assert!(NameRule::new("missing", "new").replace("old").is_err());
}

#[test]
fn rename_updates_machine_references_and_preserves_unrelated_json_bytes() {
    let (dir, repo) = fixture();
    let printer = repo.find(PresetKind::Machine, "A 0.4")[0];
    let before = fs::read_to_string(dir.path().join("Acme/process/child.json")).unwrap();
    let plan = ManagementPlan::rename(&repo, printer, "New A 0.4", "new-a.json", true).unwrap();
    assert!(plan.is_ready(), "{:?}", plan.notices());
    plan.apply().unwrap();
    let loaded = Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap();
    clean(&loaded);
    assert!(!dir.path().join("Acme/machine/a.json").exists());
    let renamed = fs::read_to_string(dir.path().join("Acme/machine/new-a.json")).unwrap();
    assert!(renamed.contains("\"NAME\":\"New A 0.4\""));
    assert!(renamed.contains("renamed_from"));
    assert_eq!(
        fs::read_to_string(dir.path().join("Acme/process/child.json")).unwrap(),
        before.replace("\"A 0.4\"", "\"New A 0.4\"")
    );
}

#[test]
fn rename_parent_updates_inherits_and_deletion_requires_descendants() {
    let (dir, repo) = fixture();
    let base = repo.find(PresetKind::Process, "base 0.4")[0];
    assert!(!ManagementPlan::delete(&repo, &[base]).unwrap().is_ready());
    let plan = ManagementPlan::rename(&repo, base, "base renamed", "newbase.json", false).unwrap();
    assert!(plan.is_ready(), "{:?}", plan.notices());
    plan.apply().unwrap();
    let repo = Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap();
    clean(&repo);
    let child = repo.find(PresetKind::Process, "child 0.4")[0];
    assert_eq!(repo.preset(child).meta.inherits(), Some("base renamed"));
    let base = repo.find(PresetKind::Process, "base renamed")[0];
    let plan = ManagementPlan::delete(&repo, &[base, child]).unwrap();
    assert!(plan.is_ready());
    plan.apply().unwrap();
    clean(&Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap());
}

#[test]
fn stale_preview_and_partial_io_failure_never_leave_half_a_change_set() {
    let (dir, repo) = fixture();
    let child = repo.find(PresetKind::Process, "child 0.4")[0];
    let plan = ManagementPlan::rename(&repo, child, "renamed", "renamed.json", false).unwrap();
    let original = super::transaction::snapshot(dir.path()).unwrap();
    for step in 1..plan.changes().count() {
        assert!(plan.apply_with_failure(Some(step)).is_err());
        assert_eq!(super::transaction::snapshot(dir.path()).unwrap(), original);
    }
    assert!(!dir.path().join(".orca-profile-studio.lock").exists());
    fs::write(dir.path().join("Acme/process/child.json"), "{}").unwrap();
    assert!(plan.apply().unwrap_err().to_string().contains("changed since the preview"));
    assert!(!dir.path().join("Acme/process/renamed.json").exists());
}

#[test]
fn deletion_removes_name_list_references_and_copy_can_rewrite_arrays() {
    let (dir, repo) = fixture();
    let child = repo.find(PresetKind::Process, "child 0.4")[0];
    let plan = ManagementPlan::copy(
        &repo,
        vec![PresetDraft::copy(&repo, child, "copied", "copied.json")],
        Some(NameRule::new("A 0.4", "A 0.2").rewrite_array("compatible_printers")),
    )
    .unwrap();
    assert!(plan.is_ready());
    let copied = plan.changes().find(|change| change.path().ends_with("copied.json")).unwrap();
    assert_eq!(
        Document::parse(copied.after().unwrap()).unwrap().value("compatible_printers").unwrap(),
        serde_json::json!(["A 0.2"])
    );
    // Emptying compatible_printers would make the child match every printer: blocked.
    let printer = repo.find(PresetKind::Machine, "A 0.4")[0];
    let plan = ManagementPlan::delete(&repo, &[printer]).unwrap();
    assert!(!plan.is_ready());
    assert!(plan.notices().iter().any(|n| n.is_blocking() && n.message().contains("empties compatible_printers")));
    assert!(plan.apply().is_err());

    let plan = ManagementPlan::delete(&repo, &[printer, child]).unwrap();
    assert!(plan.is_ready(), "{:?}", plan.notices());
    plan.apply().unwrap();
    let loaded = Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap();
    clean(&loaded);
    assert!(loaded.find(PresetKind::Process, "child 0.4").is_empty());
    assert!(loaded.find(PresetKind::Machine, "A 0.4").is_empty());
}

#[test]
fn rename_keeps_existing_issues_and_allows_case_only_file_renames() {
    let (dir, _) = fixture();
    // An unknown key is an existing warning of the child, unrelated to its name.
    let path = dir.path().join("Acme/process/child.json");
    let text = fs::read_to_string(&path).unwrap().replace("\"setting_id\"", "\"not_a_key\":\"1\", \"setting_id\"");
    fs::write(&path, text).unwrap();
    let repo = Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap();
    let child = repo.find(PresetKind::Process, "child 0.4")[0];
    assert!(
        repo.issues().iter().any(|issue| issue.path.ends_with("child.json") && issue.severity() == Severity::Warning)
    );
    let plan = ManagementPlan::rename(&repo, child, "child 0.4", "Child.json", false).unwrap();
    assert!(plan.is_ready(), "{:?}", plan.notices());
    assert!(
        plan.notices().iter().all(|notice| !notice.message().contains("not_a_key")),
        "an existing warning is not reported as new: {:?}",
        plan.notices()
    );
    plan.apply().unwrap();
    let names: Vec<_> =
        fs::read_dir(dir.path().join("Acme/process")).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert!(names.iter().any(|name| name == "Child.json"), "{names:?}");
    assert!(!names.iter().any(|name| name == "child.json"), "{names:?}");
    clean(&Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap());
}

#[test]
fn a_file_changed_after_loading_is_stale_even_when_values_match() {
    let (dir, repo) = fixture();
    let child = repo.find(PresetKind::Process, "child 0.4")[0];
    // Only formatting changes: the old value comparison accepted this, the fingerprint does not.
    let path = dir.path().join("Acme.json");
    fs::write(&path, fs::read_to_string(&path).unwrap().replace("\r\n", "\n")).unwrap();
    let err = ManagementPlan::copy(&repo, vec![PresetDraft::copy(&repo, child, "copy", "copy.json")], None);
    assert!(err.err().unwrap().to_string().contains("changed on disk"));
}

#[test]
fn copying_library_and_vendor_parents_preserves_cross_vendor_inheritance() {
    let (dir, _) = fixture();
    fs::create_dir_all(dir.path().join("OrcaFilamentLibrary/filament")).unwrap();
    fs::create_dir_all(dir.path().join("Acme/filament")).unwrap();
    fs::write(
        dir.path().join("OrcaFilamentLibrary.json"),
        r#"{"name":"OrcaFilamentLibrary","version":"1.0","filament_list":[]}"#,
    )
    .unwrap();
    fs::write(dir.path().join("OrcaFilamentLibrary/filament/base.json"), r#"{"type":"filament","name":"Library base","from":"system","instantiation":"false","filament_id":"GF","nozzle_temperature":["210"]}"#).unwrap();
    fs::write(dir.path().join("Acme/filament/child.json"), r#"{"type":"filament","name":"Vendor PLA","from":"system","instantiation":"true","inherits":"Library base","compatible_printers":["A 0.4"]}"#).unwrap();
    let mut manifest = Document::parse(fs::read_to_string(dir.path().join("Acme.json")).unwrap()).unwrap();
    manifest
        .array_insert("filament_list", 0, &serde_json::json!({"name":"Vendor PLA","sub_path":"filament/child.json"}))
        .unwrap();
    fs::write(dir.path().join("Acme.json"), manifest.into_text()).unwrap();
    let repo = Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap();
    clean(&repo);
    let base = repo.find(PresetKind::Filament, "Library base")[0];
    let child = repo.find(PresetKind::Filament, "Vendor PLA")[0];
    let plan = ManagementPlan::copy(
        &repo,
        vec![
            PresetDraft::copy(&repo, child, "Vendor PLA copy", "copy.json"),
            PresetDraft::copy(&repo, base, "Library base copy", "base-copy.json").keep_identifiers(true),
        ],
        None,
    )
    .unwrap();
    assert!(plan.is_ready(), "{:?}", plan.notices());
    plan.apply().unwrap();
    let repo = Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap();
    clean(&repo);
    let base = repo.find(PresetKind::Filament, "Library base copy")[0];
    let child = repo.find(PresetKind::Filament, "Vendor PLA copy")[0];
    assert_eq!(repo.preset(child).parent, Some(base));
    assert_eq!(repo.filament_id(child), Some("GF"));
    let manifest = Document::parse(fs::read_to_string(dir.path().join("OrcaFilamentLibrary.json")).unwrap()).unwrap();
    assert_eq!(manifest.value("filament_list").unwrap().as_array().unwrap().len(), 2);
}

#[test]
fn stale_manifest_order_is_rejected_before_creating_a_plan() {
    let (dir, repo) = fixture();
    let child = repo.find(PresetKind::Process, "child 0.4")[0];
    let path = dir.path().join("Acme.json");
    let mut doc = Document::parse(fs::read_to_string(&path).unwrap()).unwrap();
    let entries = doc.value("process_list").unwrap();
    doc.array_set("process_list", 0, &entries[1]).unwrap();
    doc.array_set("process_list", 1, &entries[0]).unwrap();
    fs::write(&path, doc.into_text()).unwrap();
    assert!(ManagementPlan::copy(&repo, vec![PresetDraft::copy(&repo, child, "copy", "copy.json")], None).is_err());
}

#[test]
fn rename_and_delete_update_scalar_defaults_and_model_material_lists() {
    let (dir, _) = fixture();
    let file = dir.path().join("Acme/machine/a.json");
    let mut doc = Document::parse(fs::read_to_string(&file).unwrap()).unwrap();
    doc.set("default_print_profile", &RawValue::Str("child 0.4".into()));
    fs::write(&file, doc.into_text()).unwrap();
    fs::create_dir_all(dir.path().join("Acme/filament")).unwrap();
    fs::write(dir.path().join("Acme/filament/pla.json"),r#"{"type":"filament","name":"Acme PLA","from":"system","instantiation":"true","filament_id":"GF","compatible_printers":["A 0.4"]}"#).unwrap();
    let manifest = dir.path().join("Acme.json");
    let mut doc = Document::parse(fs::read_to_string(&manifest).unwrap()).unwrap();
    doc.array_insert("filament_list", 0, &serde_json::json!({"name":"Acme PLA","sub_path":"filament/pla.json"}))
        .unwrap();
    fs::write(&manifest, doc.into_text()).unwrap();
    let model = dir.path().join("Acme/machine/model.json");
    let mut doc = Document::parse(fs::read_to_string(&model).unwrap()).unwrap();
    doc.set("default_materials", &RawValue::Str("Acme PLA;Other material".into()));
    fs::write(&model, doc.into_text()).unwrap();
    let repo = Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap();
    clean(&repo);
    let child = repo.find(PresetKind::Process, "child 0.4")[0];
    ManagementPlan::rename(&repo, child, "new process", "new-process.json", false).unwrap().apply().unwrap();
    let repo = Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap();
    assert_eq!(
        repo.lookup_scalar(repo.find(PresetKind::Machine, "A 0.4")[0], "default_print_profile"),
        Some("new process")
    );
    let filament = repo.find(PresetKind::Filament, "Acme PLA")[0];
    ManagementPlan::rename(&repo, filament, "Renamed PLA", "renamed-pla.json", false).unwrap().apply().unwrap();
    let doc = Document::parse(fs::read_to_string(&model).unwrap()).unwrap();
    assert_eq!(doc.value("default_materials").unwrap(), "Renamed PLA;Other material");
    let repo = Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap();
    let filament = repo.find(PresetKind::Filament, "Renamed PLA")[0];
    ManagementPlan::delete(&repo, &[filament]).unwrap().apply().unwrap();
    let doc = Document::parse(fs::read_to_string(&model).unwrap()).unwrap();
    assert_eq!(doc.value("default_materials").unwrap(), "Other material");
    clean(&Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap());
}

#[cfg(unix)]
#[test]
fn managing_through_a_symbolic_link_is_refused() {
    let (dir, repo) = fixture();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("file.json"), "{}").unwrap();
    std::os::unix::fs::symlink(outside.path().join("file.json"), dir.path().join("Acme/process/link.json")).unwrap();
    let child = repo.find(PresetKind::Process, "child 0.4")[0];
    assert!(ManagementPlan::delete(&repo, &[child]).is_err());
    assert_eq!(fs::read_to_string(outside.path().join("file.json")).unwrap(), "{}");
}
