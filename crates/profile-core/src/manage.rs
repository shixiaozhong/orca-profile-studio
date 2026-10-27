//! Previewed, validated changes to preset identity and vendor manifests.

mod models;
#[cfg(test)]
mod tests;
mod transaction;
pub use models::{ModelDraft, ModelPlan};

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::{Document, PresetId, PresetKind, PresetMeta, RawValue, Repository, Severity, VendorId};

#[derive(Debug, thiserror::Error)]
pub enum ManageError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Document(#[from] crate::DocumentError),
}

/// A new preset, optionally based on an existing preset's file.
#[derive(Clone)]
pub struct PresetDraft {
    source: Option<PresetId>,
    vendor: VendorId,
    kind: PresetKind,
    name: String,
    file_name: String,
    parent: Option<PresetId>,
    inherit_source: bool,
    keep_identifiers: bool,
    values: BTreeMap<String, RawValue>,
}

impl PresetDraft {
    pub fn new(vendor: VendorId, kind: PresetKind, name: impl Into<String>, file_name: impl Into<String>) -> Self {
        Self {
            source: None,
            vendor,
            kind,
            name: name.into(),
            file_name: file_name.into(),
            parent: None,
            inherit_source: false,
            keep_identifiers: false,
            values: BTreeMap::new(),
        }
    }

    pub fn copy(repo: &Repository, source: PresetId, name: impl Into<String>, file_name: impl Into<String>) -> Self {
        let preset = repo.preset(source);
        Self { source: Some(source), ..Self::new(preset.vendor, preset.kind, name, file_name) }
    }

    pub fn with_parent(mut self, parent: PresetId) -> Self {
        self.parent = Some(parent);
        self
    }
    pub fn inherit_source(mut self, inherit: bool) -> Self {
        self.inherit_source = inherit;
        self
    }
    pub fn keep_identifiers(mut self, keep: bool) -> Self {
        self.keep_identifiers = keep;
        self
    }
    pub fn with_value(mut self, key: impl Into<String>, value: RawValue) -> Self {
        self.values.insert(key.into(), value);
        self
    }
}

#[derive(Clone)]
pub struct NameRule {
    from: String,
    to: String,
    arrays: Vec<String>,
}

impl NameRule {
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self { from: from.into(), to: to.into(), arrays: Vec::new() }
    }
    pub fn rewrite_array(mut self, key: impl Into<String>) -> Self {
        self.arrays.push(key.into());
        self
    }
    pub fn replace(&self, name: &str) -> Result<String, ManageError> {
        if self.from.is_empty() || !name.contains(&self.from) {
            return Err(ManageError::Invalid(format!("Name replacement does not match {name}")));
        }
        Ok(name.replace(&self.from, &self.to))
    }
}

#[derive(Debug)]
pub struct FileChange {
    path: PathBuf,
    before: Option<String>,
    after: Option<String>,
}

impl FileChange {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn before(&self) -> Option<&str> {
        self.before.as_deref()
    }
    pub fn after(&self) -> Option<&str> {
        self.after.as_deref()
    }
}

#[derive(Debug)]
pub struct PlanNotice {
    message: String,
    blocking: bool,
}

impl PlanNotice {
    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn is_blocking(&self) -> bool {
        self.blocking
    }
}

/// Owns file snapshots and a proposed change set. Planning never writes to the source directory.
pub struct ManagementPlan {
    root: PathBuf,
    snapshot: BTreeMap<PathBuf, String>,
    changes: BTreeMap<PathBuf, FileChange>,
    notices: Vec<PlanNotice>,
    /// Old path → new path of renamed files, so their existing issues are not reported as new.
    moved: BTreeMap<PathBuf, PathBuf>,
}

impl ManagementPlan {
    fn new(repo: &Repository) -> Result<Self, ManageError> {
        let root = repo.root().canonicalize()?;
        let snapshot = transaction::snapshot(&root)?;
        // Refuse stale source data: every file the load read must still have the same bytes.
        for (path, loaded) in repo.sources() {
            let current = snapshot.get(path).map(|text| crate::json::fingerprint(text.as_bytes()));
            if current != loaded {
                return Err(invalid(format!("{} changed on disk; Reload before managing presets", path.display())));
            }
        }
        Ok(Self { root, snapshot, changes: BTreeMap::new(), notices: Vec::new(), moved: BTreeMap::new() })
    }

