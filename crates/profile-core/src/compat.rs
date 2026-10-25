//! Which printers (and processes) a filament or process preset is compatible with.
//!
//! Mirrors `is_compatible_with_printer` / `is_compatible_with_print` in OrcaSlicer's
//! `src/libslic3r/Preset.cpp` for system presets, including the OrcaFilamentLibrary exclusion
//! (`PresetCollection::update_library_profile_excluded_from`). See spec section 6.

use std::collections::{HashMap, HashSet};

use crate::condition::{Condition, ConditionError, Value, Variable};
use crate::model::{PresetId, PresetKind, RawValue};
use crate::repository::{FILAMENT_LIBRARY, Repository};
use crate::schema::OptionType;

/// Why a preset is or is not compatible with a printer or process.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// A library filament whose alias a vendor filament for this printer also has.
    Excluded,
    /// The condition decided. OrcaSlicer treats an error as compatible.
    Condition(Result<bool, ConditionError>),
    /// Named in `compatible_printers` / `compatible_prints`.
    Listed,
    /// Not named in a non-empty list.
    NotListed,
    /// No list and no condition: compatible with everything.
    Unrestricted,
}

impl Verdict {
    pub fn is_compatible(&self) -> bool {
        match self {
            Verdict::Excluded | Verdict::NotListed | Verdict::Condition(Ok(false)) => false,
            Verdict::Listed | Verdict::Unrestricted | Verdict::Condition(Ok(true) | Err(_)) => true,
        }
    }
}

/// The keys deciding compatibility with a kind of preset.
fn keys(target: PresetKind) -> (&'static str, &'static str) {
    match target {
        PresetKind::Process => ("compatible_prints", "compatible_prints_condition"),
        _ => ("compatible_printers", "compatible_printers_condition"),
    }
}

/// Precomputed compatibility data of a loaded repository.
#[derive(Debug, Default)]
pub struct CompatibilityIndex {
    /// Library filament → names of printers it is excluded from.
    excluded: HashMap<PresetId, HashSet<String>>,
    /// Parsed conditions, by preset and target kind, for presets whose list is empty.
    conditions: HashMap<(PresetId, PresetKind), Result<Condition, ConditionError>>,
}

impl CompatibilityIndex {
    pub(crate) fn build(repo: &Repository) -> Self {
        let selectable = || repo.presets().iter().filter(|preset| repo.is_selectable(preset.id));
        let list = |id, key| match repo.lookup(id, key).map(|resolved| resolved.value) {
            Some(RawValue::List(items)) => items.iter().filter(|item| !item.is_empty()).cloned().collect(),
            Some(RawValue::Str(item)) if !item.is_empty() => vec![item.clone()],
            _ => Vec::new(),
        };

        // Library filaments with an empty compatible_printers, by alias (the last one wins, like
        // the C++ map).
        let mut by_alias: HashMap<&str, PresetId> = HashMap::new();
        for preset in selectable().filter(|preset| preset.kind == PresetKind::Filament) {
            if repo.vendor(preset.vendor).name == FILAMENT_LIBRARY && list(preset.id, "compatible_printers").is_empty()
            {
                by_alias.insert(repo.alias(preset.id), preset.id);
            }
        }
        let mut excluded: HashMap<PresetId, HashSet<String>> = HashMap::new();
        for preset in selectable().filter(|preset| preset.kind == PresetKind::Filament) {
            if repo.vendor(preset.vendor).name == FILAMENT_LIBRARY {
                continue;
            }
            if let Some(&library) = by_alias.get(repo.alias(preset.id)) {
                excluded.entry(library).or_default().extend(list(preset.id, "compatible_printers"));
            }
        }

        let mut conditions = HashMap::new();
        for preset in selectable().filter(|preset| preset.kind != PresetKind::Machine) {
            for target in [PresetKind::Machine, PresetKind::Process] {
                if target == PresetKind::Process && preset.kind != PresetKind::Filament {
                    continue;
                }
                let (list_key, condition_key) = keys(target);
                let condition = repo.lookup_scalar(preset.id, condition_key).unwrap_or_default();
                if list(preset.id, list_key).is_empty() && !condition.trim().is_empty() {
                    conditions.insert((preset.id, target), Condition::parse(condition));
                }
            }
        }
        CompatibilityIndex { excluded, conditions }
    }

