use std::collections::{BTreeMap, HashSet};

use crate::issue::Severity;
use crate::model::{PresetKind, RawValue};
use crate::repository::{Repository, ResolvedValue};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KindStats {
    pub files: usize,
    pub selectable: usize,
    pub templates: usize,
    /// Distinct config keys used by files of this kind.
    pub distinct_keys: usize,
}

/// Counts that summarize a loaded profile set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub vendors: usize,
    pub machine_models: usize,
    pub kinds: BTreeMap<PresetKind, KindStats>,
    /// Number of ancestors of each selectable preset whose chain fully resolves.
    pub depth_histogram: BTreeMap<usize, usize>,
    pub string_values: usize,
    pub single_item_lists: usize,
    pub multi_item_lists: usize,
    pub inherits_from_library: usize,
    /// Non-empty `compatible_printers` entries of selectable presets, after inheritance.
    pub compatible_printer_refs: usize,
    pub issues_by_severity: BTreeMap<Severity, usize>,
    pub issues_by_code: BTreeMap<&'static str, usize>,
}

impl Stats {
    pub fn collect(repo: &Repository) -> Self {
        let mut stats =
            Stats { vendors: repo.vendors().len(), machine_models: repo.models().len(), ..Default::default() };
        let mut keys: BTreeMap<PresetKind, HashSet<&str>> = BTreeMap::new();
        for preset in repo.presets() {
            let kind = stats.kinds.entry(preset.kind).or_default();
            kind.files += 1;
            if repo.is_selectable(preset.id) {
                kind.selectable += 1;
            } else {
                kind.templates += 1;
            }
            keys.entry(preset.kind).or_default().extend(preset.params.keys().map(String::as_str));
            for value in preset.params.values() {
                match value {
                    RawValue::Str(_) => stats.string_values += 1,
                    RawValue::List(items) if items.len() == 1 => stats.single_item_lists += 1,
                    RawValue::List(_) => stats.multi_item_lists += 1,
                }
            }
            if preset.inherits_from_library {
                stats.inherits_from_library += 1;
            }
            if !repo.is_selectable(preset.id) {
                continue;
            }
            if let Some(depth) = repo.depth(preset.id) {
                *stats.depth_histogram.entry(depth).or_default() += 1;
            }
            if let Some(ResolvedValue { value: RawValue::List(printers), .. }) =
                repo.lookup(preset.id, "compatible_printers")
            {
                stats.compatible_printer_refs += printers.iter().filter(|printer| !printer.is_empty()).count();
            }
        }
        for (kind, keys) in keys {
            stats.kinds.entry(kind).or_default().distinct_keys = keys.len();
        }
        for issue in repo.issues() {
            *stats.issues_by_severity.entry(issue.severity()).or_default() += 1;
            *stats.issues_by_code.entry(issue.kind.code()).or_default() += 1;
        }
        stats
    }
}
