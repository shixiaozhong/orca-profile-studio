//! Immutable previews calculated off the UI thread from a coherent set of edit sessions.
use crate::{catalog::Catalog, details::format_value, edits::Edits};
use profile_core::{OptionType, PresetId, RawValue};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Set,
    Remove,
    AddItem,
    RemoveItem,
}
impl Operation {
    pub const ALL: [Self; 4] = [Self::Set, Self::Remove, Self::AddItem, Self::RemoveItem];
    pub fn label(self) -> &'static str {
        match self {
            Self::Set => "Set value",
            Self::Remove => "Remove key (inherit)",
            Self::AddItem => "Add item",
            Self::RemoveItem => "Remove item",
        }
    }
}

pub struct PreviewRow {
    preset: usize,
    cells: [String; 7],
}
impl PreviewRow {
    pub fn cell(&self, col: usize) -> &str {
        &self.cells[col]
    }
}

pub struct BatchPlan {
    baseline: Edits,
    changes: Vec<(usize, String, Option<RawValue>)>,
    rows: Vec<PreviewRow>,
}
impl BatchPlan {
    pub fn rows(&self) -> &[PreviewRow] {
        &self.rows
    }
    pub fn changed_count(&self) -> usize {
        self.changes.len()
    }
    pub fn is_current(&self, edits: &Edits) -> bool {
        self.baseline.same_values(edits)
    }
    pub fn apply(&self, edits: &mut Edits) -> Result<Option<u64>, String> {
        if !self.is_current(edits) {
            return Err("Edits changed after this preview. Preview again.".into());
        }
        edits.apply_batch(&self.changes)
    }

