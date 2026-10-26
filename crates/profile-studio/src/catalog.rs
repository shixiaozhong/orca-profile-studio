//! Display-oriented indexes over a loaded [`Repository`]. Built on a background thread.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use profile_core::{Issue, PresetKind, Repository, SearchIndex, Severity};

/// Printers grouped under the machine model their `printer_model` names.
pub struct ModelEntry {
    /// Index into [`Repository::models`]; `None` collects printers whose model is unknown.
    pub model: Option<usize>,
    pub label: String,
    pub printers: Vec<usize>,
}

/// One vendor's presets, sorted for display. All numbers are indexes into [`Repository::presets`].
#[derive(Default)]
pub struct VendorContents {
    pub models: Vec<ModelEntry>,
    pub filaments: Vec<usize>,
    pub processes: Vec<usize>,
    pub templates: Vec<usize>,
    pub issues: BTreeMap<Severity, usize>,
}

impl VendorContents {
    pub fn printer_count(&self) -> usize {
        self.models.iter().map(|entry| entry.printers.len()).sum()
    }

    pub fn model_count(&self) -> usize {
        self.models.iter().filter(|entry| entry.model.is_some()).count()
    }

    pub fn other_printers(&self) -> Option<&ModelEntry> {
        self.models.iter().find(|entry| entry.model.is_none())
    }
}

pub struct Catalog {
    pub repo: Repository,
    pub load_time: Duration,
    /// Indexed like [`Repository::vendors`].
    pub vendors: Vec<VendorContents>,
    /// Indexed like [`Repository::models`].
    pub model_printer_counts: Vec<usize>,
    /// Issue indexes per preset, including file-level issues such as a BOM.
    pub preset_issues: Vec<Vec<usize>>,
    pub preset_severity: Vec<Option<Severity>>,
    /// Global search over the loaded data (not over unsaved edits).
    pub search: SearchIndex,
    translations: Arc<profile_core::Translations>,
    pub translation_warning: Option<String>,
    /// Indexes are local to one load; file paths keep identity across reloads.
    preset_by_path: HashMap<PathBuf, usize>,
}

impl Catalog {
    pub fn new(repo: Repository, load_time: Duration) -> Self {
        let vendors = group_presets(&repo);
        let mut model_printer_counts = vec![0; repo.models().len()];
        for entry in vendors.iter().flat_map(|contents| &contents.models) {
            if let Some(model) = entry.model {
                model_printer_counts[model] = entry.printers.len();
            }
        }
        let (vendors, preset_issues) = attach_issues(&repo, vendors);
        let preset_severity = preset_issues
            .iter()
            .map(|issues| issues.iter().map(|&index| repo.issues()[index].severity()).max())
            .collect();
        let (translations, translation_warning) = match crate::i18n::load_translations(
            repo.root(),
            std::env::var_os("ORCA_LOCALIZATION_DIR").map(PathBuf::from),
        ) {
            Ok(translations) => (translations, None),
            Err(message) => (Arc::default(), Some(message)),
        };
        let mut search = SearchIndex::build(&repo);
        if let Some(schema) = repo.schema() {
            search.add_option_labels(schema.options.iter().filter_map(|(key, option)| {
                translations.get(option.display_label()).map(|label| (key.clone(), label.to_owned()))
            }));
        }
        let mut preset_by_path = HashMap::new();
        for preset in repo.presets() {
            // A file listed twice resolves to its first preset, like a linear search would.
            preset_by_path.entry(preset.path.clone()).or_insert(preset.id.index());
        }
        Catalog {
            repo,
            load_time,
            vendors,
            model_printer_counts,
            preset_issues,
            preset_severity,
            search,
            translations,
            translation_warning,
            preset_by_path,
        }
    }

    pub fn search_labels(&self, option: Option<&profile_core::OptionDef>) -> String {
        option.map_or(String::new(), |option| {
            [option.display_label(), &option.category]
                .into_iter()
                .map(|text| format!("{text} {}", self.translations.get(text).unwrap_or("")))
                .collect::<Vec<_>>()
                .join(" ")
        })
    }

    pub fn translate(&self, text: &str) -> String {
        if crate::i18n::is_chinese() { self.translations.get(text).unwrap_or(text).to_owned() } else { text.to_owned() }
    }

