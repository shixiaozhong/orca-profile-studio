//! Case-insensitive search over presets, options and the values preset files set.
//!
//! The profile data is small (about 250 000 values, 7.5 MB of text), so a linear scan over
//! lower-cased copies is fast enough; no inverted index is needed. Values are the ones the files
//! set, as loaded: inherited and default values are not repeated for every preset.

use crate::model::{PresetId, RawValue};
use crate::repository::Repository;

struct PresetEntry {
    id: PresetId,
    /// Lower-cased name.
    name: String,
    /// Lower-cased name, alias, `setting_id` and `filament_id`.
    haystack: String,
    selectable: bool,
}

struct OptionEntry {
    key: String,
    /// Lower-cased key.
    haystack: String,
}

struct ValueEntry {
    preset: PresetId,
    key: String,
    /// The value on one line, as written.
    text: String,
    /// Lower-cased `key = value`, so a query can name both.
    haystack: String,
}

/// A prebuilt index over a loaded [`Repository`].
pub struct SearchIndex {
    presets: Vec<PresetEntry>,
    options: Vec<OptionEntry>,
    values: Vec<ValueEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionHit {
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueHit {
    pub preset: PresetId,
    pub key: String,
    /// The value as one line, shortened around the first match.
    pub snippet: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResults {
    pub presets: Vec<PresetId>,
    pub options: Vec<OptionHit>,
    pub values: Vec<ValueHit>,
    /// Matches before `limit` cut the lists.
    pub total_presets: usize,
    pub total_options: usize,
    pub total_values: usize,
}

impl SearchIndex {
    pub fn build(repo: &Repository) -> Self {
        let presets = repo
            .presets()
            .iter()
            .map(|preset| {
                let mut haystack = vec![preset.name().to_owned(), repo.alias(preset.id).to_owned()];
                haystack.extend(preset.meta.setting_id.clone());
                haystack.extend(repo.filament_id(preset.id).map(str::to_owned));
                PresetEntry {
                    id: preset.id,
                    name: preset.name().to_lowercase(),
                    haystack: haystack.join("\n").to_lowercase(),
                    selectable: repo.is_selectable(preset.id),
                }
            })
            .collect();
        // Options are the keys the profiles set.
        let options = repo
            .presets()
            .iter()
            .flat_map(|preset| preset.params.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|key| OptionEntry { key: key.clone(), haystack: key.to_lowercase() })
            .collect();
        let values = repo
            .presets()
            .iter()
            .flat_map(|preset| {
                preset.params.iter().map(|(key, value)| {
                    let text = joined(value).replace('\n', " ⏎ ");
                    let haystack = format!("{key} = {text}").to_lowercase();
                    ValueEntry { preset: preset.id, key: key.clone(), text, haystack }
                })
            })
            .collect();
        SearchIndex { presets, options, values }
    }

    /// Every whitespace-separated term of `query` must occur. Each list keeps at most `limit`
    /// hits; an empty query matches nothing.
    pub fn search(&self, query: &str, limit: usize) -> SearchResults {
        let query = query.to_lowercase();
        let terms: Vec<&str> = query.split_whitespace().collect();
        if terms.is_empty() {
            return SearchResults::default();
        }
        let matches = |haystack: &str| terms.iter().all(|term| haystack.contains(term));

        let mut presets: Vec<&PresetEntry> = self.presets.iter().filter(|entry| matches(&entry.haystack)).collect();
        // Names starting with the query first, then selectable presets, then shorter names.
        presets.sort_by_key(|entry| (!entry.name.starts_with(&query), !entry.selectable, entry.name.len(), entry.id));
        let mut options: Vec<&OptionEntry> = self.options.iter().filter(|entry| matches(&entry.haystack)).collect();
        options.sort_by_key(|entry| (!entry.key.starts_with(&query), entry.key.len(), entry.key.as_str()));
        let values: Vec<&ValueEntry> = self.values.iter().filter(|entry| matches(&entry.haystack)).collect();

        SearchResults {
            total_presets: presets.len(),
            total_options: options.len(),
            total_values: values.len(),
            presets: presets.into_iter().take(limit).map(|entry| entry.id).collect(),
            options: options.into_iter().take(limit).map(|entry| OptionHit { key: entry.key.clone() }).collect(),
            values: values
                .into_iter()
                .take(limit)
                .map(|entry| ValueHit {
                    preset: entry.preset,
                    key: entry.key.clone(),
                    snippet: snippet(&entry.text, &entry.haystack[entry.key.len() + 3..], &terms),
                })
                .collect(),
        }
    }
}

fn joined(value: &RawValue) -> String {
    match value {
        RawValue::Str(text) => text.clone(),
        RawValue::List(items) => items.join(", "),
    }
}

/// At most about 80 characters of `text` around the first term found in its lower-cased copy.
fn snippet(text: &str, lower: &str, terms: &[&str]) -> String {
    const WIDTH: usize = 80;
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= WIDTH {
        return text.to_owned();
    }
    // Lower-casing keeps the character count for everything but a few rare letters.
    let at = terms.iter().find_map(|term| lower.find(term)).map_or(0, |byte| lower[..byte].chars().count());
    let start = at.saturating_sub(WIDTH / 4).min(chars.len() - WIDTH);
    let mut out: String = chars[start..start + WIDTH].iter().collect();
    if start > 0 {
        out.insert(0, '…');
    }
    if start + WIDTH < chars.len() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{SearchIndex, snippet};
    use crate::model::PresetKind;
    use crate::repository::Repository;

    fn fixture() -> (tempfile::TempDir, Repository) {
        let dir = tempfile::tempdir().unwrap();
        let write = |path: &str, text: &str| {
            let path = dir.path().join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write(
            "Acme.json",
            r#"{ "name": "Acme", "version": "1.0", "filament_list": [
                { "name": "acme_base", "sub_path": "filament/base.json" },
                { "name": "Acme PLA", "sub_path": "filament/pla.json" },
                { "name": "Super Acme PLA", "sub_path": "filament/super.json" } ] }"#,
        );
        write(
            "Acme/filament/base.json",
            r#"{ "type": "filament", "name": "acme_base", "instantiation": "false", "filament_id": "AC01" }"#,
        );
        write(
            "Acme/filament/pla.json",
            r#"{ "type": "filament", "name": "Acme PLA", "inherits": "acme_base", "instantiation": "true",
                "nozzle_temperature": ["210"], "filament_start_gcode": ["; start\nM104 S210"] }"#,
        );
        write(
            "Acme/filament/super.json",
            r#"{ "type": "filament", "name": "Super Acme PLA", "inherits": "acme_base", "instantiation": "true",
                "setting_id": "GFSX01", "nozzle_temperature": ["230"] }"#,
        );
        let repo = Repository::load(dir.path()).unwrap();
        (dir, repo)
    }