    /// Whether `preset` (a filament or process) can be used with `target` (a printer, or for a
    /// filament also a process).
    pub fn verdict(&self, repo: &Repository, preset: PresetId, target: PresetId) -> Verdict {
        let target_kind = repo.preset(target).kind;
        if target_kind == PresetKind::Machine
            && self.excluded.get(&preset).is_some_and(|printers| printers.contains(repo.preset(target).name()))
        {
            return Verdict::Excluded;
        }
        if let Some(condition) = self.conditions.get(&(preset, target_kind)) {
            let result = condition
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|condition| condition.evaluate(&|name| variable(repo, target, name)));
            return Verdict::Condition(result);
        }
        let (list_key, _) = keys(target_kind);
        match repo.lookup(preset, list_key).map(|resolved| resolved.value) {
            Some(RawValue::List(items)) if items.iter().any(|item| !item.is_empty()) => {
                if items.iter().any(|item| item == repo.preset(target).name()) {
                    Verdict::Listed
                } else {
                    Verdict::NotListed
                }
            }
            _ => Verdict::Unrestricted,
        }
    }

    /// The selectable printers `preset` is compatible with, with the reason for each.
    pub fn compatible_printers(&self, repo: &Repository, preset: PresetId) -> Vec<(PresetId, Verdict)> {
        repo.presets()
            .iter()
            .filter(|printer| printer.kind == PresetKind::Machine && repo.is_selectable(printer.id))
            .map(|printer| (printer.id, self.verdict(repo, preset, printer.id)))
            .filter(|(_, verdict)| verdict.is_compatible())
            .collect()
    }

    /// The selectable presets of `kind` compatible with `printer`.
    pub fn compatible_with_printer(&self, repo: &Repository, printer: PresetId, kind: PresetKind) -> Vec<PresetId> {
        repo.presets()
            .iter()
            .filter(|preset| preset.kind == kind && repo.is_selectable(preset.id))
            .filter(|preset| self.verdict(repo, preset.id, printer).is_compatible())
            .map(|preset| preset.id)
            .collect()
    }

    /// The parsed condition deciding compatibility with `target` kind presets, if any.
    pub fn condition(&self, preset: PresetId, target: PresetKind) -> Option<&Result<Condition, ConditionError>> {
        self.conditions.get(&(preset, target))
    }

    pub(crate) fn conditions(
        &self,
    ) -> impl Iterator<Item = (PresetId, PresetKind, &Result<Condition, ConditionError>)> {
        self.conditions.iter().map(|(&(preset, target), condition)| (preset, target, condition))
    }
}

/// A variable of the target preset's configuration, typed by the schema, plus the
/// `printer_preset` and `num_extruders` OrcaSlicer adds for printers.
fn variable(repo: &Repository, target: PresetId, name: &str) -> Option<Variable> {
    let preset = repo.preset(target);
    if preset.kind == PresetKind::Machine {
        match name {
            "printer_preset" => return Some(Variable::Scalar(Value::Str(preset.name().to_owned()))),
            "num_extruders" => {
                let count = match repo.lookup(target, "nozzle_diameter")?.value {
                    RawValue::List(items) => items.len(),
                    RawValue::Str(text) => text.split(',').count(),
                };
                return Some(Variable::Scalar(Value::Int(count as i64)));
            }
            _ => {}
        }
    }
    variable_from(repo, name, repo.lookup(target, name)?.value)
}

/// Types a configuration value by the schema.
fn variable_from(repo: &Repository, name: &str, raw: &RawValue) -> Option<Variable> {
    let type_ = repo.schema().and_then(|schema| schema.option(name)).map_or(OptionType::String, |option| option.type_);
    let element = type_.element();
    Some(if type_.is_vector() {
        let items: Vec<String> = match raw {
            RawValue::List(items) => items.clone(),
            RawValue::Str(text) if type_ == OptionType::Strings => vec![text.clone()],
            RawValue::Str(text) => text.split(',').map(|item| item.trim().to_owned()).collect(),
        };
        Variable::Vector(items.iter().map(|item| typed(element, item)).collect())
    } else {
        let text = match raw {
            RawValue::Str(text) => text.as_str(),
            RawValue::List(items) => items.first().map_or("", String::as_str),
        };
        Variable::Scalar(typed(type_, text))
    })
}

