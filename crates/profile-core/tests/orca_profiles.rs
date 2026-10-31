//! Regression tests against a real OrcaSlicer checkout.
//!
//! Set `ORCA_PROFILES_DIR`, or keep OrcaSlicer next to this repository; otherwise the tests
//! are skipped. The expected numbers describe the profiles of OrcaSlicer commit `953509c23f`
//! and need updating when the profile data changes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use profile_core::{
    IssueKind, KindStats, ManagementPlan, Origin, PresetDraft, PresetKind, Repository, SearchIndex, Severity, Stats,
    Verdict,
};

fn profiles_dir() -> Option<PathBuf> {
    // Cargo runs integration tests from the package directory, so a relative
    // `ORCA_PROFILES_DIR` (as written in the docs) is taken relative to the workspace root.
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::var_os("ORCA_PROFILES_DIR").map(|dir| workspace.join(dir)).or_else(|| {
        let sibling = workspace.join("../OrcaSlicer/resources/profiles");
        sibling.is_dir().then_some(sibling)
    });
    if dir.is_none() {
        eprintln!("skipping: set ORCA_PROFILES_DIR to OrcaSlicer's resources/profiles");
    }
    dir
}

#[test]
fn management_preview_handles_real_profiles_without_writing_the_checkout() {
    let Some(repo) = repo() else { return };
    let source = repo
        .presets()
        .iter()
        .find(|preset| {
            preset.kind == PresetKind::Process
                && repo.vendor(preset.vendor).name == "Snapmaker"
                && repo.is_selectable(preset.id)
                && repo
                    .issues()
                    .iter()
                    .all(|issue| issue.preset != Some(preset.id) || issue.severity() < Severity::Error)
        })
        .expect("a valid Snapmaker process");
    let name = "M10 readonly preview process";
    let path = source.path.parent().unwrap().join("M10 readonly preview process.json");
    assert!(!repo.root().join(&path).exists());
    let original = std::fs::read_to_string(repo.root().join(&source.path)).unwrap();
    let plan = ManagementPlan::copy(
        repo,
        vec![PresetDraft::copy(repo, source.id, name, "M10 readonly preview process.json")],
        None,
    )
    .unwrap();
    assert!(plan.is_ready(), "{:?}", plan.notices());
    assert_eq!(plan.changes().count(), 2);
    assert!(!repo.root().join(path).exists());
    assert_eq!(std::fs::read_to_string(repo.root().join(&source.path)).unwrap(), original);
}

fn repo() -> Option<&'static Repository> {
    static REPO: OnceLock<Option<Repository>> = OnceLock::new();
    REPO.get_or_init(|| Some(Repository::load(profiles_dir()?).expect("profiles directory loads"))).as_ref()
}

fn kind_stats(files: usize, selectable: usize, templates: usize, distinct_keys: usize) -> KindStats {
    KindStats { files, selectable, templates, distinct_keys }
}

#[test]
fn matches_reference_counts() {
    let Some(repo) = repo() else { return };
    let stats = Stats::collect(repo);
    assert_eq!(stats.vendors, 57);
    assert_eq!(stats.machine_models, 326);
    assert_eq!(
        stats.kinds,
        BTreeMap::from([
            (PresetKind::Process, kind_stats(2523, 2156, 367, 333)),
            (PresetKind::Filament, kind_stats(3637, 2916, 721, 180)),
            (PresetKind::Machine, kind_stats(935, 796, 139, 162)),
        ])
    );
    assert_eq!(
        stats.depth_histogram,
        BTreeMap::from([(0, 31), (1, 585), (2, 1843), (3, 2725), (4, 623), (5, 56), (6, 5)])
    );
    assert_eq!((stats.string_values, stats.single_item_lists, stats.multi_item_lists), (113725, 74880, 8861));
    assert_eq!(stats.inherits_from_library, 65);
}

#[test]
fn nothing_would_stop_orca_from_loading() {
    let Some(repo) = repo() else { return };
    let fatal: Vec<_> = repo.issues().iter().filter(|issue| issue.severity() == Severity::Fatal).collect();
    assert!(fatal.is_empty(), "fatal issues: {fatal:#?}");
}

#[test]
fn reports_known_data_problems() {
    let Some(repo) = repo() else { return };
    let stats = Stats::collect(repo);
    let count = |code| stats.issues_by_code.get(code).copied().unwrap_or(0);
    assert_eq!(count("duplicate-across-vendors"), 10);
    assert_eq!(count("dangling-compatible-printer"), 581);
    assert_eq!(count("orphan-file"), 457);
    assert_eq!(count("utf8-bom"), 2);
    assert_eq!(count("list-name-mismatch"), 6);
    assert_eq!(count("unexpected-from"), 97);
    // Independently matched by profile_format_stats.py.
    assert_eq!(count("redundant-override"), 56926);

    let forgotten = Path::new("Snapmaker/process/0.24 Standard @Snapmaker (0.8 nozzle).json");
    assert!(
        repo.issues().iter().any(|issue| issue.kind == IssueKind::OrphanFile && issue.path == forgotten),
        "the unlisted Snapmaker process profile is reported"
    );
}

#[test]
fn resolves_snapmaker_filament_chain() {
    let Some(repo) = repo() else { return };
    let &[id] = repo.find(PresetKind::Filament, "Generic ABS @U1 0.2 nozzle") else {
        panic!("exactly one preset with this name");
    };
    let chain: Vec<&str> = repo.chain(id).into_iter().map(|link| repo.preset(link).name()).collect();
    assert_eq!(
        chain,
        ["Generic ABS @U1 0.2 nozzle", "Generic ABS @U1 base", "fdm_filament_abs_category", "fdm_filament_common"]
    );
    assert_eq!(repo.alias(id), "Generic ABS");
    // No own filament_id: OrcaSlicer falls back to setting_id.
    assert_eq!(repo.filament_id(id), Some("GFSB99"));
    assert_eq!(repo.lookup_scalar(id, "nozzle_temperature"), Some("270"));
}

