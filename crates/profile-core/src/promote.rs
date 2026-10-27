//! Plan a single-key promotion without modifying files or edit sessions.
use crate::{PresetId, RawValue, Repository};
use indexmap::IndexMap;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PromotionError {
    #[error("Select at least two distinct sibling presets")]
    NeedSiblings,
    #[error("Every selected preset must have the same direct parent and kind")]
    DifferentParents,
    #[error("The inheritance chain is incomplete")]
    IncompleteChain,
    #[error("Promoting values across vendors is not allowed")]
    CrossVendor,
    #[error("Promoting into the shared filament library would change filaments of every vendor")]
    SharedLibrary,
    #[error("Every selected preset must set the key in its own file")]
    MissingOwnValue,
    #[error("The selected presets must set exactly the same value")]
    DifferentValues,
}

#[derive(Debug)]
pub struct PromotionImpact {
    preset: PresetId,
    before: Option<RawValue>,
    after: Option<RawValue>,
}
impl PromotionImpact {
    pub fn preset(&self) -> PresetId {
        self.preset
    }
    pub fn before(&self) -> Option<&RawValue> {
        self.before.as_ref()
    }
    pub fn after(&self) -> Option<&RawValue> {
        self.after.as_ref()
    }
}

/// Own-value changes plus every unselected descendant whose effective value changes.
#[derive(Debug)]
pub struct PromotionPlan {
    parent: PresetId,
    changes: Vec<(PresetId, Option<RawValue>)>,
    impacts: Vec<PromotionImpact>,
}
impl PromotionPlan {
    pub fn new(repo: &Repository, selected: &[PresetId], key: &str) -> Result<Self, PromotionError> {
        Self::with_values(repo, selected, key, |id| &repo.preset(id).params)
    }
    /// Uses the caller's current own values for every preset, including edited ancestors and
    /// unselected descendants. The caller owns staleness checking, undo grouping and saving.
    pub fn with_values<'a>(
        repo: &'a Repository,
        selected: &[PresetId],
        key: &str,
        own: impl Fn(PresetId) -> &'a IndexMap<String, RawValue>,
    ) -> Result<Self, PromotionError> {
        let ids: BTreeSet<_> = selected.iter().copied().collect();
        if ids.len() < 2 || ids.len() != selected.len() {
            return Err(PromotionError::NeedSiblings);
        }
        let paths: BTreeSet<_> = ids.iter().map(|&id| &repo.preset(id).path).collect();
        if paths.len() != ids.len() {
            return Err(PromotionError::NeedSiblings);
        }
        let first = repo.preset(*ids.first().unwrap());
        let parent = first.parent.ok_or(PromotionError::DifferentParents)?;
        if paths.contains(&repo.preset(parent).path)
            || ids.iter().any(|&id| repo.preset(id).parent != Some(parent) || repo.preset(id).kind != first.kind)
        {
            return Err(PromotionError::DifferentParents);
        }
        if ids.iter().any(|&id| repo.preset(id).vendor != repo.preset(parent).vendor) {
            return Err(PromotionError::CrossVendor);
        }
        if repo.vendor(repo.preset(parent).vendor).name == crate::FILAMENT_LIBRARY {
            return Err(PromotionError::SharedLibrary);
        }
        if ids.iter().any(|&id| repo.depth(id).is_none()) {
            return Err(PromotionError::IncompleteChain);
        }
        let value = own(first.id).get(key).ok_or(PromotionError::MissingOwnValue)?;
        for &id in &ids {
            match own(id).get(key) {
                None => return Err(PromotionError::MissingOwnValue),
                Some(other) if other != value => return Err(PromotionError::DifferentValues),
                Some(_) => {}
            }
        }
        let mut changes = Vec::new();
        if own(parent).get(key) != Some(value) {
            changes.push((parent, Some(value.clone())));
        }
        changes.extend(ids.iter().map(|&id| (id, None)));
        let proposed: BTreeMap<_, _> = changes.iter().map(|(id, value)| (*id, value.as_ref())).collect();
        let effective = |id, after| {
            for ancestor in repo.chain(id) {
                let value = if after {
                    proposed.get(&ancestor).copied().unwrap_or_else(|| own(ancestor).get(key))
                } else {
                    own(ancestor).get(key)
                };
                if let Some(value) = value {
                    return Some(value.clone());
                }
            }
            repo.schema().and_then(|schema| schema.default_preset(repo.preset(id).kind).values.get(key)).cloned()
        };
        let impacts = repo
            .presets()
            .iter()
            .filter(|preset| {
                preset.id != parent && !ids.contains(&preset.id) && repo.chain(preset.id).contains(&parent)
            })
            .filter_map(|preset| {
                let before = effective(preset.id, false);
                let after = effective(preset.id, true);
                (before != after).then_some(PromotionImpact { preset: preset.id, before, after })
            })
            .collect();
        Ok(Self { parent, changes, impacts })
    }
    pub fn parent(&self) -> PresetId {
        self.parent
    }
    pub fn changes(&self) -> &[(PresetId, Option<RawValue>)] {
        &self.changes
    }
    pub fn impacts(&self) -> &[PromotionImpact] {
        &self.impacts
    }
}

