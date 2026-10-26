//! Unsaved edits of the loaded profiles: one [`EditSession`] per edited preset.
//!
//! An entity shared by the app (dirty marks in the tree, refreshing after saves) and the Values tab
//! (editing), so edits survive switching between presets.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::EventEmitter;
use profile_core::{EditSession, RawValue, SaveError};

use crate::catalog::Catalog;

#[derive(Clone)]
pub struct Edits {
    catalog: Arc<Catalog>,
    /// Keyed by preset index.
    sessions: BTreeMap<usize, EditSession>,
    groups: BTreeMap<u64, Vec<PathBuf>>,
    next_group: u64,
}

pub enum EditsEvent {
    Saved,
}

impl EventEmitter<EditsEvent> for Edits {}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RefreshConflict {
    pub path: PathBuf,
    pub reason: String,
}

impl Edits {
    pub fn new(catalog: Arc<Catalog>) -> Self {
        Edits { catalog, sessions: BTreeMap::new(), groups: BTreeMap::new(), next_group: 0 }
    }

    pub fn catalog(&self) -> &Arc<Catalog> {
        &self.catalog
    }

    /// Migration is atomic: on a conflict the window keeps its previous, coherent snapshot.
    /// Only unsaved changes conflict; a saved session whose file changed on disk loses its history.
    pub fn rebase(&mut self, catalog: &Arc<Catalog>) -> Result<(), Vec<RefreshConflict>> {
        let mut sessions = BTreeMap::new();
        let mut conflicts = Vec::new();
        for (&index, session) in &self.sessions {
            if !session.is_dirty() && !session.can_undo() && !session.can_redo() {
                continue;
            }
            let old = &self.catalog.repo.presets()[index];
            let dirty = session.is_dirty();
            let mut session = session.clone();
            let reason = match catalog.preset_at(&old.path).map(|index| &catalog.repo.presets()[index]) {
                None => Some(t!("edits.missing")),
                Some(new) if old.kind != new.kind || old.meta != new.meta => Some(t!("edits.metadata_changed")),
                Some(new) => match session.rebase(&catalog.repo, new.id) {
                    Ok(()) => {
                        sessions.insert(new.id.index(), session);
                        None
                    }
                    Err(_) => Some(t!("edits.values_changed")),
                },
            };
            if let Some(reason) = reason
                && dirty
            {
                conflicts.push(RefreshConflict { path: old.path.clone(), reason });
            }
        }
        if !conflicts.is_empty() {
            return Err(conflicts);
        }
        self.sessions = sessions;
        self.catalog = catalog.clone();
        Ok(())
    }

    pub fn discard_sessions(&mut self, paths: &[PathBuf]) {
        self.sessions.retain(|&index, _| !paths.contains(&self.catalog.repo.presets()[index].path));
    }

    pub fn session(&self, preset: usize) -> Option<&EditSession> {
        self.sessions.get(&preset)
    }

    /// The session of a preset, started on first use.
    pub fn session_mut(&mut self, preset: usize) -> &mut EditSession {
        let repo = &self.catalog.repo;
        self.sessions.entry(preset).or_insert_with(|| EditSession::new(repo, repo.presets()[preset].id))
    }

    /// One key per preset, validated before any session changes. No-op rows are excluded.
    pub fn apply_batch(&mut self, changes: &[(usize, String, Option<RawValue>)]) -> Result<Option<u64>, String> {
        let mut seen = std::collections::BTreeSet::new();
        if changes.iter().any(|(ix, _, _)| *ix >= self.catalog.repo.presets().len() || !seen.insert(*ix)) {
            return Err(t!("edits.one_change"));
        }
        self.next_group += 1;
        let group = self.next_group;
        let mut paths = Vec::new();
        for (ix, key, value) in changes {
            if self.session_mut(*ix).set_in_group(key, value.clone(), group) {
                paths.push(self.catalog.repo.presets()[*ix].path.clone());
            }
        }
        if paths.is_empty() {
            return Ok(None);
        }
        self.groups.insert(group, paths);
        Ok(Some(group))
    }