    pub fn build(
        catalog: &Arc<Catalog>,
        baseline: Edits,
        selected: Vec<PresetId>,
        key: String,
        operation: Operation,
        value: Option<RawValue>,
    ) -> Result<Self, String> {
        let repo = &catalog.repo;
        let schema = repo.schema().ok_or("A schema is required for batch editing.")?;
        let kind = selected.first().map(|id| repo.preset(*id).kind).ok_or("Select at least one preset.")?;
        if !schema.belongs_to(kind, &key) {
            return Err("Choose a key belonging to the selected preset kind.".into());
        }
        if selected.iter().any(|id| repo.preset(*id).kind != kind) {
            return Err("Select presets of one kind.".into());
        }
        let array = matches!(operation, Operation::AddItem | Operation::RemoveItem);
        if array && schema.option(&key).is_none_or(|o| o.type_ != OptionType::Strings) {
            return Err("Item operations are available only for string arrays.".into());
        }
        if operation != Operation::Remove && value.is_none() {
            return Err("Enter a value first.".into());
        }
        let item = if array {
            let item = value.as_ref().and_then(RawValue::as_scalar).ok_or("Enter one array item.")?;
            if item.is_empty() {
                return Err("Enter a nonempty array item.".into());
            }
            Some(item.to_owned())
        } else {
            None
        };
        let mut candidate = baseline.clone();
        let mut changes = Vec::new();
        let mut rows = Vec::new();
        let mut selected = selected;
        // Parents load before their children, so index order proposes every ancestor's value first.
        selected.sort_unstable();
        for id in selected {
            let ix = id.index();
            let params = baseline.session(ix).map_or(&repo.preset(id).params, |s| s.params());
            let own = params.get(&key).cloned();
            let effective = baseline.effective(ix, &key);
            // What this preset would inherit once the selected ancestors are changed.
            let inherited = candidate.effective(ix, &key);
            let value_of = |v: &Option<(RawValue, String)>| v.as_ref().map(|v| v.0.clone());
            // Covered by a selected ancestor: the target value arrives through inheritance.
            let mut inherits_result = false;
            let next = match operation {
                Operation::Set if own.is_none() && value_of(&inherited) == value => {
                    inherits_result = value_of(&effective) != value;
                    None
                }
                Operation::Set => value.clone(),
                Operation::Remove => None,
                Operation::AddItem | Operation::RemoveItem => {
                    // A preset without its own list follows its (possibly edited) ancestors.
                    let base = if own.is_some() { effective.clone() } else { inherited.clone() };
                    let Some((RawValue::List(mut items), _)) = base else {
                        return Err(format!(
                            "{} has a non-array value for {key}. Set a complete value first.",
                            repo.preset(id).name()
                        ));
                    };
                    let item = item.as_ref().unwrap();
                    let changed = if operation == Operation::AddItem {
                        if items.contains(item) {
                            false
                        } else {
                            items.push(item.clone());
                            true
                        }
                    } else {
                        let old_len = items.len();
                        items.retain(|v| v != item);
                        items.len() != old_len
                    };
                    if changed {
                        Some(RawValue::List(items))
                    } else {
                        inherits_result = own.is_none() && value_of(&inherited) != value_of(&effective);
                        own.clone()
                    }
                }
            };
            let changed = own != next;
            let state = if inherits_result {
                "Inherits the new value (skipped)"
            } else if !changed {
                "Unchanged (skipped)"
            } else if own.is_none() && next.is_some() {
                "New override"
            } else {
                "Will change"
            };
            let before_issues = repo.check_values(id, params);
            candidate.session_mut(ix).set(&key, next.clone());
            let new_issues = repo
                .check_values(id, candidate.session(ix).unwrap().params())
                .into_iter()
                .filter(|issue| !before_issues.contains(issue))
                .map(|issue| format!("{}: {issue}", issue.severity()))
                .collect::<Vec<_>>()
                .join("\n");
            if changed {
                changes.push((ix, key.clone(), next));
            }
            rows.push(PreviewRow {
                preset: ix,
                cells: [
                    format!("{} / {}", repo.vendor(repo.preset(id).vendor).name, repo.preset(id).name()),
                    describe(effective),
                    String::new(),
                    state.into(),
                    new_issues,
                    String::new(),
                    repo.preset(id).path.display().to_string(),
                ],
            });
        }
        // Compute after-values only once all parents and children have their proposed own values.
        let affected = repo
            .presets()
            .iter()
            .filter(|p| p.kind == kind)
            .filter(|p| {
                baseline.effective(p.id.index(), &key).map(|v| v.0)
                    != candidate.effective(p.id.index(), &key).map(|v| v.0)
            })
            .map(|p| p.id)
            .collect::<Vec<_>>();
        // One chain walk per affected preset, counted for each selected ancestor.
        let mut descendants: BTreeMap<usize, usize> = rows.iter().map(|row| (row.preset, 0)).collect();
        for id in &affected {
            for ancestor in repo.chain(*id).into_iter().skip(1) {
                if let Some(count) = descendants.get_mut(&ancestor.index()) {
                    *count += 1;
                }
            }
        }
        for row in &mut rows {
            row.cells[2] = describe(candidate.effective(row.preset, &key));
            row.cells[5] = descendants[&row.preset].to_string();
        }
        Ok(Self { baseline, changes, rows })
    }
}
fn describe(value: Option<(RawValue, String)>) -> String {
    value.map_or("(unset)".into(), |(value, source)| format!("{} — {source}", format_value(&value)))
}

#[cfg(test)]
mod tests {
    use super::{BatchPlan, Operation};
    use crate::{catalog::Catalog, edits::Edits};
    use profile_core::{PresetKind, RawValue, Repository, Schema};
    use std::{fs, sync::Arc, time::Duration};