    pub fn changes(&self) -> impl Iterator<Item = &FileChange> {
        self.changes.values()
    }
    pub fn notices(&self) -> &[PlanNotice] {
        &self.notices
    }
    pub fn is_ready(&self) -> bool {
        !self.changes.is_empty() && !self.notices.iter().any(PlanNotice::is_blocking)
    }

    fn notice(&mut self, blocking: bool, message: impl Into<String>) {
        self.notices.push(PlanNotice { message: message.into(), blocking });
    }

    fn document(&self, path: &Path) -> Result<Document, ManageError> {
        let text = self
            .changes
            .get(path)
            .map(|change| change.after.as_deref())
            .unwrap_or_else(|| self.snapshot.get(path).map(String::as_str))
            .ok_or_else(|| invalid(format!("Cannot read {}", path.display())))?;
        Ok(Document::parse(text)?)
    }

    fn put(&mut self, path: PathBuf, after: Option<String>) -> Result<(), ManageError> {
        transaction::check_path(&self.root, &path)?;
        let before = self.snapshot.get(&path).cloned();
        if before == after {
            self.changes.remove(&path);
        } else {
            self.changes.insert(path.clone(), FileChange { path, before, after });
        }
        Ok(())
    }

    fn manifest(
        &mut self,
        repo: &Repository,
        vendor: VendorId,
        kind: PresetKind,
    ) -> Result<(PathBuf, Document), ManageError> {
        let info = repo.vendor(vendor);
        let mut doc = self.document(&info.index_path)?;
        let key = kind.list_key();
        if !doc.contains(key) {
            doc.set(key, &RawValue::List(Vec::new()));
        }
        // An explicit list must include all auto-discovered library entries, in their load order.
        if info.name == crate::FILAMENT_LIBRARY && kind == PresetKind::Filament && array(&doc, key)?.is_empty() {
            for preset in repo.presets().iter().filter(|preset| preset.vendor == vendor && preset.kind == kind) {
                let sub = preset.path.strip_prefix(&info.name).map_err(|_| invalid("Invalid vendor path"))?;
                let ix = array(&doc, key)?.len();
                doc.array_insert(key, ix, &json!({"name":preset.name(),"sub_path":sub.to_string_lossy()}))?;
            }
            self.notice(false, "The filament library manifest is made explicit to preserve parent-before-child order.");
        }
        Ok((info.index_path.clone(), doc))
    }

    pub fn create(repo: &Repository, draft: PresetDraft) -> Result<Self, ManageError> {
        Self::copy(repo, vec![draft], None)
    }

    pub fn copy(repo: &Repository, mut drafts: Vec<PresetDraft>, rule: Option<NameRule>) -> Result<Self, ManageError> {
        let mut plan = Self::new(repo)?;
        if drafts.is_empty() {
            return Err(invalid("Select at least one preset"));
        }
        if drafts.iter().any(|draft| draft.kind != drafts[0].kind) {
            return Err(invalid("A copy group must contain presets of one kind"));
        }
        if rule.as_ref().is_some_and(|rule| rule.from.is_empty()) {
            return Err(invalid("Enter a non-empty name replacement"));
        }
        drafts.sort_by_key(|draft| draft.source.map(PresetId::index).unwrap_or(usize::MAX));
        let names: BTreeMap<_, _> =
            drafts.iter().filter_map(|draft| draft.source.map(|id| (id, draft.name.clone()))).collect();
        if names.len() != drafts.iter().filter(|draft| draft.source.is_some()).count() {
            return Err(invalid("Select each source preset only once per copy operation"));
        }
        let mut created = BTreeSet::new();
        for draft in &drafts {
            let result = plan.add_draft(repo, draft, &names, &mut created, rule.as_ref());
            if let Err(err) = result {
                plan.notice(true, format!("{}: {err}", draft.name));
            }
        }
        plan.validate(repo)?;
        Ok(plan)
    }

