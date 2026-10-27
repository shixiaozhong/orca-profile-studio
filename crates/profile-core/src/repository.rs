//! Loads a `resources/profiles` tree the way OrcaSlicer's `PresetBundle` does.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::check::check_params;
use crate::compat::{CompatibilityIndex, Verdict};
use crate::issue::{Issue, IssueKind};
use crate::json::{fingerprint, read_json};
use crate::model::{MachineModel, Preset, PresetId, PresetKind, PresetMeta, RawValue, Vendor, VendorId, derived_alias};
use crate::schema::Schema;

/// The shared filament library. It loads first and is the only cross-vendor inheritance source.
pub const FILAMENT_LIBRARY: &str = "OrcaFilamentLibrary";
/// Presets of this vendor are loaded even when `instantiation` is "false".
const TEMPLATE_VENDOR: &str = "Template";
/// Top-level JSON files that are not vendor indexes.
const NON_VENDOR_FILES: &[&str] = &["blacklist"];

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("cannot list profiles directory {}: {source}", .path.display())]
    ReadDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Where an effective value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// A preset file in the inheritance chain.
    Preset(PresetId),
    /// The schema's default preset of the kind, which the root of every chain starts from.
    Default,
}

/// A config value together with the layer of the inheritance chain that set it.
#[derive(Debug, Clone, Copy)]
pub struct ResolvedValue<'a> {
    pub value: &'a RawValue,
    pub origin: Origin,
}

/// Every layer of an inheritance chain that sets one key, nearest first.
///
/// The first layer holds the effective value; the others are the values it overrides.
#[derive(Debug, Clone)]
pub struct KeyTrace<'a> {
    pub key: &'a str,
    pub layers: Vec<ResolvedValue<'a>>,
}

impl<'a> KeyTrace<'a> {
    pub fn effective(&self) -> ResolvedValue<'a> {
        self.layers[0]
    }
}

#[derive(Debug, Default)]
pub struct Repository {
    root: PathBuf,
    schema: Option<Arc<Schema>>,
    compatibility: CompatibilityIndex,
    vendors: Vec<Vendor>,
    models: Vec<MachineModel>,
    presets: Vec<Preset>,
    issues: Vec<Issue>,
    by_name: BTreeMap<(PresetKind, String), Vec<PresetId>>,
    /// Every path named by a vendor index, whether or not it could be loaded.
    listed: HashSet<PathBuf>,
    /// Content fingerprint of every JSON file the load looked at, relative to `root`; `None` when
    /// the file did not exist or could not be read.
    sources: HashMap<PathBuf, Option<u64>>,
}

struct ManifestItem {
    list_name: String,
    sub_path: String,
}

#[derive(Default)]
struct Manifest {
    machine_models: Vec<ManifestItem>,
    presets: [Vec<ManifestItem>; 3],
}

struct ParsedPreset {
    path: PathBuf,
    list_name: String,
    meta: PresetMeta,
    params: IndexMap<String, RawValue>,
    had_bom: bool,
}

impl Repository {
    /// Loads without a schema: values are not checked and keys no file sets stay unresolved.
    pub fn load(root: impl AsRef<Path>) -> Result<Self, LoadError> {
        Self::load_impl(root.as_ref(), None)
    }

    /// Loads, checks every value against `schema` and resolves unset keys to its defaults.
    pub fn load_with_schema(root: impl AsRef<Path>, schema: Arc<Schema>) -> Result<Self, LoadError> {
        Self::load_impl(root.as_ref(), Some(schema))
    }

    fn load_impl(root: &Path, schema: Option<Arc<Schema>>) -> Result<Self, LoadError> {
        let root = root.to_path_buf();
        let vendor_names = vendor_names(&root)?;
        let mut repo = Repository { root, schema, ..Default::default() };
        let mut library = HashMap::new();
        for name in &vendor_names {
            repo.load_vendor(name, &mut library);
        }
        repo.check_cross_vendor_duplicates();
        repo.check_compatible_printers();
        repo.find_orphans();
        repo.compatibility = CompatibilityIndex::build(&repo);
        repo.check_conditions();
        Ok(repo)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Files this load looked at, relative to [`Self::root`], with a content fingerprint each
    /// (`None`: missing or unreadable).
    pub(crate) fn sources(&self) -> impl Iterator<Item = (&Path, Option<u64>)> {
        self.sources.iter().map(|(path, &fingerprint)| (path.as_path(), fingerprint))
    }

    pub fn schema(&self) -> Option<&Schema> {
        self.schema.as_deref()
    }

    /// Which printers and processes each filament and process preset is compatible with.
    pub fn compatibility(&self) -> &CompatibilityIndex {
        &self.compatibility
    }

    pub fn vendors(&self) -> &[Vendor] {
        &self.vendors
    }

    pub fn vendor(&self, id: VendorId) -> &Vendor {
        &self.vendors[id.index()]
    }

    pub fn models(&self) -> &[MachineModel] {
        &self.models
    }

    pub fn presets(&self) -> &[Preset] {
        &self.presets
    }

    pub fn preset(&self, id: PresetId) -> &Preset {
        &self.presets[id.index()]
    }

    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }

    /// All presets of `kind` with this exact name; more than one means a duplicate.
    pub fn find(&self, kind: PresetKind, name: &str) -> &[PresetId] {
        self.by_name.get(&(kind, name.to_owned())).map_or(&[], Vec::as_slice)
    }

    /// Whether OrcaSlicer turns this file into a preset users can pick.
    pub fn is_selectable(&self, id: PresetId) -> bool {
        let preset = self.preset(id);
        !preset.is_template() || self.vendor(preset.vendor).name == TEMPLATE_VENDOR
    }

    /// The preset followed by its ancestors, nearest first.
    pub fn chain(&self, id: PresetId) -> Vec<PresetId> {
        let mut chain = vec![id];
        let mut current = id;
        // Parents always load before their children, so the walk cannot cycle; the bound is a safety net.
        while let Some(parent) = self.preset(current).parent
            && chain.len() <= self.presets.len()
        {
            chain.push(parent);
            current = parent;
        }
        chain
    }

    /// Number of ancestors, or `None` when some `inherits` in the chain could not be resolved.
    pub fn depth(&self, id: PresetId) -> Option<usize> {
        let chain = self.chain(id);
        let broken = chain.iter().any(|&link| self.preset(link).inherits_unresolved);
        (!broken).then(|| chain.len() - 1)
    }

