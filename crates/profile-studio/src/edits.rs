//! Unsaved edits of the loaded profiles: one [`EditSession`] per edited preset.
//!
//! An entity shared by the app (dirty marks in the tree, guarding reloads) and the Values tab
//! (editing), so edits survive switching between presets.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use profile_core::{EditSession, SaveError};

use crate::catalog::Catalog;

pub struct Edits {
    catalog: Arc<Catalog>,
    /// Keyed by preset index.
    sessions: BTreeMap<usize, EditSession>,
}

impl Edits {
    pub fn new(catalog: Arc<Catalog>) -> Self {
        Edits { catalog, sessions: BTreeMap::new() }
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
    pub fn save_all(&mut self) -> Result<Vec<PathBuf>, (usize, SaveError)> {
        let repo = &self.catalog.repo;
        let mut saved = Vec::new();
        for (&preset, session) in self.sessions.iter_mut().filter(|(_, session)| session.is_dirty()) {
            saved.push(session.save(repo).map_err(|err| (preset, err))?);
        }
        Ok(saved)
    }
}