    /// Includes saved histories: a batch stays atomic across saves and repository reindexing.
    fn group_members(&self, group: u64, redo: bool) -> Result<Vec<usize>, String> {
        let paths = self.groups.get(&group).ok_or(t!("edits.no_history"))?;
        paths
            .iter()
            .map(|path| {
                let ix = self.catalog.preset_at(path).ok_or(t!("edits.missing_member"))?;
                let session = self.session(ix).ok_or(t!("edits.discarded_history"))?;
                let top = if redo { session.redo_group() } else { session.undo_group() };
                if top != Some(group) {
                    return Err(t!("edits.newer_changes"));
                }
                Ok(ix)
            })
            .collect()
    }

    pub fn history_group(&mut self, group: u64, redo: bool) -> Result<usize, String> {
        let members = self.group_members(group, redo)?;
        for &ix in &members {
            let session = self.session_mut(ix);
            if redo {
                session.redo();
            } else {
                session.undo();
            }
        }
        Ok(members.len())
    }

    pub fn history(&mut self, preset: usize, redo: bool) -> Result<(Option<String>, usize), String> {
        let session = self.session_mut(preset);
        let group = if redo { session.redo_group() } else { session.undo_group() };
        if let Some(group) = group {
            // The selection's key is not needed to execute the group; keep the current row.
            return self.history_group(group, redo).map(|count| (None, count));
        }
        let key = if redo { session.redo() } else { session.undo() };
        Ok((key, 0))
    }

    pub fn latest_group(&self, redo: bool) -> Option<u64> {
        self.sessions
            .values()
            .filter_map(|s| if redo { s.redo_group() } else { s.undo_group() })
            .reduce(|a, b| if redo { a.min(b) } else { a.max(b) })
    }

    pub fn same_values(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.catalog, &other.catalog)
            && self.sessions.keys().chain(other.sessions.keys()).all(|&ix| {
                let original = &self.catalog.repo.presets()[ix].params;
                self.session(ix).map_or(original, EditSession::params)
                    == other.session(ix).map_or(original, EditSession::params)
            })
    }

    /// Resolves through every edited ancestor, not only the selected preset's own session.
    pub fn effective(&self, preset: usize, key: &str) -> Option<(RawValue, String)> {
        let repo = &self.catalog.repo;
        let info = &repo.presets()[preset];
        for id in repo.chain(info.id) {
            let params = self.session(id.index()).map_or(&repo.preset(id).params, EditSession::params);
            if let Some(value) = params.get(key) {
                let source = if id == info.id { t!("common.this_file") } else { repo.preset(id).name().to_owned() };
                return Some((value.clone(), source));
            }
        }
        repo.schema()?.default_preset(info.kind).values.get(key).map(|v| (v.clone(), t!("common.default")))
    }

    pub fn is_dirty(&self, preset: usize) -> bool {
        self.sessions.get(&preset).is_some_and(EditSession::is_dirty)
    }

    /// Presets with unsaved changes, by index.
    pub fn dirty(&self) -> Vec<usize> {
        self.sessions.iter().filter(|(_, session)| session.is_dirty()).map(|(&preset, _)| preset).collect()
    }

    /// Saves every preset with unsaved changes; stops at the first failure.
    pub fn save_all(&mut self) -> (Vec<PathBuf>, Option<(usize, SaveError)>) {
        let repo = &self.catalog.repo;
        let mut saved = Vec::new();
        for (&preset, session) in self.sessions.iter_mut().filter(|(_, session)| session.is_dirty()) {
            match session.save(repo) {
                Ok(path) => saved.push(path),
                Err(err) => return (saved, Some((preset, err))),
            }
        }
        (saved, None)
    }
}