    /// The effective value of `key`: a child replaces the whole value of its parent, and the
    /// schema's default applies when no file in the chain sets the key.
    pub fn lookup(&self, id: PresetId, key: &str) -> Option<ResolvedValue<'_>> {
        let own = self.chain(id).into_iter().find_map(|link| {
            let value = self.preset(link).params.get(key)?;
            Some(ResolvedValue { value, origin: Origin::Preset(link) })
        });
        own.or_else(|| {
            let value = self.defaults(self.preset(id).kind)?.get(key)?;
            Some(ResolvedValue { value, origin: Origin::Default })
        })
    }

    pub fn lookup_scalar(&self, id: PresetId, key: &str) -> Option<&str> {
        self.lookup(id, key)?.value.as_scalar()
    }

    /// All effective values: the schema defaults first (when a schema is loaded), then the keys
    /// in the order they first appear from the root of the chain down.
    ///
    /// Without a schema, keys no file in the chain sets are missing.
    pub fn resolve(&self, id: PresetId) -> IndexMap<&str, ResolvedValue<'_>> {
        let mut values = IndexMap::new();
        for (key, value) in self.defaults(self.preset(id).kind).into_iter().flatten() {
            values.insert(key.as_str(), ResolvedValue { value, origin: Origin::Default });
        }
        for link in self.chain(id).into_iter().rev() {
            for (key, value) in &self.preset(link).params {
                values.insert(key.as_str(), ResolvedValue { value, origin: Origin::Preset(link) });
            }
        }
        values
    }

    /// Every key with all the layers that set it, in the order of [`Repository::resolve`].
    pub fn trace(&self, id: PresetId) -> Vec<KeyTrace<'_>> {
        self.trace_with(id, &self.preset(id).params)
    }

    /// Like [`Repository::trace`], with `own` in place of the preset's own values, e.g. the
    /// values of an [`EditSession`](crate::EditSession).
    pub fn trace_with<'a>(&'a self, id: PresetId, own: &'a IndexMap<String, RawValue>) -> Vec<KeyTrace<'a>> {
        fn add<'a>(traces: &mut IndexMap<&'a str, KeyTrace<'a>>, key: &'a str, layer: ResolvedValue<'a>) {
            let trace = traces.entry(key).or_insert_with(|| KeyTrace { key, layers: Vec::new() });
            // Layers are added from the root down; keep the nearest first.
            trace.layers.insert(0, layer);
        }
        let mut traces = IndexMap::new();
        for (key, value) in self.defaults(self.preset(id).kind).into_iter().flatten() {
            add(&mut traces, key, ResolvedValue { value, origin: Origin::Default });
        }
        for link in self.chain(id).into_iter().rev() {
            let params = if link == id { own } else { &self.preset(link).params };
            for (key, value) in params {
                add(&mut traces, key, ResolvedValue { value, origin: Origin::Preset(link) });
            }
        }
        traces.into_values().collect()
    }

    /// The schema checks of `params` as the own values of preset `id`; empty without a schema.
    pub fn check_values(&self, id: PresetId, params: &IndexMap<String, RawValue>) -> Vec<IssueKind> {
        let Some(schema) = &self.schema else { return Vec::new() };
        let preset = self.preset(id);
        let instantiated = !preset.is_template() || self.vendor(preset.vendor).name == TEMPLATE_VENDOR;
        let ancestors = preset.parent.map(|parent| self.chain(parent)).unwrap_or_default();
        let inherited = |key: &str| ancestors.iter().any(|&link| self.preset(link).params.contains_key(key));
        check_params(schema, preset.kind, params, instantiated, inherited)
    }

    fn defaults(&self, kind: PresetKind) -> Option<&IndexMap<String, RawValue>> {
        Some(&self.schema()?.default_preset(kind).values)
    }

    /// The display alias: an explicit `alias` option, otherwise the name before `@`.
    pub fn alias(&self, id: PresetId) -> &str {
        if let Some(alias) = self.lookup_scalar(id, "alias").filter(|alias| !alias.is_empty()) {
            return alias;
        }
        let name = self.preset(id).name();
        match derived_alias(name) {
            "" => name,
            alias => alias,
        }
    }

    /// Own `filament_id`, else own `setting_id`, else the parent's resolved id.
    pub fn filament_id(&self, id: PresetId) -> Option<&str> {
        let preset = self.preset(id);
        let own = [&preset.meta.filament_id, &preset.meta.setting_id]
            .into_iter()
            .find_map(|value| value.as_deref().filter(|value| !value.is_empty()));
        own.or_else(|| self.filament_id(preset.parent?))
    }

    fn load_vendor(&mut self, name: &str, library: &mut HashMap<String, PresetId>) {
        let vendor = VendorId(self.vendors.len() as u32);
        let index_path = PathBuf::from(format!("{name}.json"));
        self.vendors.push(Vendor {
            id: vendor,
            name: name.to_owned(),
            index_path: index_path.clone(),
            ..Default::default()
        });
        let Some((index, _)) = self.read_object(vendor, &index_path) else {
            return;
        };

        let mut manifest = Manifest::default();
        for (key, value) in &index {
            let text = value.as_str().map(str::to_owned);
            let info = &mut self.vendors[vendor.index()];
            match key.to_ascii_lowercase().as_str() {
                "name" => info.display_name = text,
                "version" => info.version = text,
                "description" => info.description = text,
                "url" => info.url = text,
                "force_update" => info.force_update = text,
                "machine_model_list" => {
                    manifest.machine_models = self.parse_manifest_list(vendor, &index_path, key, value);
                }
                list_key => {
                    if let Some(kind) = PresetKind::from_list_key(list_key) {
                        manifest.presets[kind.index()] = self.parse_manifest_list(vendor, &index_path, key, value);
                    }
                }
            }
        }
        let version = self.vendors[vendor.index()].version.clone();
        if !version.as_deref().is_some_and(is_valid_version) {
            self.issue(vendor, &index_path, None, IssueKind::InvalidVendorVersion(version));
        }

        let vendor_dir = PathBuf::from(name);
        let filaments = &mut manifest.presets[PresetKind::Filament.index()];
        if name == FILAMENT_LIBRARY && filaments.is_empty() {
            *filaments = discover_library_filaments(&self.root, &vendor_dir);
        }

        self.load_machine_models(vendor, &vendor_dir, &manifest.machine_models);
        for kind in PresetKind::ALL {
            let local = self.load_section(vendor, &vendor_dir, kind, &manifest.presets[kind.index()], library);
            if name == FILAMENT_LIBRARY && kind == PresetKind::Filament {
                *library = local;
            }
        }
    }

    fn parse_manifest_list(
        &mut self,
        vendor: VendorId,
        index_path: &Path,
        key: &str,
        value: &Value,
    ) -> Vec<ManifestItem> {
        let Some(entries) = value.as_array() else {
            let reason = format!("`{key}` is not an array");
            self.issue(vendor, index_path, None, IssueKind::InvalidManifestEntry(reason));
            return Vec::new();
        };
        let mut items = Vec::with_capacity(entries.len());
        for (position, entry) in entries.iter().enumerate() {
            let field = |wanted: &str| {
                let (_, value) = entry.as_object()?.iter().find(|(key, _)| key.eq_ignore_ascii_case(wanted))?;
                value.as_str().filter(|text| !text.is_empty()).map(str::to_owned)
            };
            match (field("name"), field("sub_path")) {
                (Some(list_name), Some(sub_path)) => items.push(ManifestItem { list_name, sub_path }),
                _ => {
                    let reason = format!("`{key}[{position}]` needs non-empty string `name` and `sub_path`");
                    self.issue(vendor, index_path, None, IssueKind::InvalidManifestEntry(reason));
                }
            }
        }
        items
    }

    fn load_machine_models(&mut self, vendor: VendorId, vendor_dir: &Path, items: &[ManifestItem]) {
        for item in items {
            let path = vendor_dir.join(&item.sub_path);
            self.listed.insert(path.clone());
            let Some((object, _)) = self.read_object(vendor, &path) else {
                continue;
            };
            let mut model = MachineModel { vendor, id: item.list_name.clone(), path, ..Default::default() };
            for (key, value) in object {
                let text = value.as_str().map(str::to_owned);
                match key.to_ascii_lowercase().as_str() {
                    "name" => model.name = text,
                    "model_id" => model.model_id = text,
                    "nozzle_diameter" => model.variants = text.as_deref().map(split_list).unwrap_or_default(),
                    "machine_tech" => model.technology = text,
                    "family" => model.family = text,
                    "bed_model" => model.bed_model = text,
                    "bed_texture" => model.bed_texture = text,
                    "hotend_model" => model.hotend_model = text,
                    "default_materials" => {
                        let mut materials = text.as_deref().map(split_list).unwrap_or_default();
                        materials.sort();
                        materials.dedup();
                        model.default_materials = materials;
                        model.default_materials_text = text;
                    }
                    _ => {
                        model.extra.insert(key, value);
                    }
                }
            }
            if model.variants.is_empty() {
                let path = model.path.clone();
                self.issue(vendor, &path, None, IssueKind::ModelWithoutVariants);
            }
            self.models.push(model);
        }
    }

    /// Loads one section of a vendor in list order and returns its name table, which is what
    /// later entries of the same section can inherit from.
    fn load_section(
        &mut self,
        vendor: VendorId,
        vendor_dir: &Path,
        kind: PresetKind,
        items: &[ManifestItem],
        library: &HashMap<String, PresetId>,
    ) -> HashMap<String, PresetId> {
        // Read everything first so a parent listed after its child can be told apart from a missing one.
        let parsed: Vec<ParsedPreset> = items
            .iter()
            .filter_map(|item| self.read_preset(vendor, vendor_dir.join(&item.sub_path), &item.list_name))
            .collect();
        let listed_names: HashSet<String> = parsed.iter().map(|preset| preset.meta.name.clone()).collect();
        let is_library = self.vendor(vendor).name == FILAMENT_LIBRARY;
        let is_template_vendor = self.vendor(vendor).name == TEMPLATE_VENDOR;

        let mut local: HashMap<String, PresetId> = HashMap::new();
        let mut instantiated: HashSet<String> = HashSet::new();
        for parsed in parsed {
            let id = PresetId(self.presets.len() as u32);
            let path = parsed.path.clone();
            let name = parsed.meta.name.clone();
            let mut issues = Vec::new();

            if name != parsed.list_name {
                issues.push(IssueKind::ListNameMismatch { list_name: parsed.list_name.clone() });
            }
            if let Some(declared) = &parsed.meta.type_
                && !declared.eq_ignore_ascii_case(kind.as_str())
            {
                issues.push(IssueKind::TypeMismatch { declared: declared.clone() });
            }
            if parsed.meta.from.as_deref() != Some("system") {
                issues.push(IssueKind::UnexpectedFrom(parsed.meta.from.clone()));
            }
            let instantiation = parsed.meta.instantiation.as_deref();
            if !matches!(instantiation, Some("true" | "false")) {
                issues.push(IssueKind::InvalidInstantiation(parsed.meta.instantiation.clone()));
            }

            let mut parent = None;
            let mut inherits_unresolved = false;
            let mut inherits_from_library = false;
            if let Some(parent_name) = parsed.meta.inherits() {
                if let Some(&own) = local.get(parent_name) {
                    parent = Some(own);
                } else if let Some(&shared) = library.get(parent_name) {
                    parent = Some(shared);
                    inherits_from_library = !is_library;
                } else {
                    inherits_unresolved = true;
                    let parent = parent_name.to_owned();
                    issues.push(if listed_names.contains(&parent) {
                        IssueKind::ParentListedAfterChild { parent }
                    } else {
                        IssueKind::ParentNotFound { parent }
                    });
                }
            }

            let is_template = instantiation == Some("false") && !is_template_vendor;
            self.presets.push(Preset {
                id,
                vendor,
                kind,
                list_name: parsed.list_name,
                path: parsed.path,
                meta: parsed.meta,
                params: parsed.params,
                had_bom: parsed.had_bom,
                parent,
                inherits_unresolved,
                inherits_from_library,
            });
            self.by_name.entry((kind, name.clone())).or_default().push(id);
            // OrcaSlicer's table keeps the first preset of a name.
            local.entry(name.clone()).or_insert(id);

            if !is_template {
                if !instantiated.insert(name) {
                    issues.push(IssueKind::DuplicateInVendor);
                }
                match kind {
                    PresetKind::Machine => issues.extend(self.check_printer_model(id)),
                    PresetKind::Filament if !is_template_vendor && self.filament_id(id).is_none() => {
                        issues.push(IssueKind::FilamentIdMissing);
                    }
                    _ => {}
                }
            }
            issues.extend(self.check_values(id, &self.preset(id).params));
            for kind in issues {
                self.issue(vendor, &path, Some(id), kind);
            }
        }
        local
    }

    fn read_preset(&mut self, vendor: VendorId, path: PathBuf, list_name: &str) -> Option<ParsedPreset> {
        self.listed.insert(path.clone());
        let (object, had_bom) = self.read_object(vendor, &path)?;
        let mut meta = PresetMeta::default();
        let mut params = IndexMap::with_capacity(object.len());
        for (key, value) in object {
            let lower = key.to_ascii_lowercase();
            if PresetMeta::KEYS.contains(&lower.as_str()) {
                match value {
                    Value::String(text) => meta.set(&lower, text),
                    _ => self.issue(vendor, &path, None, IssueKind::InvalidMetaType { key }),
                }
            } else {
                match to_raw_value(value) {
                    Some(value) => {
                        params.insert(key, value);
                    }
                    None => self.issue(vendor, &path, None, IssueKind::InvalidValueType { key }),
                }
            }
        }
        if meta.name.is_empty() {
            self.issue(vendor, &path, None, IssueKind::MissingName);
            return None;
        }
        Some(ParsedPreset { path, list_name: list_name.to_owned(), meta, params, had_bom })
    }

    fn read_object(&mut self, vendor: VendorId, path: &Path) -> Option<(Map<String, Value>, bool)> {
        let full_path = self.root.join(path);
        if !full_path.is_file() {
            self.sources.insert(path.to_owned(), None);
            self.issue(vendor, path, None, IssueKind::ListedFileMissing);
            return None;
        }
        let file = match read_json(&full_path) {
            Ok(file) => {
                self.sources.insert(path.to_owned(), Some(file.fingerprint));
                file
            }
            Err(err) => {
                self.sources.insert(path.to_owned(), fs::read(&full_path).ok().map(|bytes| fingerprint(&bytes)));
                self.issue(vendor, path, None, IssueKind::FileUnreadable(err.to_string()));
                return None;
            }
        };
        if file.had_bom {
            self.issue(vendor, path, None, IssueKind::Utf8Bom);
        }
        match file.value {
            Value::Object(object) => Some((object, file.had_bom)),
            _ => {
                let reason = "top-level JSON value is not an object".to_owned();
                self.issue(vendor, path, None, IssueKind::FileUnreadable(reason));
                None
            }
        }
    }

    fn check_printer_model(&self, id: PresetId) -> Option<IssueKind> {
        let vendor = self.preset(id).vendor;
        let non_empty = |key| self.lookup_scalar(id, key).filter(|text| !text.is_empty());
        let (Some(model), Some(variant)) = (non_empty("printer_model"), non_empty("printer_variant")) else {
            return Some(IssueKind::PrinterModelMissing);
        };
        match self.models.iter().find(|candidate| candidate.vendor == vendor && candidate.id == model) {
            None => Some(IssueKind::PrinterModelUnknown { model: model.to_owned() }),
            Some(found) if !found.variants.iter().any(|known| known == variant) => {
                Some(IssueKind::PrinterVariantUnknown { model: model.to_owned(), variant: variant.to_owned() })
            }
            Some(_) => None,
        }
    }

    fn check_cross_vendor_duplicates(&mut self) {
        let mut found = Vec::new();
        for ids in self.by_name.values() {
            let mut selectable = ids.iter().copied().filter(|&id| self.is_selectable(id));
            let Some(first) = selectable.next() else { continue };
            let first_vendor = self.preset(first).vendor;
            // Duplicates within one vendor were reported while loading it.
            found.extend(selectable.filter(|&id| self.preset(id).vendor != first_vendor).map(|id| (id, first_vendor)));
        }
        for (id, first_vendor) in found {
            let first_vendor = self.vendor(first_vendor).name.clone();
            self.preset_issue(id, IssueKind::DuplicateAcrossVendors { first_vendor });
        }
    }

    fn check_compatible_printers(&mut self) {
        let printers: HashSet<&str> = self
            .presets
            .iter()
            .filter(|preset| preset.kind == PresetKind::Machine && self.is_selectable(preset.id))
            .map(Preset::name)
            .collect();
        let mut dangling = Vec::new();
        for preset in &self.presets {
            if !self.is_selectable(preset.id) {
                continue;
            }
            let Some(ResolvedValue { value: RawValue::List(names), .. }) =
                self.lookup(preset.id, "compatible_printers")
            else {
                continue;
            };
            for printer in names {
                if !printer.is_empty() && !printers.contains(printer.as_str()) {
                    dangling.push((preset.id, printer.clone()));
                }
            }
        }
        for (id, printer) in dangling {
            self.preset_issue(id, IssueKind::DanglingCompatiblePrinter { printer });
        }
    }

    /// Reports conditions that do not parse, or that fail for some of the presets they are
    /// evaluated against. OrcaSlicer treats both as "compatible with everything".
    fn check_conditions(&mut self) {
        let mut found = Vec::new();
        for (preset, target, condition) in self.compatibility.conditions() {
            let key = if target == PresetKind::Process {
                "compatible_prints_condition"
            } else {
                "compatible_printers_condition"
            };
            if let Err(err) = condition {
                found.push((preset, IssueKind::ConditionInvalid { key: key.to_owned(), message: err.to_string() }));
                continue;
            }
            let targets = self.presets.iter().filter(|other| other.kind == target && self.is_selectable(other.id));
            let mut failures = 0;
            let mut first = None;
            for other in targets {
                if let Verdict::Condition(Err(err)) = self.compatibility.verdict(self, preset, other.id) {
                    failures += 1;
                    first.get_or_insert_with(|| format!("{}: {err}", other.name()));
                }
            }
            if let Some(message) = first {
                found.push((preset, IssueKind::ConditionFailed { key: key.to_owned(), message, failures }));
            }
        }
        for (preset, kind) in found {
            self.preset_issue(preset, kind);
        }
    }

    fn find_orphans(&mut self) {
        let mut orphans = Vec::new();
        for vendor in &self.vendors {
            let mut files = Vec::new();
            collect_json_files(&self.root.join(&vendor.name), true, &mut files);
            files.sort();
            for file in files {
                if let Ok(relative) = file.strip_prefix(&self.root)
                    && !self.listed.contains(relative)
                {
                    orphans.push((vendor.id, relative.to_path_buf()));
                }
            }
        }
        for (vendor, path) in orphans {
            self.issue(vendor, &path, None, IssueKind::OrphanFile);
        }
    }

    fn preset_issue(&mut self, id: PresetId, kind: IssueKind) {
        let preset = self.preset(id);
        let (vendor, path) = (preset.vendor, preset.path.clone());
        self.issue(vendor, &path, Some(id), kind);
    }

    fn issue(&mut self, vendor: VendorId, path: &Path, preset: Option<PresetId>, kind: IssueKind) {
        self.issues.push(Issue { kind, vendor, path: path.to_path_buf(), preset });
    }
}