fn typed(type_: OptionType, text: &str) -> Value {
    let number = || text.trim().trim_end_matches('%').trim().parse::<f64>().ok();
    match type_ {
        OptionType::Bool => match text {
            "1" => Value::Bool(true),
            "0" => Value::Bool(false),
            _ => Value::Str(text.to_owned()),
        },
        OptionType::Int => text.trim().parse().map(Value::Int).unwrap_or_else(|_| Value::Str(text.to_owned())),
        OptionType::Float | OptionType::Percent | OptionType::FloatOrPercent => {
            number().map(Value::Double).unwrap_or_else(|| Value::Str(text.to_owned()))
        }
        _ => Value::Str(text.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::Verdict;
    use crate::model::PresetKind;
    use crate::repository::Repository;
    use crate::schema::Schema;

    /// A library PLA, a vendor PLA with the same alias for printer "A 0.4", and processes
    /// selecting printers by list or by condition.
    fn fixture() -> (tempfile::TempDir, Repository) {
        let dir = tempfile::tempdir().unwrap();
        let write = |path: &str, text: &str| {
            let path = dir.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write(
            "OrcaFilamentLibrary.json",
            r#"{ "name": "OrcaFilamentLibrary", "version": "1.0", "filament_list": [] }"#,
        );
        write(
            "OrcaFilamentLibrary/filament/Generic PLA.json",
            r#"{ "type": "filament", "name": "Generic PLA @System", "instantiation": "true", "filament_id": "GFL" }"#,
        );
        write(
            "Acme.json",
            r#"{ "name": "Acme", "version": "1.0",
                "machine_model_list": [{ "name": "A", "sub_path": "machine/A.json" }],
                "process_list": [
                    { "name": "by list", "sub_path": "process/list.json" },
                    { "name": "by condition", "sub_path": "process/condition.json" },
                    { "name": "broken", "sub_path": "process/broken.json" } ],
                "filament_list": [{ "name": "Generic PLA @A", "sub_path": "filament/pla.json" }],
                "machine_list": [
                    { "name": "A 0.4 nozzle", "sub_path": "machine/a04.json" },
                    { "name": "A 0.6 nozzle", "sub_path": "machine/a06.json" } ] }"#,
        );
        write("Acme/machine/A.json", r#"{ "type": "machine_model", "name": "A", "nozzle_diameter": "0.4;0.6" }"#);
        for (file, name, nozzle) in [("a04", "A 0.4 nozzle", "0.4"), ("a06", "A 0.6 nozzle", "0.6")] {
            write(
                &format!("Acme/machine/{file}.json"),
                &format!(
                    r#"{{ "type": "machine", "name": "{name}", "instantiation": "true", "printer_model": "A",
                        "printer_variant": "{nozzle}", "nozzle_diameter": ["{nozzle}"],
                        "printer_notes": "Acme\nMODEL_A\n" }}"#
                ),
            );
        }
        write(
            "Acme/filament/pla.json",
            r#"{ "type": "filament", "name": "Generic PLA @A", "inherits": "Generic PLA @System",
                "instantiation": "true", "compatible_printers": ["A 0.4 nozzle"] }"#,
        );
        write(
            "Acme/process/list.json",
            r#"{ "type": "process", "name": "by list", "instantiation": "true", "compatible_printers": ["A 0.6 nozzle"] }"#,
        );
        write(
            "Acme/process/condition.json",
            r#"{ "type": "process", "name": "by condition", "instantiation": "true", "compatible_printers": [],
                "compatible_printers_condition": "printer_notes=~/.*MODEL_A.*/ and nozzle_diameter[0]==0.4" }"#,
        );
        write(
            "Acme/process/broken.json",
            r#"{ "type": "process", "name": "broken", "instantiation": "true",
                "compatible_printers_condition": "nozzle_diameter == 0.4" }"#,
        );
        let repo = Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap();
        (dir, repo)
    }

    #[test]
    fn decides_by_exclusion_list_and_condition() {
        let (_dir, repo) = fixture();
        let index = repo.compatibility();
        let one = |kind, name| repo.find(kind, name)[0];
        let (a04, a06) = (one(PresetKind::Machine, "A 0.4 nozzle"), one(PresetKind::Machine, "A 0.6 nozzle"));
        let names = |preset| {
            index
                .compatible_printers(&repo, preset)
                .into_iter()
                .map(|(id, _)| repo.preset(id).name())
                .collect::<Vec<_>>()
        };

        let library = one(PresetKind::Filament, "Generic PLA @System");
        assert_eq!(index.verdict(&repo, library, a04), Verdict::Excluded, "the vendor's PLA covers A 0.4");
        assert_eq!(names(library), ["A 0.6 nozzle"]);
        assert_eq!(names(one(PresetKind::Filament, "Generic PLA @A")), ["A 0.4 nozzle"]);
        assert_eq!(index.verdict(&repo, one(PresetKind::Process, "by list"), a04), Verdict::NotListed);

        let by_condition = one(PresetKind::Process, "by condition");
        assert_eq!(index.verdict(&repo, by_condition, a04), Verdict::Condition(Ok(true)));
        assert_eq!(index.verdict(&repo, by_condition, a06), Verdict::Condition(Ok(false)));
        assert_eq!(
            index
                .compatible_with_printer(&repo, a06, PresetKind::Process)
                .into_iter()
                .map(|id| repo.preset(id).name())
                .collect::<Vec<_>>(),
            ["by list", "broken"]
        );
    }

    #[test]
    fn a_failing_condition_is_compatible_and_reported() {
        let (_dir, repo) = fixture();
        let broken = repo.find(PresetKind::Process, "broken")[0];
        let verdict = repo.compatibility().verdict(&repo, broken, repo.find(PresetKind::Machine, "A 0.4 nozzle")[0]);
        assert!(matches!(verdict, Verdict::Condition(Err(_))) && verdict.is_compatible());
        let codes: Vec<_> = repo
            .issues()
            .iter()
            .filter(|issue| issue.preset == Some(broken) && issue.kind.code().starts_with("condition"))
            .map(|issue| issue.kind.code())
            .collect();
        assert_eq!(codes, ["condition-failed"]);
    }
}