    fn add_draft(
        &mut self,
        repo: &Repository,
        draft: &PresetDraft,
        names: &BTreeMap<PresetId, String>,
        created: &mut BTreeSet<(PresetKind, String)>,
        rule: Option<&NameRule>,
    ) -> Result<(), ManageError> {
        check_name(&draft.name)?;
        let file = check_filename(&draft.file_name)?;
        if !repo.find(draft.kind, &draft.name).is_empty() || !created.insert((draft.kind, draft.name.clone())) {
            return Err(invalid("The new preset name already exists"));
        }
        let source = draft.source.map(|id| repo.preset(id));
        let path = source
            .map_or_else(
                || PathBuf::from(&repo.vendor(draft.vendor).name).join(draft.kind.as_str()),
                |p| p.path.parent().unwrap().to_owned(),
            )
            .join(file);
        let folded = path.to_string_lossy().to_lowercase();
        if self
            .snapshot
            .keys()
            .chain(self.changes.keys())
            .any(|existing| existing.to_string_lossy().to_lowercase() == folded)
            || self.root.join(&path).exists()
        {
            return Err(invalid(format!("{} already exists", path.display())));
        }
        let mut doc = match source {
            Some(source) => self.document(&source.path)?,
            None => Document::parse("{\n}\n")?,
        };
        set_meta(&mut doc, "name", Some(&draft.name));
        set_meta(&mut doc, "type", Some(draft.kind.as_str()));
        set_meta(&mut doc, "from", Some("system"));
        if source.is_none() {
            set_meta(&mut doc, "instantiation", Some("true"));
        }
        let parent =
            draft.parent.or_else(|| source.and_then(|p| if draft.inherit_source { Some(p.id) } else { p.parent }));
        if let Some(parent) = parent {
            let p = repo.preset(parent);
            if p.kind != draft.kind
                || (p.vendor != draft.vendor
                    && !(p.kind == PresetKind::Filament && repo.vendor(p.vendor).name == crate::FILAMENT_LIBRARY))
            {
                return Err(invalid(
                    "The parent must belong to the same kind and vendor, or the shared filament library",
                ));
            }
            let name = if draft.inherit_source {
                p.name()
            } else {
                names.get(&parent).map(String::as_str).unwrap_or(p.name())
            };
            set_meta(&mut doc, "inherits", Some(name));
        } else {
            set_meta(&mut doc, "inherits", None);
        }
        // Old aliases and cloud identity are not silently assigned to a new preset.
        set_meta(&mut doc, "renamed_from", None);
        for key in ["setting_id", "filament_id"] {
            if let Some(value) = source.and_then(|p| {
                if key == "setting_id" { p.meta.setting_id.as_ref() } else { p.meta.filament_id.as_ref() }
            }) {
                self.notice(
                    false,
                    format!(
                        "{}: {} identifier {value} is {}",
                        draft.name,
                        key,
                        if draft.keep_identifiers {
                            "retained; review before writing"
                        } else {
                            "removed from the copy"
                        }
                    ),
                );
            }
            if !draft.keep_identifiers {
                set_meta(&mut doc, key, None);
            }
        }
        if let Some(rule) = rule {
            for key in &rule.arrays {
                if let Some(Value::Array(items)) = doc.value(key) {
                    for (ix, item) in items.iter().enumerate() {
                        if let Some(name) = item.as_str().filter(|name| name.contains(&rule.from)) {
                            doc.array_set(key, ix, &Value::String(name.replace(&rule.from, &rule.to)))?;
                        }
                    }
                }
            }
        }
        for (key, value) in &draft.values {
            if ["name", "type", "from", "inherits"].contains(&key.to_ascii_lowercase().as_str()) {
                return Err(invalid(format!("Use the dedicated control for {key}")));
            }
            if PresetMeta::KEYS.contains(&key.to_ascii_lowercase().as_str()) {
                let RawValue::Str(value) = value else {
                    return Err(invalid(format!("Metadata {key} must be a string")));
                };
                set_meta(&mut doc, key, Some(value));
                if ["setting_id", "filament_id"].contains(&key.as_str()) {
                    self.notice(
                        false,
                        format!("{}: {key} is explicitly set to {value}; review this identifier", draft.name),
                    );
                }
            } else {
                doc.set(key, value);
            }
        }
        let (manifest_path, mut manifest) = self.manifest(repo, draft.vendor, draft.kind)?;
        let key = draft.kind.list_key();
        let ix = array(&manifest, key)?.len();
        let sub = path.strip_prefix(&repo.vendor(draft.vendor).name).map_err(|_| invalid("Invalid vendor path"))?;
        manifest.array_insert(key, ix, &json!({"name":draft.name,"sub_path":sub.to_string_lossy()}))?;
        self.put(path, Some(doc.into_text()))?;
        self.put(manifest_path, Some(manifest.into_text()))?;
        Ok(())
    }

