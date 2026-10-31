//! Read-only comparisons of effective preset values, retaining both inheritance traces.

use indexmap::IndexMap;

use crate::{KeyTrace, PresetId, PresetKind, RawValue, Repository};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("cannot compare {expected} and {found} presets")]
pub struct DiffError {
    /// The kind of the first preset.
    pub expected: PresetKind,
    pub found: PresetKind,
}

/// One key in the union of all compared presets, with each preset's trace in the order the
/// presets were given. Missing values differ from empty strings or lists.
#[derive(Debug, Clone)]
pub struct KeyDiff<'a> {
    pub key: &'a str,
    pub traces: Vec<Option<KeyTrace<'a>>>,
}

impl KeyDiff<'_> {
    fn value(&self, preset: usize) -> Option<&RawValue> {
        self.traces[preset].as_ref().map(|trace| trace.effective().value)
    }

    /// Whether the raw effective values (including string/array shape) are not all equal,
    /// ignoring their origins. Equal values can still have different sources.
    pub fn is_different(&self) -> bool {
        (1..self.traces.len()).any(|preset| self.differs_from_first(preset))
    }

    /// Whether the effective value of the `preset`-th preset differs from the first one's.
    pub fn differs_from_first(&self, preset: usize) -> bool {
        self.value(preset) != self.value(0)
    }
}

impl Repository {
    /// Compares same-kind presets, including across vendors. Keys follow the first trace's
    /// order, with keys only later presets set appended in their trace order.
    pub fn diff(&self, presets: &[PresetId]) -> Result<Vec<KeyDiff<'_>>, DiffError> {
        let presets: Vec<_> = presets.iter().map(|&id| (id, &self.preset(id).params)).collect();
        self.diff_with(&presets)
    }

    /// Like [`Repository::diff`], with each preset's own values replaced, e.g. by an edit session.
    /// Ancestors use the loaded repository snapshot, just as in [`Repository::trace_with`].
    pub fn diff_with<'a>(
        &'a self,
        presets: &[(PresetId, &'a IndexMap<String, RawValue>)],
    ) -> Result<Vec<KeyDiff<'a>>, DiffError> {
        let Some(&(first, _)) = presets.first() else { return Ok(Vec::new()) };
        let expected = self.preset(first).kind;
        if let Some(&(other, _)) = presets.iter().find(|(id, _)| self.preset(*id).kind != expected) {
            return Err(DiffError { expected, found: self.preset(other).kind });
        }
        let mut rows: IndexMap<&str, KeyDiff<'a>> = IndexMap::new();
        for (ix, &(id, own)) in presets.iter().enumerate() {
            for trace in self.trace_with(id, own) {
                let key = trace.key;
                let row = rows.entry(key).or_insert_with(|| KeyDiff { key, traces: vec![None; presets.len()] });
                row.traces[ix] = Some(trace);
            }
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
        let rows = repo.diff(ids).unwrap();
        let row = |key| rows.iter().find(|row| row.key == key).unwrap();
        assert!(row("left_only").traces[1].is_none());
        assert!(row("right_only").traces[0].is_none());
        assert!(row("left_only").is_different());
        assert!(row("right_only").is_different());
        assert!(row("array").is_different());
        assert!(row("shape").is_different());
        assert!(row("empty").is_different());
        let same = row("wall_loops");
        assert!(!same.is_different());
        let origin = |ix: usize| same.traces[ix].as_ref().unwrap().effective().origin;
        assert_ne!(origin(0), origin(1));
        assert_eq!(same.traces[1].as_ref().unwrap().layers.len(), 2);
        assert_eq!(rows.last().unwrap().key, "right_only");
        assert!(repo.diff(&[ids[0], ids[0]]).unwrap().iter().all(|row| !row.is_different()));
        let filament = repo.find(PresetKind::Filament, "filament")[0];
        let err = repo.diff(&[ids[0], filament]).unwrap_err();
        assert_eq!((err.expected, err.found), (PresetKind::Process, PresetKind::Filament));
    }

    #[test]
    fn compares_several_presets_against_the_first() {
        let (_dir, repo) = fixture();
        let ids = repo.find(PresetKind::Process, "child");
        let base = repo.find(PresetKind::Process, "base");
        let rows = repo.diff(&[ids[0], ids[1], base[0]]).unwrap();
        let row = |key| rows.iter().find(|row| row.key == key).unwrap();
        // Every preset inherits wall_loops = 2 or sets it to 2.
        let walls = row("wall_loops");
        assert!(!walls.is_different() && walls.traces.iter().all(Option::is_some));
        // The base has no array: missing differs from the first preset's value.
        let array = row("array");
        assert!(array.is_different() && array.differs_from_first(1) && array.differs_from_first(2));
        assert!(array.traces[2].is_none());
        let left_only = row("left_only");
        assert!(left_only.differs_from_first(1) && left_only.differs_from_first(2));
        assert_eq!(repo.diff(&[ids[0]]).unwrap().iter().filter(|row| row.is_different()).count(), 0);
        assert!(repo.diff(&[]).unwrap().is_empty());
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
        let rows = repo.diff_with(&[(ids[0], left.params()), (ids[1], right.params())]).unwrap();
        let row = rows.iter().find(|row| row.key == "wall_loops").unwrap();
        assert_eq!(row.traces[0].as_ref().unwrap().effective().value.as_scalar(), Some("4"));
        assert_eq!(row.traces[1].as_ref().unwrap().effective().value.as_scalar(), Some("5"));
        assert!(row.is_different());
        assert!(!rows.iter().any(|row| row.key == "right_only"));
        // Keys no file sets are absent: their defaults live in OrcaSlicer's sources.
        assert!(!rows.iter().any(|row| row.key == "layer_height"));
        assert_eq!(repo.lookup_scalar(ids[0], "wall_loops"), Some("2"));
        left.set("wall_loops", None);
        right.set("wall_loops", None);
        let rows = repo.diff_with(&[(ids[0], left.params()), (ids[1], right.params())]).unwrap();
        assert!(!rows.iter().find(|row| row.key == "wall_loops").unwrap().is_different());
    }
}
