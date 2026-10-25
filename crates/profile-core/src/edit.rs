//! Editing one preset's own values, with undo and redo, and saving them back to its file.

use std::path::PathBuf;
use std::{fs, io};

use indexmap::IndexMap;

use crate::document::{Document, DocumentError};
use crate::json::read_json;
use crate::model::{PresetId, PresetMeta, RawValue};
use crate::repository::Repository;

/// One step of the undo history. `None` means the preset does not set the key, so the value is
/// inherited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub key: String,
    pub before: Option<RawValue>,
    pub after: Option<RawValue>,
}

/// The values a preset file sets, as edited so far.
#[derive(Debug, Clone)]
pub struct EditSession {
    preset: PresetId,
    /// The own values as last loaded or saved.
    saved: IndexMap<String, RawValue>,
    current: IndexMap<String, RawValue>,
    undo: Vec<Change>,
    redo: Vec<Change>,
}

impl EditSession {
    pub fn new(repo: &Repository, preset: PresetId) -> Self {
        let params = repo.preset(preset).params.clone();
        EditSession { preset, saved: params.clone(), current: params, undo: Vec::new(), redo: Vec::new() }
    }

    pub fn preset(&self) -> PresetId {
        self.preset
    }

    /// Moves this session to a freshly loaded repository without losing edits or history.
    /// The caller matches the preset by file path. External changes must be resolved explicitly;
    /// accepting them here would silently turn old values into new edits on the next undo.
    pub fn rebase(&mut self, repo: &Repository, preset: PresetId) -> Result<(), SaveError> {
        let loaded = repo.preset(preset);
        if loaded.params != self.saved {
            return Err(SaveError::ChangedOnDisk(repo.root().join(&loaded.path)));
        }
        self.preset = preset;
        self.saved = loaded.params.clone();
        Ok(())
    }

    /// The own values with the edits applied, in file order; new keys come last.
    pub fn params(&self) -> &IndexMap<String, RawValue> {
        &self.current
    }

