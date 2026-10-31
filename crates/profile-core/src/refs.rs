//! Where presets and printer models are referenced by name (M19).
//!
//! References are found in effective values: a child that inherits `compatible_printers` refers to
//! the same printers as the ancestor that sets it. [`Referrer::Preset::set_by`] tells the two apart;
//! renaming a preset only rewrites the files that set the value themselves.

use crate::model::{PresetId, PresetKind, RawValue};
use crate::repository::{Origin, Repository};

/// Something that names a preset or a printer model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Referrer {
    /// A preset whose effective `key` names the target; `set_by` is the layer that sets that value,
    /// the preset itself or an ancestor it inherits it from.
    Preset { id: PresetId, key: &'static str, set_by: PresetId },
    /// A printer model whose `default_materials` names the target filament (an index into
    /// [`Repository::models`]).
    Model(usize),
}

impl Referrer {
    /// Whether the referring file sets the value itself rather than inheriting it.
    pub fn is_own(&self) -> bool {
        match self {
            Referrer::Preset { id, set_by, .. } => id == set_by,
            Referrer::Model(_) => true,
        }
    }
}

/// Everything that refers to one preset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct References {
    /// Presets whose parent this is, in load order.
    pub children: Vec<PresetId>,
    /// All presets that inherit from this one, directly or not.
    pub descendants: usize,
    /// Name references in load order: presets first, then printer models.
    pub referrers: Vec<Referrer>,
    /// Another preset of the same kind has the same name, so a name reference may mean either.
    pub ambiguous: bool,
}

/// The keys of `referrer` presets that name a preset of `target` kind.
fn reference_keys(target: PresetKind, referrer: PresetKind) -> &'static [&'static str] {
    match (target, referrer) {
        (PresetKind::Machine, PresetKind::Filament | PresetKind::Process) => &["compatible_printers"],
        (PresetKind::Process, PresetKind::Filament) => &["compatible_prints"],
        (PresetKind::Process, PresetKind::Machine) => &["default_print_profile"],
        (PresetKind::Filament, PresetKind::Machine) => &["default_filament_profile"],
        _ => &[],
    }
}

fn names(value: &RawValue, name: &str) -> bool {
    match value {
        RawValue::Str(item) => item == name,
        RawValue::List(items) => items.iter().any(|item| item == name),
    }
}

impl Repository {
    /// The children, descendants and name references of a preset, from the loaded files.
    ///
    /// Only selectable presets can be named: OrcaSlicer keeps templates out of its preset
    /// collections, and each vendor has templates of the same name.
    pub fn references(&self, id: PresetId) -> References {
        let target = self.preset(id);
        let name = target.name();
        let named = self.is_selectable(id);
        let same_name = self.find(target.kind, name).iter().filter(|&&other| self.is_selectable(other)).count();
        let mut refs = References { ambiguous: named && same_name > 1, ..Default::default() };
        for preset in self.presets() {
            if preset.parent == Some(id) {
                refs.children.push(preset.id);
            }
            if preset.id != id && self.chain(preset.id)[1..].contains(&id) {
                refs.descendants += 1;
            }
            if !named {
                continue;
            }
            for &key in reference_keys(target.kind, preset.kind) {
                if let Some(resolved) = self.lookup(preset.id, key)
                    && names(resolved.value, name)
                {
                    let Origin::Preset(set_by) = resolved.origin;
                    refs.referrers.push(Referrer::Preset { id: preset.id, key, set_by });
                }
            }
        }
        if named && target.kind == PresetKind::Filament {
            let models = self.models().iter().enumerate();
            refs.referrers.extend(
                models
                    .filter(|(_, model)| model.default_materials.iter().any(|material| material == name))
                    .map(|(ix, _)| Referrer::Model(ix)),
            );
        }
        refs
    }