    pub fn rename(
        repo: &Repository,
        id: PresetId,
        name: &str,
        file_name: &str,
        remember: bool,
    ) -> Result<Self, ManageError> {
        let mut plan = Self::new(repo)?;
        let preset = repo.preset(id);
        check_name(name)?;
        if repo.find(preset.kind, preset.name()).len() > 1 {
            return Err(invalid("The source name occurs in more than one vendor; its global references are ambiguous"));
        }
        let file = check_filename(file_name)?;
        if repo.find(preset.kind, name).iter().any(|other| *other != id) {
            return Err(invalid("The new name already exists"));
        }
        let path = preset.path.parent().unwrap().join(file);
        // A case-only rename names the source file itself on case-insensitive file systems.
        let folded = path.to_string_lossy().to_lowercase();
        if folded != preset.path.to_string_lossy().to_lowercase()
            && (plan.snapshot.keys().any(|existing| existing.to_string_lossy().to_lowercase() == folded)
                || plan.root.join(&path).exists())
        {
            return Err(invalid("The destination file already exists"));
        }
        let mut doc = plan.document(&preset.path)?;
        set_meta(&mut doc, "name", Some(name));
        for key in reference_keys(preset.kind) {
            rewrite_reference(&mut doc, key, preset.name(), Some(name))?;
        }
        if remember && name != preset.name() {
            // OrcaSlicer uses a semicolon-separated, C-escaped string list.
            let escaped = serde_json::to_string(preset.name()).expect("string serializes");
            let previous = preset
                .meta
                .renamed_from
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|s| format!("{s};{escaped}"))
                .unwrap_or(escaped);
            set_meta(&mut doc, "renamed_from", Some(&previous));
        }
        let (manifest_path, mut manifest) = plan.manifest(repo, preset.vendor, preset.kind)?;
        let key = preset.kind.list_key();
        let ix = manifest_index(repo, preset, &manifest)?;
        manifest.array_set_member(key, ix, "name", &RawValue::Str(name.into()))?;
        let sub = path.strip_prefix(&repo.vendor(preset.vendor).name).map_err(|_| invalid("Invalid vendor path"))?;
        manifest.array_set_member(key, ix, "sub_path", &RawValue::Str(sub.to_string_lossy().into()))?;
        plan.put(preset.path.clone(), None)?;
        if path != preset.path {
            plan.moved.insert(preset.path.clone(), path.clone());
        }
        plan.put(path, Some(doc.into_text()))?;
        plan.put(manifest_path, Some(manifest.into_text()))?;
        for other in repo.presets().iter().filter(|p| p.id != id) {
            let mut doc = plan.document(&other.path)?;
            let mut changed = false;
            if other.id != id && other.parent == Some(id) {
                set_meta(&mut doc, "inherits", Some(name));
                changed = true;
            }
            for key in reference_keys(preset.kind) {
                changed |= rewrite_reference(&mut doc, key, preset.name(), Some(name))? > 0;
            }
            if changed {
                plan.notice(false, format!("Update references in {}", other.path.display()));
                plan.put(other.path.clone(), Some(doc.into_text()))?;
            }
        }
        if preset.kind == PresetKind::Filament {
            for model in repo.models() {
                let mut doc = plan.document(&model.path)?;
                if rewrite_reference(&mut doc, "default_materials", preset.name(), Some(name))? > 0 {
                    plan.notice(false, format!("Update default_materials in {}", model.path.display()));
                    plan.put(model.path.clone(), Some(doc.into_text()))?;
                }
            }
        }
        plan.validate(repo)?;
        Ok(plan)
    }

    pub fn delete(repo: &Repository, ids: &[PresetId]) -> Result<Self, ManageError> {
        if ids.is_empty() {
            return Err(invalid("Select at least one preset"));
        }
        let mut plan = Self::new(repo)?;
        let ids: BTreeSet<_> = ids.iter().copied().collect();
        for other in repo.presets().iter().filter(|p| !ids.contains(&p.id)) {
            if other.parent.is_some_and(|parent| ids.contains(&parent)) {
                plan.notice(
                    true,
                    format!(
                        "{} inherits a selected preset; include it in the deletion or change its parent first",
                        other.path.display()
                    ),
                );
            }
            let mut doc = plan.document(&other.path)?;
            let mut changed = false;
            for id in &ids {
                let target = repo.preset(*id);
                if repo.find(target.kind, target.name()).iter().any(|other| !ids.contains(other)) {
                    continue;
                }
                for key in reference_keys(target.kind) {
                    if rewrite_reference(&mut doc, key, target.name(), None)? > 0 {
                        changed = true;
                        plan.notice(
                            false,
                            format!("Remove {} reference from {}: {key}", target.name(), other.path.display()),
                        );
                        // An empty list means "no restriction": the preset would match everything.
                        if ["compatible_printers", "compatible_prints"].contains(key)
                            && doc.value(key) == Some(json!([]))
                        {
                            plan.notice(
                                true,
                                format!(
                                    "{}: removing {} empties {key}, which would make it compatible with every \
                                     {}; delete that preset too or change its {key} first",
                                    other.path.display(),
                                    target.name(),
                                    if *key == "compatible_printers" { "printer" } else { "process" }
                                ),
                            );
                        }
                    }
                }
            }
            if changed {
                plan.put(other.path.clone(), Some(doc.into_text()))?;
            }
        }
        for model in repo.models() {
            let mut doc = plan.document(&model.path)?;
            let mut changed = false;
            for id in &ids {
                let target = repo.preset(*id);
                if target.kind == PresetKind::Filament
                    && !repo.find(target.kind, target.name()).iter().any(|other| !ids.contains(other))
                {
                    changed |= rewrite_reference(&mut doc, "default_materials", target.name(), None)? > 0;
                }
            }
            if changed {
                plan.notice(false, format!("Remove deleted material references from {}", model.path.display()));
                plan.put(model.path.clone(), Some(doc.into_text()))?;
            }
        }
        for id in ids {
            let preset = repo.preset(id);
            let (path, mut manifest) = plan.manifest(repo, preset.vendor, preset.kind)?;
            let ix = manifest_index(repo, preset, &manifest)?;
            manifest.array_remove(preset.kind.list_key(), ix)?;
            plan.put(path, Some(manifest.into_text()))?;
            plan.put(preset.path.clone(), None)?;
        }
        plan.validate(repo)?;
        Ok(plan)
    }

    fn validate(&mut self, repo: &Repository) -> Result<(), ManageError> {
        if self.notices.iter().any(PlanNotice::is_blocking) {
            return Ok(());
        }
        // The loaded repository is the baseline (`new` checked it matches the snapshot), so only the
        // candidate is loaded. Unchanged files are linked instead of copied where the file system allows.
        let shadow = tempfile::tempdir()?;
        for (path, text) in self.snapshot.iter().filter(|(path, _)| !self.changes.contains_key(*path)) {
            let full = shadow.path().join(path);
            std::fs::create_dir_all(full.parent().unwrap())?;
            if std::fs::hard_link(self.root.join(path), &full).is_err() {
                std::fs::write(full, text)?;
            }
        }
        for change in self.changes.values() {
            if let Some(text) = &change.after {
                let path = shadow.path().join(&change.path);
                std::fs::create_dir_all(path.parent().unwrap())?;
                std::fs::write(path, text)?;
            }
        }
        let load = |path: &Path| match repo.schema() {
            Some(schema) => Repository::load_with_schema(path, Arc::new(schema.clone())),
            None => Repository::load(path),
        };
        let key = |path: &Path, issue: &crate::Issue| format!("{}:{:?}", path.display(), issue.kind);
        let old: BTreeSet<_> =
            repo.issues().iter().map(|issue| key(self.moved.get(&issue.path).unwrap_or(&issue.path), issue)).collect();
        let candidate = load(shadow.path()).map_err(|e| invalid(e.to_string()))?;
        for issue in candidate.issues() {
            if issue.severity() >= Severity::Warning && !old.contains(&key(&issue.path, issue)) {
                self.notice(
                    issue.severity() >= Severity::Error,
                    format!("{}: {} · {}", issue.path.display(), issue.severity(), issue.kind),
                );
            }
        }
        Ok(())
    }

    /// Rechecks all previewed input files and writes with rollback on an I/O failure.
    pub fn apply(&self) -> Result<(), ManageError> {
        self.apply_with_failure(None)
    }
    fn apply_with_failure(&self, fail_after: Option<usize>) -> Result<(), ManageError> {
        if !self.is_ready() {
            return Err(invalid("Resolve the blocking preview issues before writing"));
        }
        transaction::apply(self, fail_after)
    }
}

