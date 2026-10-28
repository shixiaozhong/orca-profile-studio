//! Read-only comparisons of effective preset values, retaining both inheritance traces.

use indexmap::IndexMap;

use crate::{KeyTrace, PresetId, PresetKind, RawValue, Repository};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("cannot compare {left} and {right} presets")]
pub struct DiffError {
    pub left: PresetKind,
    pub right: PresetKind,
}

/// One key in the union of both presets. Missing values differ from empty strings or lists.
#[derive(Debug, Clone)]
pub struct KeyDiff<'a> {
    pub key: &'a str,
    pub left: Option<KeyTrace<'a>>,
    pub right: Option<KeyTrace<'a>>,
}

impl KeyDiff<'_> {
    /// Compares raw effective values (including string/array shape), ignoring their origins.
    /// Equal values can still have different sources; both traces are always retained.
    pub fn is_different(&self) -> bool {
        self.left.as_ref().map(|trace| trace.effective().value)
            != self.right.as_ref().map(|trace| trace.effective().value)
    }
}

impl Repository {
    /// Compares same-kind presets, including across vendors. Keys follow the left trace's
    /// order, with right-only keys appended in their trace order.
    pub fn diff(&self, left: PresetId, right: PresetId) -> Result<Vec<KeyDiff<'_>>, DiffError> {
        self.diff_with(left, &self.preset(left).params, right, &self.preset(right).params)
    }

    /// Like [`Repository::diff`], with each side's own values replaced by an edit session.
    /// Ancestors use the loaded repository snapshot, just as in [`Repository::trace_with`].
    pub fn diff_with<'a>(
        &'a self,
        left: PresetId,
        left_own: &'a IndexMap<String, RawValue>,
        right: PresetId,
        right_own: &'a IndexMap<String, RawValue>,
    ) -> Result<Vec<KeyDiff<'a>>, DiffError> {
        let (left_kind, right_kind) = (self.preset(left).kind, self.preset(right).kind);
        if left_kind != right_kind {
            return Err(DiffError { left: left_kind, right: right_kind });
        }
        let mut rows = IndexMap::new();
        for trace in self.trace_with(left, left_own) {
            rows.insert(trace.key, KeyDiff { key: trace.key, left: Some(trace), right: None });
        }
        for trace in self.trace_with(right, right_own) {
            let key = trace.key;
            rows.entry(key).or_insert_with(|| KeyDiff { key, left: None, right: None }).right = Some(trace);
        }
        Ok(rows.into_values().collect())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::{EditSession, PresetKind, RawValue, Repository};

    fn fixture() -> (tempfile::TempDir, Repository) {
        let dir = tempfile::tempdir().unwrap();
        for vendor in ["A", "B"] {
            fs::create_dir_all(dir.path().join(vendor)).unwrap();
            fs::write(
                dir.path().join(format!("{vendor}.json")),
                r#"{
                "version":"01.00.00.00", "process_list":[
                    {"name":"base","sub_path":"base.json"}, {"name":"child","sub_path":"child.json"}],
                "filament_list":[{"name":"filament","sub_path":"filament.json"}] }"#,
            )
            .unwrap();
            fs::write(
                dir.path().join(vendor).join("base.json"),
                r#"{
                "name":"base", "type":"process", "instantiation":"false", "wall_loops":"2" }"#,
            )
            .unwrap();
            let extra = if vendor == "A" {
                r#""left_only":"", "array":["1","2"], "shape":"1", "empty":"""#
            } else {
                r#""right_only":[], "array":["1","3"], "wall_loops":"2", "shape":["1"], "empty":[]"#
            };
            fs::write(
                dir.path().join(vendor).join("child.json"),
                format!(r#"{{"name":"child", "type":"process", "inherits":"base", {extra}}}"#),
            )
            .unwrap();
            fs::write(dir.path().join(vendor).join("filament.json"), r#"{"name":"filament","type":"filament"}"#)
                .unwrap();
        }
        let repo = Repository::load(dir.path()).unwrap();
        (dir, repo)
    }

    #[test]
    fn compares_union_arrays_and_origins_across_vendors() {
        let (_dir, repo) = fixture();
        let ids = repo.find(PresetKind::Process, "child");
        let rows = repo.diff(ids[0], ids[1]).unwrap();
        let row = |key| rows.iter().find(|row| row.key == key).unwrap();
        assert!(row("left_only").right.is_none());
        assert!(row("right_only").left.is_none());
        assert!(row("left_only").is_different());
        assert!(row("right_only").is_different());
        assert!(row("array").is_different());
        assert!(row("shape").is_different());
        assert!(row("empty").is_different());
        let same = row("wall_loops");
        assert!(!same.is_different());
        assert_ne!(same.left.as_ref().unwrap().effective().origin, same.right.as_ref().unwrap().effective().origin);
        assert_eq!(same.right.as_ref().unwrap().layers.len(), 2);
        assert_eq!(rows.last().unwrap().key, "right_only");
        assert!(repo.diff(ids[0], ids[0]).unwrap().iter().all(|row| !row.is_different()));
        let filament = repo.find(PresetKind::Filament, "filament")[0];
        assert!(repo.diff(ids[0], filament).is_err());
    }

    #[test]
    fn includes_both_edit_sessions_without_mutating_repository() {
        let (_dir, repo) = fixture();
        let ids = repo.find(PresetKind::Process, "child");
        let mut left = EditSession::new(&repo, ids[0]);
        let mut right = EditSession::new(&repo, ids[1]);
        left.set("wall_loops", Some(RawValue::Str("4".into())));
        right.set("wall_loops", Some(RawValue::Str("5".into())));
        right.set("right_only", None);
        let rows = repo.diff_with(ids[0], left.params(), ids[1], right.params()).unwrap();
        let row = rows.iter().find(|row| row.key == "wall_loops").unwrap();
        assert_eq!(row.left.as_ref().unwrap().effective().value.as_scalar(), Some("4"));
        assert_eq!(row.right.as_ref().unwrap().effective().value.as_scalar(), Some("5"));
        assert!(row.is_different());
        assert!(!rows.iter().any(|row| row.key == "right_only"));
        // Keys no file sets are absent: their defaults live in OrcaSlicer's sources.
        assert!(!rows.iter().any(|row| row.key == "layer_height"));
        assert_eq!(repo.lookup_scalar(ids[0], "wall_loops"), Some("2"));
        left.set("wall_loops", None);
        right.set("wall_loops", None);
        let rows = repo.diff_with(ids[0], left.params(), ids[1], right.params()).unwrap();
        assert!(!rows.iter().find(|row| row.key == "wall_loops").unwrap().is_different());
    }
}