    pub fn preset_at(&self, path: &Path) -> Option<usize> {
        self.preset_by_path.get(path).copied()
    }

    /// The preset of `old` (another load) with the same file in this catalog.
    pub fn remap_preset(&self, old: &Catalog, preset: usize) -> Option<usize> {
        self.preset_at(&old.repo.presets()[preset].path)
    }

    /// The vendor of `old` (another load) with the same index file in this catalog.
    pub fn remap_vendor(&self, old: &Catalog, vendor: usize) -> Option<usize> {
        let path = &old.repo.vendors()[vendor].index_path;
        self.repo.vendors().iter().position(|vendor| &vendor.index_path == path)
    }

    /// The machine model of `old` (another load) with the same file in this catalog.
    pub fn remap_model(&self, old: &Catalog, model: usize) -> Option<usize> {
        let path = &old.repo.models()[model].path;
        self.repo.models().iter().position(|model| &model.path == path)
    }

    pub fn issues_of(&self, preset: usize) -> impl Iterator<Item = &Issue> {
        self.preset_issues[preset].iter().map(|&index| &self.repo.issues()[index])
    }

    pub fn issue_totals(&self) -> BTreeMap<Severity, usize> {
        let mut totals = BTreeMap::new();
        for issue in self.repo.issues() {
            *totals.entry(issue.severity()).or_default() += 1;
        }
        totals
    }
}

fn group_presets(repo: &Repository) -> Vec<VendorContents> {
    let mut vendors: Vec<VendorContents> = repo.vendors().iter().map(|_| VendorContents::default()).collect();
    let mut model_slots: HashMap<(usize, &str), usize> = HashMap::new();
    for (index, model) in repo.models().iter().enumerate() {
        let contents = &mut vendors[model.vendor.index()];
        model_slots.insert((model.vendor.index(), model.id.as_str()), contents.models.len());
        contents.models.push(ModelEntry { model: Some(index), label: model.id.clone(), printers: Vec::new() });
    }

    for preset in repo.presets() {
        let vendor = preset.vendor.index();
        let index = preset.id.index();
        let contents = &mut vendors[vendor];
        if !repo.is_selectable(preset.id) {
            contents.templates.push(index);
            continue;
        }
        match preset.kind {
            PresetKind::Filament => contents.filaments.push(index),
            PresetKind::Process => contents.processes.push(index),
            PresetKind::Machine => {
                let known =
                    repo.lookup_scalar(preset.id, "printer_model").and_then(|model| model_slots.get(&(vendor, model)));
                let slot = match known {
                    Some(&slot) => slot,
                    None => match contents.models.iter().position(|entry| entry.model.is_none()) {
                        Some(slot) => slot,
                        None => {
                            let label = t!("tree.other_printers");
                            contents.models.push(ModelEntry { model: None, label, printers: Vec::new() });
                            contents.models.len() - 1
                        }
                    },
                };
                contents.models[slot].printers.push(index);
            }
        }
    }

    let name_key = |&index: &usize| repo.presets()[index].name().to_lowercase();
    for contents in &mut vendors {
        for entry in &mut contents.models {
            entry.printers.sort_by_cached_key(name_key);
        }
        contents.models.sort_by_cached_key(|entry| (entry.model.is_none(), entry.label.to_lowercase()));
        contents.filaments.sort_by_cached_key(name_key);
        contents.processes.sort_by_cached_key(name_key);
        contents.templates.sort_by_cached_key(|index| (repo.presets()[*index].kind, name_key(index)));
    }
    vendors
}

fn attach_issues(repo: &Repository, mut vendors: Vec<VendorContents>) -> (Vec<VendorContents>, Vec<Vec<usize>>) {
    let by_path: HashMap<&Path, usize> =
        repo.presets().iter().map(|preset| (preset.path.as_path(), preset.id.index())).collect();
    let mut preset_issues = vec![Vec::new(); repo.presets().len()];
    for (index, issue) in repo.issues().iter().enumerate() {
        *vendors[issue.vendor.index()].issues.entry(issue.severity()).or_default() += 1;
        // Some file-level issues are recorded before the preset gets an id.
        let preset = issue.preset.map(|id| id.index()).or_else(|| by_path.get(issue.path.as_path()).copied());
        if let Some(preset) = preset {
            preset_issues[preset].push(index);
        }
    }
    (vendors, preset_issues)
}