fn vendor_names(root: &Path) -> Result<Vec<String>, LoadError> {
    let entries = fs::read_dir(root).map_err(|source| LoadError::ReadDir { path: root.to_path_buf(), source })?;
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_json(path))
        .filter_map(|path| path.file_stem()?.to_str().map(str::to_owned))
        .filter(|name| !NON_VENDOR_FILES.contains(&name.as_str()))
        .collect();
    // OrcaSlicer uses directory order, which is unspecified; sort for reproducible results.
    names.sort();
    if let Some(position) = names.iter().position(|name| name == FILAMENT_LIBRARY) {
        let library = names.remove(position);
        names.insert(0, library);
    }
    Ok(names)
}

/// OrcaFilamentLibrary ships an empty `filament_list`; OrcaSlicer builds it from the disk:
/// `filament/base/` first, then `filament/` itself, then each other subdirectory by name.
fn discover_library_filaments(root: &Path, vendor_dir: &Path) -> Vec<ManifestItem> {
    let vendor_root = root.join(vendor_dir);
    let filament_dir = vendor_root.join("filament");
    let mut groups = vec![(filament_dir.join("base"), true), (filament_dir.clone(), false)];
    let mut subdirs: Vec<PathBuf> = fs::read_dir(&filament_dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.file_name().is_some_and(|name| name != "base"))
        .collect();
    subdirs.sort();
    groups.extend(subdirs.into_iter().map(|dir| (dir, true)));

    let mut items = Vec::new();
    for (dir, recursive) in groups {
        let mut files = Vec::new();
        collect_json_files(&dir, recursive, &mut files);
        files.sort_by_cached_key(|file| (library_priority(file), generic_path(file)));
        for file in files {
            let Ok(relative) = file.strip_prefix(&vendor_root) else { continue };
            let list_name = file.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
            items.push(ManifestItem { list_name, sub_path: generic_path(relative) });
        }
    }
    items
}