    fn fixture() -> (tempfile::TempDir, Arc<Catalog>) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("Acme/process")).unwrap();
        fs::write(dir.path().join("Acme.json"), r#"{"name":"Acme","version":"01.00.00.00","process_list":[{"name":"base","sub_path":"process/base.json"},{"name":"a","sub_path":"process/a.json"},{"name":"b","sub_path":"process/b.json"},{"name":"c","sub_path":"process/c.json"}]}"#).unwrap();
        for (name, extra) in [
            ("base", r#", "wall_loops":"2", "compatible_printers":["A", "B"]"#),
            ("a", r#", "inherits":"base""#),
            ("b", r#", "inherits":"base", "compatible_printers":["C", "P"]"#),
            ("c", r#", "inherits":"a""#),
        ] {
            fs::write(dir.path().join(format!("Acme/process/{name}.json")), format!("{{\"type\":\"process\",\"name\":\"{name}\",\"from\":\"system\",\"instantiation\":\"false\"{extra}}}")).unwrap();
        }
        let catalog = load(dir.path());
        (dir, catalog)
    }
    fn load(path: &std::path::Path) -> Arc<Catalog> {
        Arc::new(Catalog::new(Repository::load_with_schema(path, Schema::bundled()).unwrap(), Duration::ZERO))
    }
    fn id(catalog: &Catalog, name: &str) -> profile_core::PresetId {
        catalog.repo.find(PresetKind::Process, name)[0]
    }
    fn plan(
        catalog: &Arc<Catalog>,
        edits: &Edits,
        names: &[&str],
        key: &str,
        operation: Operation,
        value: Option<RawValue>,
    ) -> BatchPlan {
        BatchPlan::build(
            catalog,
            edits.clone(),
            names.iter().map(|n| id(catalog, n)).collect(),
            key.into(),
            operation,
            value,
        )
        .unwrap()
    }
    fn text(s: &str) -> Option<RawValue> {
        Some(RawValue::Str(s.into()))
    }

    #[test]
    fn array_preview_uses_edited_ancestors_skips_existing_items_and_preserves_files() {
        let (dir, catalog) = fixture();
        let mut edits = Edits::new(catalog.clone());
        edits
            .session_mut(id(&catalog, "base").index())
            .set("compatible_printers", Some(RawValue::List(vec!["Edited".into()])));
        let before_b = fs::read_to_string(dir.path().join("Acme/process/b.json")).unwrap();
        let preview = plan(&catalog, &edits, &["a", "b"], "compatible_printers", Operation::AddItem, text("P"));
        assert_eq!(preview.changed_count(), 1);
        assert!(preview.rows()[0].cell(1).contains("Edited"));
        assert_eq!(preview.rows()[0].cell(3), "New override");
        assert_eq!(preview.rows()[0].cell(5), "1");
        assert_eq!(preview.rows()[1].cell(3), "Unchanged (skipped)");
        preview.apply(&mut edits).unwrap();
        assert_eq!(
            edits.effective(id(&catalog, "c").index(), "compatible_printers").unwrap().0,
            RawValue::List(vec!["Edited".into(), "P".into()])
        );
        assert!(edits.save_all().1.is_none());
        assert_eq!(fs::read_to_string(dir.path().join("Acme/process/b.json")).unwrap(), before_b);
        let remove = plan(&catalog, &edits, &["a"], "compatible_printers", Operation::RemoveItem, text("P"));
        remove.apply(&mut edits).unwrap();
        assert_eq!(
            edits.effective(id(&catalog, "a").index(), "compatible_printers").unwrap().0,
            RawValue::List(vec!["Edited".into()])
        );
    }

    #[test]
    fn preview_resolves_the_complete_batch_and_only_reports_new_issues() {
        let (_dir, catalog) = fixture();
        let mut edits = Edits::new(catalog.clone());
        let a = id(&catalog, "a").index();
        edits.session_mut(a).set("wall_loops", text("4"));
        let preview = plan(&catalog, &edits, &["base", "a"], "wall_loops", Operation::Remove, None);
        assert!(preview.rows().iter().all(|r| r.cell(2).contains("default")));
        let bad = plan(&catalog, &edits, &["base", "a"], "wall_loops", Operation::Set, text("invalid"));
        assert!(bad.rows().iter().all(|r| r.cell(4).contains("fatal")));
        bad.apply(&mut edits).unwrap();
        let same = plan(&catalog, &edits, &["base", "a"], "wall_loops", Operation::Set, text("invalid"));
        assert_eq!(same.changed_count(), 0);
        assert!(same.rows().iter().all(|r| r.cell(4).is_empty()));
        assert_eq!(same.apply(&mut edits).unwrap(), None);
    }

    #[test]
    fn children_inheriting_from_a_selected_parent_are_not_overridden() {
        let (_dir, catalog) = fixture();
        let edits = Edits::new(catalog.clone());
        // `a`, `b` and `c` inherit wall_loops from `base`; the selection order does not matter.
        let preview = plan(&catalog, &edits, &["c", "a", "base"], "wall_loops", Operation::Set, text("4"));
        assert_eq!(preview.changed_count(), 1);
        let states: Vec<_> = preview.rows().iter().map(|r| r.cell(3).to_owned()).collect();
        assert_eq!(states, ["Will change", "Inherits the new value (skipped)", "Inherits the new value (skipped)"]);
        assert!(preview.rows()[1..].iter().all(|r| r.cell(2).contains("4 — base")), "{:?}", preview.rows()[1].cell(2));
        assert_eq!(preview.rows()[0].cell(5), "3");
        let preview = plan(&catalog, &edits, &["base", "a"], "compatible_printers", Operation::AddItem, text("Z"));
        assert_eq!(preview.changed_count(), 1);
        assert_eq!(preview.rows()[1].cell(3), "Inherits the new value (skipped)");
        // Already inheriting the requested value is no change at all, not a new override.
        let preview = plan(&catalog, &edits, &["a"], "wall_loops", Operation::Set, text("2"));
        assert_eq!(preview.changed_count(), 0);
        assert_eq!(preview.rows()[0].cell(3), "Unchanged (skipped)");
    }

    #[test]
    fn stale_preview_and_unsupported_item_operations_leave_sessions_unchanged() {
        let (_dir, catalog) = fixture();
        let mut edits = Edits::new(catalog.clone());
        let a = id(&catalog, "a");
        let preview = plan(&catalog, &edits, &["a", "b"], "wall_loops", Operation::Set, text("5"));
        edits.session_mut(a.index()).set("layer_height", text("0.3"));
        assert!(preview.apply(&mut edits).is_err());
        assert_eq!(edits.dirty(), [a.index()]);
        for (key, operation) in
            [("wall_loops", Operation::AddItem), ("name", Operation::Set), ("missing", Operation::Remove)]
        {
            assert!(BatchPlan::build(&catalog, edits.clone(), vec![a], key.into(), operation, text("x")).is_err());
        }
    }

    #[test]
    fn grouped_history_is_atomic_across_interleaved_edits_save_reload_and_discard() {
        let (dir, catalog) = fixture();
        let mut edits = Edits::new(catalog.clone());
        let a = id(&catalog, "a").index();
        let b = id(&catalog, "b").index();
        let preview = plan(&catalog, &edits, &["a", "b"], "wall_loops", Operation::Set, text("5"));
        let group = preview.apply(&mut edits).unwrap().unwrap();
        edits.session_mut(b).set("layer_height", text("0.3"));
        assert!(edits.history(a, false).is_err());
        assert_eq!(edits.session(a).unwrap().params()["wall_loops"], RawValue::Str("5".into()));
        edits.history(b, false).unwrap();
        assert_eq!(edits.history(a, false).unwrap().1, 2);
        assert_eq!(edits.history(b, true).unwrap().1, 2);
        edits.save_all();
        let fresh = load(dir.path());
        edits.rebase(&fresh).unwrap();
        assert_eq!(edits.history_group(group, false).unwrap(), 2);
        assert_eq!(edits.dirty().len(), 2);
        edits.history_group(group, true).unwrap();
        assert!(edits.dirty().is_empty());
        edits.discard_sessions(&[fresh.repo.presets()[b].path.clone()]);
        assert!(edits.history_group(group, false).is_err());
        assert!(!edits.is_dirty(a));
    }

    #[test]
    fn new_edits_after_undo_invalidate_a_whole_batch_redo() {
        let (_dir, catalog) = fixture();
        let mut edits = Edits::new(catalog.clone());
        let a = id(&catalog, "a").index();
        let preview = plan(&catalog, &edits, &["a", "b"], "wall_loops", Operation::Set, text("5"));
        let group = preview.apply(&mut edits).unwrap().unwrap();
        edits.history_group(group, false).unwrap();
        edits.session_mut(a).set("layer_height", text("0.3"));
        assert!(edits.history_group(group, true).is_err());
        assert_eq!(edits.dirty(), [a]);
    }
}