    /// Sets a key, or with `None` removes it so the inherited value applies again.
    /// Returns whether anything changed.
    pub fn set(&mut self, key: &str, value: Option<RawValue>) -> bool {
        let before = self.current.get(key).cloned();
        if before == value {
            return false;
        }
        let change = Change { key: key.to_owned(), before, after: value };
        self.apply(&change.key, change.after.clone());
        self.undo.push(change);
        self.redo.clear();
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Reverts the last change and returns its key.
    pub fn undo(&mut self) -> Option<String> {
        let change = self.undo.pop()?;
        self.apply(&change.key, change.before.clone());
        let key = change.key.clone();
        self.redo.push(change);
        Some(key)
    }

    /// Re-applies the last undone change and returns its key.
    pub fn redo(&mut self) -> Option<String> {
        let change = self.redo.pop()?;
        self.apply(&change.key, change.after.clone());
        let key = change.key.clone();
        self.undo.push(change);
        Some(key)
    }

    fn apply(&mut self, key: &str, value: Option<RawValue>) {
        match value {
            Some(value) => {
                self.current.insert(key.to_owned(), value);
            }
            None => {
                self.current.shift_remove(key);
            }
        }
    }

    /// Keys whose value differs from the saved file.
    pub fn is_modified(&self, key: &str) -> bool {
        self.current.get(key) != self.saved.get(key)
    }

    pub fn is_dirty(&self) -> bool {
        // IndexMap equality ignores order, which saving does not change either.
        self.current != self.saved
    }

    /// The difference to the saved file, one entry per changed key: saved keys in file order,
    /// then new keys.
    pub fn pending(&self) -> Vec<Change> {
        let removed_or_changed = self.saved.iter().filter_map(|(key, before)| {
            let after = self.current.get(key);
            (after != Some(before)).then(|| Change {
                key: key.clone(),
                before: Some(before.clone()),
                after: after.cloned(),
            })
        });
        let added = self.current.iter().filter(|(key, _)| !self.saved.contains_key(*key)).map(|(key, after)| Change {
            key: key.clone(),
            before: None,
            after: Some(after.clone()),
        });
        removed_or_changed.chain(added).collect()
    }

    /// Writes the pending changes into the preset's file, leaving everything else in it as it is.
    ///
    /// Refuses when the file's values no longer match the ones last loaded or saved, so changes
    /// made on disk in the meantime are never overwritten. The undo history is kept.
    pub fn save(&mut self, repo: &Repository) -> Result<PathBuf, SaveError> {
        let path = repo.root().join(&repo.preset(self.preset).path);
        let on_disk = read_json(&path).map_err(|err| SaveError::Read(err.to_string()))?;
        let Some(object) = on_disk.value.as_object() else { return Err(SaveError::ChangedOnDisk(path)) };
        // The same split as loading: metadata apart, values that are not strings dropped.
        let on_disk: IndexMap<String, RawValue> = object
            .iter()
            .filter(|(key, _)| !PresetMeta::KEYS.contains(&key.to_ascii_lowercase().as_str()))
            .filter_map(|(key, value)| Some((key.clone(), serde_json::from_value(value.clone()).ok()?)))
            .collect();
        if on_disk != self.saved {
            return Err(SaveError::ChangedOnDisk(path));
        }
        let mut document = Document::parse(fs::read_to_string(&path)?)?;
        for change in self.pending() {
            match &change.after {
                Some(value) => document.set(&change.key, value),
                None => document.remove(&change.key),
            }
        }
        // Write next to the file and rename, so a failure never leaves half a file behind.
        let temporary = path.with_extension("json.saving");
        fs::write(&temporary, document.text())?;
        fs::rename(&temporary, &path)?;
        self.saved = self.current.clone();
        Ok(path)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("cannot read the file: {0}")]
    Read(String),
    #[error("{} changed on disk since it was loaded; reload before saving", .0.display())]
    ChangedOnDisk(PathBuf),
    #[error("cannot parse the file: {0}")]
    Document(#[from] DocumentError),
    #[error("cannot write the file: {0}")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{EditSession, SaveError};
    use crate::model::{PresetKind, RawValue};
    use crate::repository::Repository;

    const PRESET: &str = "{\n    \"type\": \"process\",\n    \"name\": \"base\",\n    \"from\": \"system\",\n    \"instantiation\": \"true\",\n    \"layer_height\": \"0.2\",\n    \"wall_loops\": \"2\"\n}";

    fn fixture() -> (tempfile::TempDir, Repository) {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Acme.json"),
            r#"{ "name": "Acme", "version": "1.0", "process_list": [{ "name": "base", "sub_path": "process/base.json" }] }"#,
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("Acme/process")).unwrap();
        fs::write(dir.path().join("Acme/process/base.json"), PRESET).unwrap();
        let repo = Repository::load(dir.path()).unwrap();
        (dir, repo)
    }

    fn s(text: &str) -> Option<RawValue> {
        Some(RawValue::Str(text.into()))
    }

    #[test]
    fn undo_and_redo_restore_values() {
        let (_dir, repo) = fixture();
        let id = repo.find(PresetKind::Process, "base")[0];
        let mut session = EditSession::new(&repo, id);
        assert!(!session.set("wall_loops", s("2")), "setting the current value is not a change");
        assert!(session.set("wall_loops", s("3")));
        assert!(session.set("layer_height", None));
        assert!(session.set("sparse_infill_density", s("20%")));
        assert!(session.is_dirty());
        assert_eq!(
            session.pending().iter().map(|change| change.key.as_str()).collect::<Vec<_>>(),
            ["layer_height", "wall_loops", "sparse_infill_density"]
        );

        assert_eq!(session.undo().as_deref(), Some("sparse_infill_density"));
        assert_eq!(session.undo().as_deref(), Some("layer_height"));
        assert_eq!(session.params().get("layer_height"), s("0.2").as_ref());
        assert_eq!(session.redo().as_deref(), Some("layer_height"));
        assert!(session.params().get("layer_height").is_none());
        assert!(session.set("wall_loops", s("2")));
        assert!(!session.can_redo(), "a new change clears the redo history");
        assert_eq!(session.undo().as_deref(), Some("wall_loops"));
        assert!(session.is_modified("wall_loops"));
    }

    #[test]
    fn saves_only_the_changed_lines() {
        let (dir, repo) = fixture();
        let id = repo.find(PresetKind::Process, "base")[0];
        let mut session = EditSession::new(&repo, id);
        session.set("wall_loops", s("3"));
        session.set("layer_height", None);
        session.set("enable_support", s("1"));
        let path = session.save(&repo).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            PRESET
                .replace("    \"layer_height\": \"0.2\",\n", "")
                .replace("\"wall_loops\": \"2\"", "\"wall_loops\": \"3\",\n    \"enable_support\": \"1\"")
        );
        assert!(!session.is_dirty());
        assert!(!dir.path().join("Acme/process/base.json.saving").exists());

        // A second save in the same session builds on the first.
        session.set("wall_loops", s("4"));
        session.save(&repo).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("\"wall_loops\": \"4\""));