#[cfg(test)]
mod tests {
    use super::{PromotionError, PromotionPlan};
    use crate::{IssueKind, PresetId, PresetKind, RawValue, Repository, Schema, Severity};
    use serde_json::json;
    use std::{collections::BTreeMap, fs};

    fn fixture() -> (tempfile::TempDir, Repository) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("Acme/process")).unwrap();
        let entries = [
            ("base", None, Some("1")),
            ("a", Some("base"), Some("3")),
            ("b", Some("base"), Some("3")),
            ("other", Some("base"), None),
            ("deep", Some("other"), None),
            ("shield", Some("base"), Some("9")),
            ("selected_child", Some("a"), None),
        ];
        let list = entries
            .iter()
            .map(|(name, _, _)| json!({"name":name,"sub_path":format!("process/{name}.json")}))
            .collect::<Vec<_>>();
        fs::write(dir.path().join("Acme.json"), json!({"name":"Acme","version":"1.0","process_list":list}).to_string())
            .unwrap();
        for (name, parent, value) in entries {
            let mut data = json!({"type":"process","name":name,"from":"system","instantiation":"false"});
            if let Some(parent) = parent {
                data["inherits"] = parent.into();
            }
            if let Some(value) = value {
                data["wall_loops"] = value.into();
            }
            fs::write(dir.path().join(format!("Acme/process/{name}.json")), data.to_string()).unwrap();
        }
        let repo = Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap();
        (dir, repo)
    }
    fn id(repo: &Repository, name: &str) -> PresetId {
        repo.find(PresetKind::Process, name)[0]
    }

    #[test]
    fn promotion_lists_only_unselected_descendants_whose_final_value_changes() {
        let (dir, repo) = fixture();
        let before = fs::read_to_string(dir.path().join("Acme/process/base.json")).unwrap();
        let plan = PromotionPlan::new(&repo, &[id(&repo, "b"), id(&repo, "a")], "wall_loops").unwrap();
        assert_eq!(plan.parent(), id(&repo, "base"));
        assert_eq!(plan.changes().len(), 3);
        assert_eq!(plan.changes()[0], (id(&repo, "base"), Some(RawValue::Str("3".into()))));
        assert_eq!(
            plan.impacts().iter().map(|i| repo.preset(i.preset()).name()).collect::<Vec<_>>(),
            ["other", "deep"]
        );
        for impact in plan.impacts() {
            assert_eq!(impact.before(), Some(&RawValue::Str("1".into())));
            assert_eq!(impact.after(), Some(&RawValue::Str("3".into())));
        }
        assert_eq!(
            fs::read_to_string(dir.path().join("Acme/process/base.json")).unwrap(),
            before,
            "planning never writes"
        );
    }

    #[test]
    fn promotion_uses_current_values_and_does_not_rewrite_an_equal_parent() {
        let (_dir, repo) = fixture();
        let mut own: BTreeMap<_, _> = repo.presets().iter().map(|p| (p.id, p.params.clone())).collect();
        for name in ["base", "a", "b"] {
            own.get_mut(&id(&repo, name)).unwrap().insert("wall_loops".into(), RawValue::Str("7".into()));
        }
        let plan =
            PromotionPlan::with_values(&repo, &[id(&repo, "a"), id(&repo, "b")], "wall_loops", |id| &own[&id]).unwrap();
        assert_eq!(plan.changes(), &[(id(&repo, "a"), None), (id(&repo, "b"), None)]);
        assert!(plan.impacts().is_empty());
        own.get_mut(&id(&repo, "base")).unwrap().insert("wall_loops".into(), RawValue::Str("8".into()));
        own.get_mut(&id(&repo, "other")).unwrap().insert("wall_loops".into(), RawValue::Str("6".into()));
        let plan =
            PromotionPlan::with_values(&repo, &[id(&repo, "a"), id(&repo, "b")], "wall_loops", |id| &own[&id]).unwrap();
        assert_eq!(plan.changes().len(), 3);
        assert!(plan.impacts().is_empty(), "unselected edited overrides shield their descendants");
    }

    #[test]
    fn promotion_rejects_roots_duplicates_non_siblings_missing_values_and_shape_differences() {
        let (_dir, repo) = fixture();
        for names in [&["a"][..], &["a", "a"][..]] {
            assert_eq!(
                PromotionPlan::new(&repo, &names.iter().map(|n| id(&repo, n)).collect::<Vec<_>>(), "wall_loops")
                    .unwrap_err(),
                PromotionError::NeedSiblings
            );
        }
        assert_eq!(
            PromotionPlan::new(&repo, &[id(&repo, "a"), id(&repo, "deep")], "wall_loops").unwrap_err(),
            PromotionError::DifferentParents
        );
        assert_eq!(
            PromotionPlan::new(&repo, &[id(&repo, "base"), id(&repo, "a")], "wall_loops").unwrap_err(),
            PromotionError::DifferentParents
        );
        assert_eq!(
            PromotionPlan::new(&repo, &[id(&repo, "a"), id(&repo, "other")], "wall_loops").unwrap_err(),
            PromotionError::MissingOwnValue
        );
        let mut own: BTreeMap<_, _> = repo.presets().iter().map(|p| (p.id, p.params.clone())).collect();
        own.get_mut(&id(&repo, "b")).unwrap().insert("wall_loops".into(), RawValue::List(vec!["3".into()]));
        assert_eq!(
            PromotionPlan::with_values(&repo, &[id(&repo, "a"), id(&repo, "b")], "wall_loops", |id| &own[&id])
                .unwrap_err(),
            PromotionError::DifferentValues
        );
        own.get_mut(&id(&repo, "a"))
            .unwrap()
            .insert("compatible_printers".into(), RawValue::List(vec!["A".into(), "B".into()]));
        own.get_mut(&id(&repo, "b"))
            .unwrap()
            .insert("compatible_printers".into(), RawValue::List(vec!["B".into(), "A".into()]));
        assert_eq!(
            PromotionPlan::with_values(&repo, &[id(&repo, "a"), id(&repo, "b")], "compatible_printers", |id| &own[&id])
                .unwrap_err(),
            PromotionError::DifferentValues
        );
    }

    #[test]
    fn redundancy_is_exact_inherited_equality_and_includes_schema_defaults_only_for_children() {
        let (dir, repo) = fixture();
        let parent = id(&repo, "base");
        let child = id(&repo, "a");
        let mut params = repo.preset(child).params.clone();
        params.insert("wall_loops".into(), RawValue::Str("1".into()));
        let issues = repo.redundant_overrides(child, &params);
        assert!(issues.iter().any(|i| matches!(i,IssueKind::RedundantOverride {key} if key=="wall_loops")));
        assert!(
            issues
                .iter()
                .all(|i| i.severity() == Severity::Info && i.key().is_some() && i.code() == "redundant-override")
        );
        for value in [RawValue::Str("1.0".into()), RawValue::List(vec!["1".into()])] {
            params.insert("wall_loops".into(), value);
            assert!(repo.redundant_overrides(child, &params).is_empty());
        }
        let defaults = &repo.schema().unwrap().default_preset(PresetKind::Process).values;
        params.insert("layer_height".into(), defaults["layer_height"].clone());
        assert!(repo.redundant_overrides(child, &params).iter().any(|i| i.key() == Some("layer_height")));
        assert!(repo.redundant_overrides(parent, &params).is_empty());
        let bare = Repository::load(dir.path()).unwrap();
        assert!(bare.redundant_overrides(child, &params).is_empty(), "no schema means no implicit defaults");
        let path = dir.path().join("Acme/process/base.json");
        let mut broken: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        broken["inherits"] = "missing".into();
        fs::write(path, broken.to_string()).unwrap();
        let broken = Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap();
        params.insert("wall_loops".into(), RawValue::Str("1".into()));
        assert!(broken.redundant_overrides(child, &params).is_empty());
        assert_eq!(
            PromotionPlan::new(&broken, &[id(&broken, "a"), id(&broken, "b")], "wall_loops").unwrap_err(),
            PromotionError::IncompleteChain
        );
    }

    #[test]
    fn cross_vendor_library_overrides_are_reported_but_cannot_be_promoted() {
        let dir = tempfile::tempdir().unwrap();
        for vendor in ["OrcaFilamentLibrary", "Acme"] {
            fs::create_dir_all(dir.path().join(format!("{vendor}/filament"))).unwrap();
        }
        fs::write(dir.path().join("OrcaFilamentLibrary.json"),r#"{"name":"OrcaFilamentLibrary","version":"1.0","filament_list":[{"name":"parent","sub_path":"filament/parent.json"}]}"#).unwrap();
        fs::write(
            dir.path().join("OrcaFilamentLibrary/filament/parent.json"),
            r#"{"type":"filament","name":"parent","instantiation":"false","from":"system","filament_type":["PLA"]}"#,
        )
        .unwrap();
        fs::write(dir.path().join("Acme.json"),r#"{"name":"Acme","version":"1.0","filament_list":[{"name":"a","sub_path":"filament/a.json"},{"name":"b","sub_path":"filament/b.json"}]}"#).unwrap();
        for name in ["a", "b"] {
            fs::write(dir.path().join(format!("Acme/filament/{name}.json")),json!({"type":"filament","name":name,"inherits":"parent","instantiation":"false","from":"system","filament_type":["PLA"]}).to_string()).unwrap();
        }
        let repo = Repository::load(dir.path()).unwrap();
        assert_eq!(repo.issues().iter().filter(|i| i.kind.code() == "redundant-override").count(), 2);
        let selected = [repo.find(PresetKind::Filament, "a")[0], repo.find(PresetKind::Filament, "b")[0]];
        assert_eq!(PromotionPlan::new(&repo, &selected, "filament_type").unwrap_err(), PromotionError::CrossVendor);
    }

    #[test]
    fn promotion_into_the_shared_library_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("OrcaFilamentLibrary/filament")).unwrap();
        let list =
            ["parent", "a", "b"].map(|name| json!({"name":name,"sub_path":format!("filament/{name}.json")})).to_vec();
        fs::write(
            dir.path().join("OrcaFilamentLibrary.json"),
            json!({"name":"OrcaFilamentLibrary","version":"1.0","filament_list":list}).to_string(),
        )
        .unwrap();
        for (name, parent) in [("parent", None), ("a", Some("parent")), ("b", Some("parent"))] {
            let mut data =
                json!({"type":"filament","name":name,"instantiation":"false","from":"system","filament_type":["PLA"]});
            if let Some(parent) = parent {
                data["inherits"] = parent.into();
                data["filament_type"] = json!(["PETG"]);
            }
            fs::write(dir.path().join(format!("OrcaFilamentLibrary/filament/{name}.json")), data.to_string()).unwrap();
        }
        let repo = Repository::load(dir.path()).unwrap();
        let selected = [repo.find(PresetKind::Filament, "a")[0], repo.find(PresetKind::Filament, "b")[0]];
        assert_eq!(PromotionPlan::new(&repo, &selected, "filament_type").unwrap_err(), PromotionError::SharedLibrary);
    }

    #[test]
    fn keys_outside_the_kind_are_not_reported_as_redundant() {
        let (dir, _) = fixture();
        // Both parent and child repeat a key that does not belong to processes.
        for name in ["base", "a"] {
            let path = dir.path().join(format!("Acme/process/{name}.json"));
            let mut data: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            data["not_an_option"] = "1".into();
            fs::write(path, data.to_string()).unwrap();
        }
        let repo = Repository::load_with_schema(dir.path(), Schema::bundled()).unwrap();
        let child = id(&repo, "a");
        let keys: Vec<_> = repo.redundant_overrides(child, &repo.preset(child).params).into_iter().collect();
        assert!(!keys.contains(&IssueKind::RedundantOverride { key: "not_an_option".into() }), "{keys:?}");
        // Without a schema there is nothing to tell the kind's keys apart, so it is compared.
        let bare = Repository::load(dir.path()).unwrap();
        let child = id(&bare, "a");
        assert!(
            bare.redundant_overrides(child, &bare.preset(child).params)
                .contains(&IssueKind::RedundantOverride { key: "not_an_option".into() })
        );
    }
}
