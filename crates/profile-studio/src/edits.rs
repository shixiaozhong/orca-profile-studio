//! Unsaved edits of the loaded profiles: one [`EditSession`] per edited preset.
//!
//! An entity shared by the app (dirty marks in the tree, refreshing after saves) and the Values tab
//! (editing), so edits survive switching between presets.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::EventEmitter;
use profile_core::{EditSession, SaveError};

use crate::catalog::Catalog;

pub struct Edits {
    catalog: Arc<Catalog>,
    /// Keyed by preset index.
    sessions: BTreeMap<usize, EditSession>,
}

pub enum EditsEvent {
    Saved,
}

impl EventEmitter<EditsEvent> for Edits {}

#[derive(Clone)]
#[non_exhaustive]
pub struct RefreshConflict {
    pub path: PathBuf,
    pub reason: &'static str,
}

impl Edits {
    pub fn new(catalog: Arc<Catalog>) -> Self {
        Edits { catalog, sessions: BTreeMap::new() }
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
                None => Some("File is missing or could not be loaded"),
                Some(new) if old.kind != new.kind || old.meta != new.meta => Some("Preset metadata changed on disk"),
                Some(new) => match session.rebase(&catalog.repo, new.id) {
                    Ok(()) => {
                        sessions.insert(new.id.index(), session);
                        None
                    }
                    Err(_) => Some("Values changed on disk"),
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
