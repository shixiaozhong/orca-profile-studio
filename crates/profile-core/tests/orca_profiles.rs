//! Regression tests against a real OrcaSlicer checkout.
//!
//! Set `ORCA_PROFILES_DIR`, or keep OrcaSlicer next to this repository; otherwise the tests
//! are skipped. The expected numbers describe the profiles of OrcaSlicer commit `953509c23f`
//! and need updating when the profile data changes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use profile_core::{IssueKind, KindStats, PresetKind, Repository, Severity, Stats};

fn repo() -> Option<&'static Repository> {
    static REPO: OnceLock<Option<Repository>> = OnceLock::new();
    REPO.get_or_init(|| {
        let dir = std::env::var_os("ORCA_PROFILES_DIR").map(PathBuf::from).or_else(|| {
            let sibling = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../OrcaSlicer/resources/profiles");
            sibling.is_dir().then_some(sibling)
        });
        match dir {
            Some(dir) => Some(Repository::load(&dir).expect("profiles directory loads")),
            None => {
                eprintln!("skipping: set ORCA_PROFILES_DIR to OrcaSlicer's resources/profiles");
                None
            }
        }
    })
    .as_ref()
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