fn invalid(message: impl Into<String>) -> ManageError {
    ManageError::Invalid(message.into())
}

fn check_name(name: &str) -> Result<(), ManageError> {
    if name.trim().is_empty() || name.chars().any(char::is_control) {
        return Err(invalid("Enter a non-empty preset name without control characters"));
    }
    Ok(())
}

fn check_filename(name: &str) -> Result<&str, ManageError> {
    if name.is_empty()
        || Path::new(name).extension().is_none()
        || name.trim() != name
        || name.chars().any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
        || !name.ends_with(".json")
        || Path::new(name).components().count() != 1
        || !matches!(Path::new(name).components().next(), Some(Component::Normal(_)))
    {
        return Err(invalid("Enter one valid .json file name without path separators"));
    }
    Ok(name)
}

pub fn preset_file_name(name: &str) -> String {
    let stem: String =
        name.chars().map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c }).collect();
    format!("{}.json", stem.trim())
}

fn set_meta(doc: &mut Document, key: &str, value: Option<&str>) {
    let keys: Vec<_> = doc.keys().filter(|k| k.eq_ignore_ascii_case(key)).map(str::to_owned).collect();
    if keys.is_empty() {
        if let Some(value) = value {
            doc.set(key, &RawValue::Str(value.into()));
        }
    } else {
        for key in keys {
            match value {
                Some(value) => doc.set(&key, &RawValue::Str(value.into())),
                None => doc.remove(&key),
            }
        }
    }
}