    /// The printers of the model's vendor whose effective `printer_model` names it (an index into
    /// [`Repository::models`]), in load order.
    pub fn model_references(&self, model: usize) -> Vec<Referrer> {
        let model = &self.models()[model];
        self.presets()
            .iter()
            .filter(|preset| preset.kind == PresetKind::Machine && preset.vendor == model.vendor)
            .filter_map(|preset| {
                let resolved = self.lookup(preset.id, "printer_model")?;
                let Origin::Preset(set_by) = resolved.origin;
                (resolved.value.as_scalar() == Some(model.id.as_str())).then_some(Referrer::Preset {
                    id: preset.id,
                    key: "printer_model",
                    set_by,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::{Value, json};

    use super::*;

    fn write(dir: &std::path::Path, path: &str, value: Value) {
        let full = dir.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    }

    fn preset(kind: &str, name: &str, inherits: &str, instantiation: bool, extra: Value) -> Value {
        let mut value = json!({
            "type": kind, "name": name, "from": "system", "inherits": inherits,
            "instantiation": instantiation.to_string(),
        });
        value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        value
    }

    fn entry(name: &str, sub_path: &str) -> Value {
        json!({ "name": name, "sub_path": sub_path })
    }

    /// A library base filament with an Acme child, and an Acme printer that is listed by an own
    /// value, an inherited value and a process.
    fn fixture() -> (tempfile::TempDir, Repository) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "OrcaFilamentLibrary.json",
            json!({ "name": "OrcaFilamentLibrary", "version": "01.00.00.00",
                    "filament_list": [entry("fdm_filament_common", "filament/fdm_filament_common.json")] }),
        );
        write(
            root,
            "OrcaFilamentLibrary/filament/fdm_filament_common.json",
            preset("filament", "fdm_filament_common", "", false, json!({})),
        );
        write(
            root,
            "Acme.json",
            json!({
                "name": "Acme", "version": "01.00.00.00",
                "machine_model_list": [entry("Acme One", "machine/Acme One.json")],
                "process_list": [
                    entry("fdm_process_common", "process/fdm_process_common.json"),
                    entry("0.20mm Standard @Acme", "process/standard.json"),
                ],
                "filament_list": [
                    entry("Acme PLA base", "filament/base.json"),
                    entry("Acme PLA", "filament/pla.json"),
                    entry("Acme PLA Silk", "filament/silk.json"),
                ],
                "machine_list": [
                    entry("fdm_machine_common", "machine/fdm_machine_common.json"),
                    entry("Acme One 0.4 nozzle", "machine/one.json"),
                    entry("Acme One 0.6 nozzle", "machine/one6.json"),
                ],
            }),
        );
        write(
            root,
            "Acme/machine/Acme One.json",
            json!({ "type": "machine_model", "name": "Acme One", "nozzle_diameter": "0.4;0.6",
                    "default_materials": "Acme PLA;Acme PLA Silk" }),
        );
        let printer = "Acme One 0.4 nozzle";
        write(
            root,
            "Acme/process/fdm_process_common.json",
            preset("process", "fdm_process_common", "", false, json!({})),
        );
        write(
            root,
            "Acme/process/standard.json",
            preset(
                "process",
                "0.20mm Standard @Acme",
                "fdm_process_common",
                true,
                json!({ "compatible_printers": [printer] }),
            ),
        );
        write(
            root,
            "Acme/filament/base.json",
            preset(
                "filament",
                "Acme PLA base",
                "fdm_filament_common",
                false,
                json!({ "compatible_printers": [printer] }),
            ),
        );
        write(root, "Acme/filament/pla.json", preset("filament", "Acme PLA", "Acme PLA base", true, json!({})));
        write(
            root,
            "Acme/filament/silk.json",
            preset("filament", "Acme PLA Silk", "Acme PLA base", true, json!({ "compatible_printers": [] })),
        );
        write(
            root,
            "Acme/machine/fdm_machine_common.json",
            preset("machine", "fdm_machine_common", "", false, json!({ "printer_model": "Acme One" })),
        );
        write(
            root,
            "Acme/machine/one.json",
            preset(
                "machine",
                printer,
                "fdm_machine_common",
                true,
                json!({ "printer_variant": "0.4", "default_print_profile": "0.20mm Standard @Acme",
                        "default_filament_profile": ["Acme PLA"] }),
            ),
        );
        write(
            root,
            "Acme/machine/one6.json",
            preset(
                "machine",
                "Acme One 0.6 nozzle",
                printer,
                true,
                json!({ "printer_variant": "0.6", "printer_model": "Acme One" }),
            ),
        );
        let repo = Repository::load(root).unwrap();
        (dir, repo)
    }

    fn id(repo: &Repository, kind: PresetKind, name: &str) -> PresetId {
        repo.find(kind, name)[0]
    }

    #[test]
    fn a_printer_is_referenced_by_own_and_inherited_lists_and_by_children() {
        let (_dir, repo) = fixture();
        let printer = id(&repo, PresetKind::Machine, "Acme One 0.4 nozzle");
        let base = id(&repo, PresetKind::Filament, "Acme PLA base");
        let pla = id(&repo, PresetKind::Filament, "Acme PLA");
        let process = id(&repo, PresetKind::Process, "0.20mm Standard @Acme");
        let refs = repo.references(printer);
        let key = "compatible_printers";
        assert_eq!(
            refs.referrers,
            [
                Referrer::Preset { id: process, key, set_by: process },
                Referrer::Preset { id: base, key, set_by: base },
                Referrer::Preset { id: pla, key, set_by: base },
            ]
        );
        assert!(refs.referrers[1].is_own() && !refs.referrers[2].is_own());
        // The silk filament's own empty list replaces the inherited one.
        assert_eq!(refs.children, [id(&repo, PresetKind::Machine, "Acme One 0.6 nozzle")]);
        assert_eq!(refs.descendants, 1);
        assert!(!refs.ambiguous);
    }

    #[test]
    fn processes_and_filaments_are_referenced_by_defaults_and_models() {
        let (_dir, repo) = fixture();
        let printer = id(&repo, PresetKind::Machine, "Acme One 0.4 nozzle");
        let child = id(&repo, PresetKind::Machine, "Acme One 0.6 nozzle");
        let process = repo.references(id(&repo, PresetKind::Process, "0.20mm Standard @Acme"));
        let key = "default_print_profile";
        assert_eq!(
            process.referrers,
            [
                Referrer::Preset { id: printer, key, set_by: printer },
                Referrer::Preset { id: child, key, set_by: printer },
            ]
        );
        let pla = repo.references(id(&repo, PresetKind::Filament, "Acme PLA"));
        let key = "default_filament_profile";
        assert_eq!(
            pla.referrers,
            [
                Referrer::Preset { id: printer, key, set_by: printer },
                Referrer::Preset { id: child, key, set_by: printer },
                Referrer::Model(0),
            ]
        );
        assert_eq!(repo.references(id(&repo, PresetKind::Filament, "Acme PLA Silk")).referrers, [Referrer::Model(0)]);
    }

    #[test]
    fn library_children_of_other_vendors_count_as_descendants() {
        let (_dir, repo) = fixture();
        let refs = repo.references(id(&repo, PresetKind::Filament, "fdm_filament_common"));
        assert_eq!(refs.children, [id(&repo, PresetKind::Filament, "Acme PLA base")]);
        assert_eq!(refs.descendants, 3);
    }

    #[test]
    fn templates_have_children_but_no_name_references() {
        let (_dir, repo) = fixture();
        let common = id(&repo, PresetKind::Machine, "fdm_machine_common");
        let refs = repo.references(common);
        assert_eq!(refs.children, [id(&repo, PresetKind::Machine, "Acme One 0.4 nozzle")]);
        assert_eq!(refs.descendants, 2);
        assert!(refs.referrers.is_empty() && !refs.ambiguous);
    }

    #[test]
    fn a_model_is_referenced_by_own_and_inherited_printer_model() {
        let (_dir, repo) = fixture();
        let common = id(&repo, PresetKind::Machine, "fdm_machine_common");
        let printer = id(&repo, PresetKind::Machine, "Acme One 0.4 nozzle");
        let child = id(&repo, PresetKind::Machine, "Acme One 0.6 nozzle");
        let key = "printer_model";
        assert_eq!(
            repo.model_references(0),
            [
                Referrer::Preset { id: common, key, set_by: common },
                Referrer::Preset { id: printer, key, set_by: common },
                Referrer::Preset { id: child, key, set_by: child },
            ]
        );
    }

    #[test]
    fn a_duplicated_name_is_ambiguous() {
        let (dir, _) = fixture();
        let manifest = dir.path().join("Acme.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        value["filament_list"].as_array_mut().unwrap().push(entry("Acme PLA", "filament/pla2.json"));
        fs::write(&manifest, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        write(dir.path(), "Acme/filament/pla2.json", preset("filament", "Acme PLA", "Acme PLA base", true, json!({})));
        let repo = Repository::load(dir.path()).unwrap();
        assert!(repo.references(id(&repo, PresetKind::Filament, "Acme PLA")).ambiguous);
    }
}
