//! Immutable previews calculated off the UI thread from a coherent set of edit sessions.
use crate::{catalog::Catalog, details::format_value, edits::Edits};
use indexmap::IndexMap;
use profile_core::{PresetId, PresetMeta, RawValue};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Set,
    Remove,
    AddItem,
    RemoveItem,
    Promote,
}
impl Operation {
    pub const ALL: [Self; 5] = [Self::Set, Self::Remove, Self::AddItem, Self::RemoveItem, Self::Promote];
    pub fn label(self) -> String {
        match self {
            Self::Set => t!("batch.set"),
            Self::Remove => t!("batch.remove"),
            Self::AddItem => t!("batch.add_item"),
            Self::RemoveItem => t!("batch.remove_item"),
            Self::Promote => t!("batch.promote"),
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
    selected_count: usize,
    impact_count: usize,
}
impl BatchPlan {
    pub fn rows(&self) -> &[PreviewRow] {
        &self.rows
    }
    pub fn selected_count(&self) -> usize {
        self.selected_count
    }
    pub fn impact_count(&self) -> usize {
        self.impact_count
    }
    pub fn changed_count(&self) -> usize {
        self.changes.len()
    }
    pub fn is_current(&self, edits: &Edits) -> bool {
        self.baseline.same_values(edits)
    }
    pub fn apply(&self, edits: &mut Edits) -> Result<Option<u64>, String> {
        if !self.is_current(edits) {
            return Err(t!("batch.stale_baseline"));
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
        let kind = selected.first().map(|id| repo.preset(*id).kind).ok_or(t!("batch.select_required"))?;
        if key.is_empty() || PresetMeta::is_key(&key) {
            return Err(t!("batch.invalid_key"));
        }
        if selected.iter().any(|id| repo.preset(*id).kind != kind) {
            return Err(t!("batch.same_kind"));
        }
        if operation == Operation::Promote {
            return Self::promote(catalog, baseline, selected, key);
        }
        let array = matches!(operation, Operation::AddItem | Operation::RemoveItem);
        if operation != Operation::Remove && value.is_none() {
            return Err(t!("batch.value_required"));
        }
        let item = if array {
            let item = value.as_ref().and_then(RawValue::as_scalar).ok_or(t!("batch.one_item"))?;
            if item.is_empty() {
                return Err(t!("batch.nonempty_item"));
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
                Operation::Promote => unreachable!("promotion has its own plan"),
                Operation::AddItem | Operation::RemoveItem => {
                    // A preset without its own list follows its (possibly edited) ancestors.
                    let base = if own.is_some() { effective.clone() } else { inherited.clone() };
                    let Some((RawValue::List(mut items), _)) = base else {
                        return Err(t!("batch.not_array", arg0 = repo.preset(id).name(), key = key));
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
                t!("batch.inherits_skip")
            } else if !changed {
                t!("batch.unchanged")
            } else if own.is_none() && next.is_some() {
                t!("batch.new_override")
            } else {
                t!("batch.will_change")
            };
            candidate.session_mut(ix).set(&key, next.clone());
            let new_issues =
                new_issues(catalog, id, (&baseline, params), (&candidate, candidate.session(ix).unwrap().params()));
            if changed {
                changes.push((ix, key.clone(), next));
            }
            rows.push(PreviewRow {
                preset: ix,
                cells: [
                    format!("{} / {}", repo.vendor(repo.preset(id).vendor).name, repo.preset(id).name()),
                    describe(effective),
                    String::new(),
                    state,
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
        Ok(Self { baseline, changes, selected_count: rows.len(), impact_count: 0, rows })
    }
    fn promote(catalog: &Arc<Catalog>, baseline: Edits, selected: Vec<PresetId>, key: String) -> Result<Self, String> {
        let repo = &catalog.repo;
        let plan = profile_core::PromotionPlan::with_values(repo, &selected, &key, |id| {
            baseline.session(id.index()).map_or(&repo.preset(id).params, |session| session.params())
        })
        .map_err(|error| match error {
            profile_core::PromotionError::NeedSiblings => t!("promote.need_siblings"),
            profile_core::PromotionError::DifferentParents => t!("promote.same_parent"),
            profile_core::PromotionError::IncompleteChain => t!("promote.incomplete"),
            profile_core::PromotionError::CrossVendor => t!("promote.cross_vendor"),
            profile_core::PromotionError::SharedLibrary => t!("promote.shared_library"),
            profile_core::PromotionError::MissingOwnValue => t!("promote.own_required"),
            profile_core::PromotionError::DifferentValues => t!("promote.same_value"),
        })?;
        let changes =
            plan.changes().iter().map(|(id, value)| (id.index(), key.clone(), value.clone())).collect::<Vec<_>>();
        let mut candidate = baseline.clone();
        for (id, _, value) in &changes {
            candidate.session_mut(*id).set(&key, value.clone());
        }
        let mut targets = vec![(
            plan.parent(),
            if changes.iter().any(|(ix, _, _)| *ix == plan.parent().index()) {
                t!("promote.parent_changed")
            } else {
                t!("promote.parent_unchanged")
            },
        )];
        let mut children = selected.clone();
        children.sort_unstable();
        targets.extend(children.into_iter().map(|id| (id, t!("promote.child"))));
        targets.extend(plan.impacts().iter().map(|impact| (impact.preset(), t!("promote.impact"))));
        // One chain walk per impacted preset, counted for each row's preset among its ancestors.
        let mut descendants: BTreeMap<PresetId, usize> = BTreeMap::new();
        for impact in plan.impacts() {
            for ancestor in repo.chain(impact.preset()).into_iter().skip(1) {
                *descendants.entry(ancestor).or_default() += 1;
            }
        }
        let rows = targets
            .into_iter()
            .map(|(id, state)| {
                let ix = id.index();
                let preset = repo.preset(id);
                let before = baseline.session(ix).map_or(&preset.params, |s| s.params());
                let after = candidate.session(ix).map_or(&preset.params, |s| s.params());
                let issues = new_issues(catalog, id, (&baseline, before), (&candidate, after));
                let descendants = descendants.get(&id).copied().unwrap_or(0);
                PreviewRow {
                    preset: ix,
                    cells: [
                        format!("{} / {}", repo.vendor(preset.vendor).name, preset.name()),
                        describe(baseline.effective(ix, &key)),
                        describe(candidate.effective(ix, &key)),
                        state,
                        issues,
                        descendants.to_string(),
                        preset.path.display().to_string(),
                    ],
                }
            })
            .collect();
        Ok(Self { baseline, changes, rows, selected_count: selected.len(), impact_count: plan.impacts().len() })
    }
}
/// Redundant overrides that `after` has and `before` lacks, each against its own parent values.
fn new_issues(
    catalog: &Catalog,
    id: PresetId,
    before: (&Edits, &IndexMap<String, RawValue>),
    after: (&Edits, &IndexMap<String, RawValue>),
) -> String {
    let repo = &catalog.repo;
    let Some(parent) = repo.preset(id).parent else { return String::new() };
    let redundant = |(edits, params): (&Edits, &IndexMap<String, RawValue>)| {
        repo.redundant_overrides_against(id, params, |key| edits.effective(parent.index(), key).map(|(value, _)| value))
    };
    let existing = redundant(before);
    redundant(after)
        .into_iter()
        .filter(|issue| !existing.contains(issue))
        .map(|issue| format!("{}: {}", crate::i18n::severity(issue.severity()), crate::issue_text::issue(&issue)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn describe(value: Option<(RawValue, String)>) -> String {
    // The same rendering as Values, Compare and Details.
    value.map_or(t!("common.unset"), |(value, source)| format!("{} — {source}", format_value(&value)))
}

#[cfg(test)]
mod tests {
    use super::{BatchPlan, Operation};
    use crate::{catalog::Catalog, edits::Edits};
    use profile_core::{PresetKind, RawValue, Repository};
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
        Arc::new(Catalog::new(Repository::load(path).unwrap(), Duration::ZERO))
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
        let _locale = crate::i18n::test_locale();
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
        let _locale = crate::i18n::test_locale();
        let (_dir, catalog) = fixture();
        let mut edits = Edits::new(catalog.clone());
        let a = id(&catalog, "a").index();
        edits.session_mut(a).set("wall_loops", text("4"));
        let preview = plan(&catalog, &edits, &["base", "a"], "wall_loops", Operation::Remove, None);
        assert!(preview.rows().iter().all(|r| r.cell(2) == "(unset)"));
        // `a` keeps its own value, now equal to what it inherits from the edited `base`.
        let redundant = plan(&catalog, &edits, &["base", "a"], "wall_loops", Operation::Set, text("4"));
        assert_eq!(redundant.rows()[0].cell(4), "");
        assert!(redundant.rows()[1].cell(4).starts_with("info: "), "{}", redundant.rows()[1].cell(4));
        redundant.apply(&mut edits).unwrap();
        let same = plan(&catalog, &edits, &["base", "a"], "wall_loops", Operation::Set, text("4"));
        assert_eq!(same.changed_count(), 0);
        assert!(same.rows().iter().all(|r| r.cell(4).is_empty()));
        assert_eq!(same.apply(&mut edits).unwrap(), None);
    }

    #[test]
    fn children_inheriting_from_a_selected_parent_are_not_overridden() {
        let _locale = crate::i18n::test_locale();
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
        let _locale = crate::i18n::test_locale();
        let (_dir, catalog) = fixture();
        let mut edits = Edits::new(catalog.clone());
        let a = id(&catalog, "a");
        let preview = plan(&catalog, &edits, &["a", "b"], "wall_loops", Operation::Set, text("5"));
        edits.session_mut(a.index()).set("layer_height", text("0.3"));
        assert!(preview.apply(&mut edits).is_err());
        assert_eq!(edits.dirty(), [a.index()]);
        for (key, operation) in
            [("wall_loops", Operation::AddItem), ("name", Operation::Set), ("Inherits", Operation::Remove)]
        {
            assert!(BatchPlan::build(&catalog, edits.clone(), vec![a], key.into(), operation, text("x")).is_err());
        }
    }

    #[test]
    fn grouped_history_is_atomic_across_interleaved_edits_save_reload_and_discard() {
        let _locale = crate::i18n::test_locale();
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
    fn values_redundancy_hint_follows_unsaved_parent_edits() {
        let _locale = crate::i18n::test_locale();
        let (_dir, catalog) = fixture();
        let mut edits = Edits::new(catalog.clone());
        let base = id(&catalog, "base").index();
        let b = id(&catalog, "b").index();
        let redundant = |edits: &Edits| {
            crate::values::build_rows(&catalog, edits, b)
                .into_iter()
                .find(|row| row.key == "wall_loops")
                .is_some_and(|row| row.issues.iter().any(|(severity, _)| *severity == profile_core::Severity::Info))
        };
        edits.session_mut(b).set("wall_loops", text("2"));
        assert!(redundant(&edits), "equal to the parent's saved value");
        // The parent is edited but not saved: the child's value is no longer what it inherits.
        edits.session_mut(base).set("wall_loops", text("4"));
        assert!(!redundant(&edits));
        edits.session_mut(b).set("wall_loops", text("4"));
        assert!(redundant(&edits), "equal to the parent's unsaved value");
    }

    #[test]
    fn promotion_groups_parent_and_children_and_rejects_stale_ancestor_edits() {
        let _locale = crate::i18n::test_locale();
        let (dir, catalog) = fixture();
        let mut edits = Edits::new(catalog.clone());
        let parent = id(&catalog, "base").index();
        for name in ["a", "b"] {
            edits.session_mut(id(&catalog, name).index()).set("wall_loops", text("5"));
        }
        let preview = plan(&catalog, &edits, &["a", "b"], "wall_loops", Operation::Promote, None);
        assert_eq!(preview.selected_count(), 2);
        assert_eq!(preview.changed_count(), 3);
        assert_eq!(preview.rows().len(), 3, "c inherits a, whose effective value stays unchanged");
        assert!(preview.rows()[0].cell(3).contains("Parent"));
        let original = std::fs::read_to_string(dir.path().join("Acme/process/base.json")).unwrap();
        let group = preview.apply(&mut edits).unwrap().unwrap();
        assert!(edits.session(id(&catalog, "a").index()).unwrap().params().get("wall_loops").is_none());
        assert_eq!(edits.effective(id(&catalog, "c").index(), "wall_loops").unwrap().0, RawValue::Str("5".into()));
        assert_eq!(std::fs::read_to_string(dir.path().join("Acme/process/base.json")).unwrap(), original);
        edits.session_mut(parent).set("layer_height", text("0.3"));
        assert!(edits.history_group(group, false).is_err());
        edits.history(parent, false).unwrap();
        assert_eq!(edits.history_group(group, false).unwrap(), 3);
        assert_eq!(edits.effective(parent, "wall_loops").unwrap().0, RawValue::Str("2".into()));
        let preview = plan(&catalog, &edits, &["a", "b"], "wall_loops", Operation::Promote, None);
        edits.session_mut(parent).set("wall_loops", text("9"));
        assert!(preview.apply(&mut edits).is_err());
    }

    #[test]
    fn new_edits_after_undo_invalidate_a_whole_batch_redo() {
        let _locale = crate::i18n::test_locale();
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