        let reloaded = Repository::load(dir.path()).unwrap();
        let id = reloaded.find(PresetKind::Process, "base")[0];
        assert_eq!(reloaded.preset(id).params, *session.params());
    }

    #[test]
    fn refuses_to_overwrite_changes_on_disk() {
        let (dir, repo) = fixture();
        let id = repo.find(PresetKind::Process, "base")[0];
        let mut session = EditSession::new(&repo, id);
        session.set("wall_loops", s("3"));
        let path = dir.path().join("Acme/process/base.json");
        fs::write(&path, PRESET.replace("\"0.2\"", "\"0.28\"")).unwrap();
        assert!(matches!(session.save(&repo), Err(SaveError::ChangedOnDisk(_))));
        assert!(fs::read_to_string(&path).unwrap().contains("\"0.28\""));
    }

    #[test]
    fn rebase_preserves_saved_baseline_pending_edits_and_both_history_stacks() {
        let (dir, repo) = fixture();
        let id = repo.find(PresetKind::Process, "base")[0];
        let mut session = EditSession::new(&repo, id);
        session.set("wall_loops", s("3"));
        session.save(&repo).unwrap();
        session.set("layer_height", s("0.3"));
        session.set("wall_loops", s("4"));
        session.undo();
        let reloaded = Repository::load(dir.path()).unwrap();
        session.rebase(&reloaded, reloaded.find(PresetKind::Process, "base")[0]).unwrap();
        assert!(session.is_modified("layer_height"));
        assert!(!session.is_modified("wall_loops"));
        session.redo();
        assert_eq!(session.params().get("wall_loops"), s("4").as_ref());
        session.undo();
        session.undo();
        assert!(!session.is_dirty());
        session.undo();
        assert_eq!(session.params().get("wall_loops"), s("2").as_ref());
        assert!(session.is_dirty(), "undo across saving is relative to the new saved baseline");
        session.save(&reloaded).unwrap();
    }

    #[test]
    fn rebase_rejects_external_values_without_mutating_history() {
        let (dir, repo) = fixture();
        let id = repo.find(PresetKind::Process, "base")[0];
        let mut session = EditSession::new(&repo, id);
        session.set("wall_loops", s("3"));
        fs::write(dir.path().join("Acme/process/base.json"), PRESET.replace("\"0.2\"", "\"0.28\"")).unwrap();
        let reloaded = Repository::load(dir.path()).unwrap();
        assert!(matches!(session.rebase(&reloaded, id), Err(SaveError::ChangedOnDisk(_))));
        session.undo();
        assert!(!session.is_dirty());
        session.redo();
        assert_eq!(session.params().get("wall_loops"), s("3").as_ref());
    }
}