#[test]
fn compares_snapmaker_abs_nozzle_variants() {
    let Some(repo) = repo() else { return };
    let left = repo.find(PresetKind::Filament, "Generic ABS @U1 0.2 nozzle")[0];
    // The 0.4 nozzle variant is named "Generic ABS", without a nozzle suffix.
    let right = repo
        .find(PresetKind::Filament, "Generic ABS")
        .iter()
        .copied()
        .find(|&id| repo.vendor(repo.preset(id).vendor).name == "Snapmaker")
        .unwrap();
    let rows = repo.diff(left, right).unwrap();
    let changed: Vec<_> = rows.iter().filter(|row| row.is_different()).collect();
    assert_eq!(changed.iter().map(|row| row.key).collect::<Vec<_>>(), ["compatible_printers"]);
    assert_eq!(changed[0].left.as_ref().unwrap().effective().value.as_scalar(), Some("Snapmaker U1 (0.2 nozzle)"));
    assert_eq!(changed[0].right.as_ref().unwrap().effective().value.as_scalar(), Some("Snapmaker U1 (0.4 nozzle)"));
    let temperature = rows.iter().find(|row| row.key == "nozzle_temperature").unwrap();
    assert_eq!(temperature.left.as_ref().unwrap().effective().value.as_scalar(), Some("270"));
    assert_eq!(temperature.right.as_ref().unwrap().effective().value.as_scalar(), Some("270"));
    assert_eq!(temperature.left.as_ref().unwrap().effective().origin, Origin::Preset(left));
    assert_eq!(temperature.right.as_ref().unwrap().effective().origin, Origin::Preset(right));
}

#[test]
fn search_finds_presets_options_and_values() {
    let Some(repo) = repo() else { return };
    let index = SearchIndex::build(repo);
    let abs = repo.find(PresetKind::Filament, "Generic ABS @U1 0.2 nozzle")[0];

    let results = index.search("generic abs @u1 0.2", 5);
    assert_eq!(results.presets.first(), Some(&abs));
    let results = index.search("wall_loops", 5);
    assert_eq!(results.options.first().map(|hit| hit.key.as_str()), Some("wall_loops"));
    // Where is 270 set as a nozzle temperature?
    let results = index.search("nozzle_temperature 270", usize::MAX);
    assert!(results.values.iter().any(|hit| hit.preset == abs && hit.key == "nozzle_temperature"));
    assert_eq!(results.total_values, results.values.len());
}

/// A baseline of the compatibility rules on real data, with condition variables typed by their
/// text (the profiles carry no option types).
#[test]
fn compatibility_follows_conditions_lists_and_library_exclusion() {
    let Some(repo) = repo() else { return };
    let index = repo.compatibility();
    let one = |kind, name| repo.find(kind, name)[0];
    let names = |preset| {
        index.compatible_printers(repo, preset).into_iter().map(|(id, _)| repo.preset(id).name()).collect::<Vec<_>>()
    };

    // All 97 conditions parse and evaluate on every printer.
    let condition_issues = repo.issues().iter().filter(|issue| issue.kind.code().starts_with("condition")).count();
    assert_eq!(condition_issues, 0);
    let detail = one(PresetKind::Process, "0.07mm DETAIL @CORE One 0.25");
    assert_eq!(names(detail), ["Prusa CORE One 0.25 nozzle"]);

    let u1 = one(PresetKind::Machine, "Snapmaker U1 (0.4 nozzle)");
    let library_abs = one(PresetKind::Filament, "Generic ABS @System");
    assert_eq!(index.verdict(repo, library_abs, u1), Verdict::Excluded, "Snapmaker ships its own Generic ABS");
    assert_eq!(names(library_abs).len(), 728);
    assert_eq!(index.compatible_with_printer(repo, u1, PresetKind::Process).len(), 16);
    assert_eq!(index.compatible_with_printer(repo, u1, PresetKind::Filament).len(), 169);
}

/// Name references and descendants (M19) on real data.
#[test]
fn references_follow_inheritance_and_name_lists() {
    let Some(repo) = repo() else { return };
    let one = |kind, name| repo.find(kind, name)[0];
    let u1 = repo.references(one(PresetKind::Machine, "Snapmaker U1 (0.4 nozzle)"));
    assert_eq!((u1.children.len(), u1.descendants, u1.referrers.len()), (0, 0, 51));
    assert!(u1.referrers.iter().all(|referrer| referrer.is_own()));

    let pla = repo.references(one(PresetKind::Filament, "Snapmaker PLA"));
    assert_eq!(pla.referrers.len(), 93);
    assert_eq!(pla.referrers.iter().filter(|referrer| !referrer.is_own()).count(), 90);

    // Every vendor has its own fdm_filament_pla template; the library's is inherited across vendors.
    let base = repo.find(PresetKind::Filament, "fdm_filament_pla");
    let library = base.iter().find(|&&id| repo.vendor(repo.preset(id).vendor).name == "OrcaFilamentLibrary");
    let library = repo.references(*library.unwrap());
    assert_eq!((library.children.len(), library.descendants), (74, 159));
    assert!(library.referrers.is_empty() && !library.ambiguous);

    let model = repo.models().iter().position(|model| model.id == "Snapmaker U1").unwrap();
    assert_eq!(repo.model_references(model).len(), 4);
}