    #[test]
    fn options_are_the_keys_the_profiles_set() {
        let (_dir, repo) = fixture();
        let index = SearchIndex::build(&repo);
        let result = index.search("nozzle_temperature", 10);
        assert_eq!(result.options.len(), 1);
        assert_eq!(result.options[0].key, "nozzle_temperature");
        assert_eq!(result.values.len(), 2);
        assert!(index.search("Wall loops", 10).options.is_empty());
    }

    #[test]
    fn finds_presets_by_name_alias_and_ids() {
        let (_dir, repo) = fixture();
        let index = SearchIndex::build(&repo);
        let names = |query: &str| {
            index.search(query, 10).presets.into_iter().map(|id| repo.preset(id).name().to_owned()).collect::<Vec<_>>()
        };
        // A name starting with the query ranks first; templates rank after selectable presets.
        assert_eq!(names("acme"), ["Acme PLA", "acme_base", "Super Acme PLA"]);
        assert_eq!(names("pla super"), ["Super Acme PLA"]);
        assert_eq!(names("gfsx01"), ["Super Acme PLA"]);
        // filament_id is inherited from the template, unless an own setting_id takes precedence.
        assert_eq!(names("ac01"), ["Acme PLA", "acme_base"]);
        assert!(names("   ").is_empty());
    }

    #[test]
    fn finds_options_and_values_with_their_keys() {
        let (_dir, repo) = fixture();
        let index = SearchIndex::build(&repo);
        let results = index.search("nozzle_temperature 230", 10);
        assert_eq!(results.values.len(), 1);
        assert_eq!(repo.preset(results.values[0].preset).name(), "Super Acme PLA");
        assert_eq!(results.values[0].snippet, "230");
        assert!(results.options.is_empty(), "no option matches both terms");

        let results = index.search("nozzle_temp", 1);
        assert_eq!(results.options[0].key, "nozzle_temperature");
        assert!(results.total_options >= 1);
        assert_eq!(results.values.len(), 1);
        assert_eq!(results.total_values, 2);

        let gcode = index.search("m104", 10);
        assert_eq!(gcode.values[0].snippet, "; start ⏎ M104 S210");
        let filament = repo.find(PresetKind::Filament, "Acme PLA")[0];
        assert_eq!(gcode.values[0].preset, filament);
    }

    #[test]
    fn snippets_center_on_the_match() {
        let long = format!("{}Needle{}", "a".repeat(100), "b".repeat(100));
        let cut = snippet(&long, &long.to_lowercase(), &["needle"]);
        assert!(cut.starts_with('…') && cut.ends_with('…'));
        assert!(cut.contains("Needle"));
        assert_eq!(cut.chars().count(), 82);
    }
}