fn library_priority(file: &Path) -> u8 {
    let stem = file.file_stem().map(|stem| stem.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if stem == "fdm_filament_common" {
        0
    } else if stem.ends_with("@base") {
        1
    } else if stem.ends_with("_common") {
        2
    } else {
        3
    }
}

fn collect_json_files(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
        if path.is_dir() {
            if recursive {
                collect_json_files(&path, true, out);
            }
        } else if is_json(&path) {
            out.push(path);
        }
    }
}

fn is_json(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
}

fn generic_path(path: &Path) -> String {
    path.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

fn is_valid_version(version: &str) -> bool {
    !version.is_empty() && version.split('.').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// Splits a `;`-separated list such as `"0.4;0.6"`. OrcaSlicer also accepts C-style quoted
/// items here; profiles do not use them.
fn split_list(text: &str) -> Vec<String> {
    text.split(';')
        .map(|item| item.trim().trim_matches('"'))
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

fn to_raw_value(value: Value) -> Option<RawValue> {
    match value {
        Value::String(text) => Some(RawValue::Str(text)),
        Value::Array(items) => items
            .into_iter()
            .map(|item| match item {
                Value::String(text) => Some(text),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .map(RawValue::List),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Fixture {
        dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            Fixture { dir: tempfile::tempdir().unwrap() }
        }

        fn write(&self, path: &str, value: Value) -> &Self {
            self.write_bytes(path, &serde_json::to_vec_pretty(&value).unwrap())
        }

        fn write_bytes(&self, path: &str, bytes: &[u8]) -> &Self {
            let full = self.dir.path().join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, bytes).unwrap();
            self
        }

        fn load(&self) -> Repository {
            Repository::load(self.dir.path()).unwrap()
        }
    }

    fn entry(name: &str, sub_path: &str) -> Value {
        json!({ "name": name, "sub_path": sub_path })
    }

    fn preset(kind: &str, name: &str, inherits: &str, instantiation: bool, extra: Value) -> Value {
        let mut value = json!({
            "type": kind,
            "name": name,
            "from": "system",
            "inherits": inherits,
            "instantiation": instantiation.to_string(),
        });
        value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        value
    }

    /// A library with one base filament and a vendor with a model, a printer and filaments.
    fn sample() -> Fixture {
        let fixture = Fixture::new();
        fixture
            .write(
                "OrcaFilamentLibrary.json",
                json!({ "name": "OrcaFilamentLibrary", "version": "01.00.00.00", "filament_list": [] }),
            )
            .write(
                "OrcaFilamentLibrary/filament/base/fdm_filament_common.json",
                preset(
                    "filament",
                    "fdm_filament_common",
                    "",
                    false,
                    json!({ "filament_id": "GFL00", "nozzle_temperature": ["200", "200"] }),
                ),
            )
            .write(
                "Acme.json",
                json!({
                    "name": "Acme",
                    "version": "01.00.00.00",
                    "machine_model_list": [entry("Acme One", "machine/Acme One.json")],
                    "process_list": [],
                    "filament_list": [entry("Acme PLA @Acme One", "filament/Acme PLA.json")],
                    "machine_list": [
                        entry("fdm_machine_common", "machine/fdm_machine_common.json"),
                        entry("Acme One 0.4 nozzle", "machine/Acme One 0.4 nozzle.json"),
                    ],
                }),
            )
            .write(
                "Acme/machine/Acme One.json",
                json!({ "type": "machine_model", "name": "Acme One", "nozzle_diameter": "0.4;0.6" }),
            )
            .write(
                "Acme/machine/fdm_machine_common.json",
                preset("machine", "fdm_machine_common", "", false, json!({ "printer_model": "Acme One" })),
            )
            .write(
                "Acme/machine/Acme One 0.4 nozzle.json",
                preset(
                    "machine",
                    "Acme One 0.4 nozzle",
                    "fdm_machine_common",
                    true,
                    json!({ "printer_variant": "0.4" }),
                ),
            )
            .write(
                "Acme/filament/Acme PLA.json",
                preset(
                    "filament",
                    "Acme PLA @Acme One",
                    "fdm_filament_common",
                    true,
                    json!({ "nozzle_temperature": ["210"], "compatible_printers": ["Acme One 0.4 nozzle"] }),
                ),
            );
        fixture
    }

    fn only(repo: &Repository, kind: PresetKind, name: &str) -> PresetId {
        match repo.find(kind, name) {
            [id] => *id,
            other => panic!("expected one {kind} named {name:?}, found {}", other.len()),
        }
    }

    fn codes(repo: &Repository) -> Vec<&'static str> {
        repo.issues().iter().map(|issue| issue.kind.code()).collect()
    }

    #[test]
    fn clean_fixture_has_no_issues() {
        let repo = sample().load();
        assert_eq!(codes(&repo), Vec::<&str>::new());
    }

    #[test]
    fn filament_inherits_from_library_across_vendors() {
        let repo = sample().load();
        let pla = only(&repo, PresetKind::Filament, "Acme PLA @Acme One");
        let base = only(&repo, PresetKind::Filament, "fdm_filament_common");
        assert_eq!(repo.chain(pla), vec![pla, base]);
        assert!(repo.preset(pla).inherits_from_library);
        assert_eq!(repo.filament_id(pla), Some("GFL00"));
        assert_eq!(repo.alias(pla), "Acme PLA");
    }

    #[test]
    fn child_replaces_whole_array() {
        let repo = sample().load();
        let pla = only(&repo, PresetKind::Filament, "Acme PLA @Acme One");
        let resolved = repo.lookup(pla, "nozzle_temperature").unwrap();
        assert_eq!(resolved.value, &RawValue::List(vec!["210".into()]));
        assert_eq!(resolved.origin, Origin::Preset(pla));
    }

    #[test]
    fn trace_lists_overridden_layers_nearest_first() {
        let repo = sample().load();
        let pla = only(&repo, PresetKind::Filament, "Acme PLA @Acme One");
        let base = only(&repo, PresetKind::Filament, "fdm_filament_common");
        let traces = repo.trace(pla);
        let temperature = traces.iter().find(|trace| trace.key == "nozzle_temperature").unwrap();
        let origins: Vec<Origin> = temperature.layers.iter().map(|layer| layer.origin).collect();
        assert_eq!(origins, [Origin::Preset(pla), Origin::Preset(base)]);
        assert_eq!(temperature.effective().value, &RawValue::List(vec!["210".into()]));
        assert_eq!(traces.len(), repo.resolve(pla).len());
    }

    #[test]
    fn printer_model_is_resolved_through_inheritance() {
        let repo = sample().load();
        let printer = only(&repo, PresetKind::Machine, "Acme One 0.4 nozzle");
        assert_eq!(repo.lookup_scalar(printer, "printer_model"), Some("Acme One"));
        assert_eq!(repo.depth(printer), Some(1));
    }

    #[test]
    fn parent_listed_after_child_is_fatal() {
        let fixture = Fixture::new();
        fixture
            .write(
                "Acme.json",
                json!({
                    "name": "Acme",
                    "version": "1.0",
                    "process_list": [entry("child", "process/child.json"), entry("base", "process/base.json")],
                }),
            )
            .write("Acme/process/child.json", preset("process", "child", "base", true, json!({})))
            .write("Acme/process/base.json", preset("process", "base", "", false, json!({})));
        let repo = fixture.load();
        assert_eq!(codes(&repo), vec!["parent-listed-after-child"]);
        assert_eq!(repo.issues()[0].severity(), crate::Severity::Fatal);
        let child = only(&repo, PresetKind::Process, "child");
        assert_eq!(repo.depth(child), None);
    }

    #[test]
    fn bom_is_tolerated_but_reported() {
        let fixture = sample();
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend(
            serde_json::to_vec(&preset(
                "filament",
                "Acme PETG",
                "fdm_filament_common",
                true,
                json!({
                    "compatible_printers": ["Acme One 0.4 nozzle"],
                }),
            ))
            .unwrap(),
        );
        fixture.write_bytes("Acme/filament/Acme PETG.json", &bytes);
        let index = json!({
            "name": "Acme",
            "version": "01.00.00.00",
            "machine_model_list": [entry("Acme One", "machine/Acme One.json")],
            "filament_list": [entry("Acme PLA @Acme One", "filament/Acme PLA.json"), entry("Acme PETG", "filament/Acme PETG.json")],
            "machine_list": [
                entry("fdm_machine_common", "machine/fdm_machine_common.json"),
                entry("Acme One 0.4 nozzle", "machine/Acme One 0.4 nozzle.json"),
            ],
        });
        fixture.write("Acme.json", index);
        let repo = fixture.load();
        assert_eq!(codes(&repo), vec!["utf8-bom"]);
        let petg = only(&repo, PresetKind::Filament, "Acme PETG");
        assert!(repo.preset(petg).had_bom);
    }

    #[test]
    fn reports_unknown_variant_dangling_printer_and_orphan() {
        let fixture = sample();
        fixture
            .write(
                "Acme/machine/Acme One 0.4 nozzle.json",
                preset(
                    "machine",
                    "Acme One 0.4 nozzle",
                    "fdm_machine_common",
                    true,
                    json!({ "printer_variant": "0.8" }),
                ),
            )
            .write(
                "Acme/filament/Acme PLA.json",
                preset(
                    "filament",
                    "Acme PLA @Acme One",
                    "fdm_filament_common",
                    true,
                    json!({ "compatible_printers": ["Acme Two"] }),
                ),
            )
            .write("Acme/process/forgotten.json", preset("process", "forgotten", "", true, json!({})));
        let repo = fixture.load();
        let mut found = codes(&repo);
        found.sort();
        assert_eq!(found, vec!["dangling-compatible-printer", "orphan-file", "printer-variant-unknown"]);
    }
}