fn array(doc: &Document, key: &str) -> Result<Vec<Value>, ManageError> {
    doc.value(key).and_then(|v| v.as_array().cloned()).ok_or_else(|| invalid(format!("{key} is not an array")))
}

fn manifest_index(repo: &Repository, preset: &crate::Preset, doc: &Document) -> Result<usize, ManageError> {
    let sub = preset.path.strip_prefix(&repo.vendor(preset.vendor).name).map_err(|_| invalid("Invalid vendor path"))?;
    let matches: Vec<_> = array(doc, preset.kind.list_key())?
        .into_iter()
        .enumerate()
        .filter(|(_, item)| item.get("sub_path").and_then(Value::as_str) == sub.to_str())
        .map(|(ix, _)| ix)
        .collect();
    if matches.len() != 1 {
        return Err(invalid(format!("{} is not uniquely listed in its manifest; Reload first", preset.path.display())));
    }
    Ok(matches[0])
}

fn reference_keys(kind: PresetKind) -> &'static [&'static str] {
    match kind {
        PresetKind::Machine => &["compatible_printers"],
        PresetKind::Process => &["compatible_prints", "default_print_profile"],
        PresetKind::Filament => &["default_filament_profile"],
    }
}

fn rewrite_reference(doc: &mut Document, key: &str, old: &str, new: Option<&str>) -> Result<usize, ManageError> {
    let mut count = 0;
    match doc.value(key) {
        Some(Value::Array(items)) => {
            for (ix, item) in items.iter().enumerate().rev() {
                if item.as_str() == Some(old) {
                    match new {
                        Some(name) => doc.array_set(key, ix, &Value::String(name.into()))?,
                        None => doc.array_remove(key, ix)?,
                    }
                    count += 1;
                }
            }
        }
        Some(Value::String(value)) if key == "default_materials" => {
            let parts: Vec<_> = value
                .split(';')
                .filter_map(|part| {
                    if part == old {
                        count += 1;
                        new.map(str::to_owned)
                    } else {
                        Some(part.to_owned())
                    }
                })
                .collect();
            if count > 0 {
                doc.set(key, &RawValue::Str(parts.join(";")));
            }
        }
        Some(Value::String(value)) if value == old => {
            match new {
                Some(name) => doc.set(key, &RawValue::Str(name.into())),
                None => doc.remove(key),
            }
            count = 1;
        }
        _ => {}
    }
    Ok(count)
}
