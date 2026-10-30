use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use profile_core::{Issue, Origin, PresetId, PresetKind, RawValue, Repository, SearchIndex, Severity, Stats, Verdict};

#[derive(Parser)]
#[command(name = "profile-tool", version, about = "Inspect and validate OrcaSlicer system profiles")]
struct Cli {
    /// OrcaSlicer's `resources/profiles` directory, or an OrcaSlicer checkout, installation or app
    /// bundle that contains it
    #[arg(long, short, env = "ORCA_PROFILES_DIR", global = true)]
    profiles: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print counts that summarize the profile set
    Stats,
    /// List validation issues, grouped by issue code
    Check {
        /// Hide issues below this severity
        #[arg(long, value_enum, default_value_t = SeverityArg::Warning)]
        min: SeverityArg,
        /// Exit with status 1 if a shown issue is at or above this severity
        #[arg(long, value_enum, default_value_t = SeverityArg::Error)]
        fail_on: SeverityArg,
        /// Only show issues of this vendor
        #[arg(long)]
        vendor: Option<String>,
        /// Issues printed per code; 0 prints all
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Show a preset's inheritance chain and effective values
    Resolve {
        /// Exact preset name, e.g. "Generic ABS @U1 0.2 nozzle"
        name: String,
        #[arg(long, value_enum)]
        kind: Option<KindArg>,
        /// Only show keys containing this text
        #[arg(long)]
        key: Option<String>,
    },
    /// List the printers a filament or process is compatible with, or the presets a printer accepts
    Compat {
        /// Exact preset name
        name: String,
        #[arg(long, value_enum)]
        kind: Option<KindArg>,
    },
    /// Compare compatibility with a table exported by OrcaSlicer's Snapmaker_Orca_compat_export
    CompatCheck {
        /// The exported JSON table
        table: PathBuf,
        /// Differences printed per kind; 0 prints all
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Search presets, options and the values preset files set; every term must match
    Search {
        query: String,
        /// Hits printed per section
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum SeverityArg {
    Info,
    Warning,
    Error,
    Fatal,
}

impl From<SeverityArg> for Severity {
    fn from(arg: SeverityArg) -> Self {
        match arg {
            SeverityArg::Info => Severity::Info,
            SeverityArg::Warning => Severity::Warning,
            SeverityArg::Error => Severity::Error,
            SeverityArg::Fatal => Severity::Fatal,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum KindArg {
    Process,
    Filament,
    Machine,
}

impl From<KindArg> for PresetKind {
    fn from(arg: KindArg) -> Self {
        match arg {
            KindArg::Process => PresetKind::Process,
            KindArg::Filament => PresetKind::Filament,
            KindArg::Machine => PresetKind::Machine,
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let Some(dir) = cli.profiles else {
        bail!("pass --profiles <DIR> or set ORCA_PROFILES_DIR");
    };
    let Some(dir) = profile_core::find_profiles_dir(&dir) else {
        bail!("{} is not a profiles directory: it has no `<Vendor>.json` beside a `<Vendor>` folder", dir.display());
    };
    let started = Instant::now();
    let repo = Repository::load(&dir).with_context(|| format!("loading {}", dir.display()))?;
    let elapsed = started.elapsed();
    match cli.command {
        Command::Stats => print_stats(&repo, elapsed),
        Command::Check { min, fail_on, vendor, limit } => {
            return Ok(check(&repo, min.into(), fail_on.into(), vendor.as_deref(), limit));
        }
        Command::Resolve { name, kind, key } => resolve(&repo, &name, kind.map(Into::into), key.as_deref())?,
        Command::Search { query, limit } => search(&repo, &query, limit),
        Command::Compat { name, kind } => compat(&repo, &name, kind.map(Into::into))?,
        Command::CompatCheck { table, limit } => return compat_check(&repo, &table, limit),
    }
    Ok(ExitCode::SUCCESS)
}

fn print_stats(repo: &Repository, elapsed: Duration) {
    let stats = Stats::collect(repo);
    println!("loaded {} in {:.0?}", repo.root().display(), elapsed);
    println!("vendors: {}   machine models: {}", stats.vendors, stats.machine_models);
    println!();
    println!("{:<10}{:>8}{:>12}{:>11}{:>8}", "kind", "files", "selectable", "templates", "keys");
    for (kind, kind_stats) in &stats.kinds {
        println!(
            "{:<10}{:>8}{:>12}{:>11}{:>8}",
            kind.as_str(),
            kind_stats.files,
            kind_stats.selectable,
            kind_stats.templates,
            kind_stats.distinct_keys
        );
    }
    println!();
    let depths: Vec<String> = stats.depth_histogram.iter().map(|(depth, count)| format!("{depth}:{count}")).collect();
    println!("inheritance depth of selectable presets: {}", depths.join("  "));
    println!(
        "values: {} strings, {} one-item arrays, {} longer arrays",
        stats.string_values, stats.single_item_lists, stats.multi_item_lists
    );
    println!("vendor presets inheriting from the filament library: {}", stats.inherits_from_library);
    println!("compatible_printers entries: {}", stats.compatible_printer_refs);
    println!();
    let severities: Vec<String> =
        stats.issues_by_severity.iter().rev().map(|(severity, count)| format!("{severity} {count}")).collect();
    println!("issues: {}", if severities.is_empty() { "none".to_owned() } else { severities.join(", ") });
    for (code, count) in &stats.issues_by_code {
        println!("  {code:<30}{count:>6}");
    }
}

fn check(repo: &Repository, min: Severity, fail_on: Severity, vendor: Option<&str>, limit: usize) -> ExitCode {
    let shown: Vec<&Issue> = repo
        .issues()
        .iter()
        .filter(|issue| issue.severity() >= min)
        .filter(|issue| vendor.is_none_or(|vendor| repo.vendor(issue.vendor).name == vendor))
        .collect();

    let mut groups: BTreeMap<(Reverse<Severity>, &str), Vec<&Issue>> = BTreeMap::new();
    for &issue in &shown {
        groups.entry((Reverse(issue.severity()), issue.kind.code())).or_default().push(issue);
    }
    for ((Reverse(severity), code), issues) in &groups {
        println!("{severity} {code} ({})", issues.len());
        let printed = if limit == 0 { issues.len() } else { limit.min(issues.len()) };
        for issue in &issues[..printed] {
            println!("  {}: {}", issue.path.display(), issue.kind);
        }
        if printed < issues.len() {
            println!("  ... {} more", issues.len() - printed);
        }
        println!();
    }

    let failing = shown.iter().filter(|issue| issue.severity() >= fail_on).count();
    println!("{} issue(s) shown, {failing} at or above {fail_on}", shown.len());
    if failing > 0 { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

fn compat(repo: &Repository, name: &str, kind: Option<PresetKind>) -> Result<()> {
    let kinds = kind.map_or(PresetKind::ALL.to_vec(), |kind| vec![kind]);
    let Some(&id) = kinds.iter().flat_map(|&kind| repo.find(kind, name)).next() else {
        bail!("no preset named {name:?}");
    };
    let index = repo.compatibility();
    let preset = repo.preset(id);
    println!("{} ({} · {})", preset.name(), preset.kind, repo.vendor(preset.vendor).name);
    if preset.kind == PresetKind::Machine {
        for kind in [PresetKind::Process, PresetKind::Filament] {
            let presets = index.compatible_with_printer(repo, id, kind);
            println!("\ncompatible {kind} presets ({}):", presets.len());
            for preset in presets {
                println!("  {}", repo.preset(preset).name());
            }
        }
        return Ok(());
    }
    if let Some(condition) = index.condition(id, PresetKind::Machine) {
        match condition {
            Ok(condition) => println!("condition: {condition}"),
            Err(err) => println!("condition does not parse: {err}"),
        }
    }
    let printers = index.compatible_printers(repo, id);
    println!("\ncompatible printers ({}):", printers.len());
    for (printer, verdict) in printers {
        let reason = match verdict {
            Verdict::Listed => "listed".to_owned(),
            Verdict::Unrestricted => "no restriction".to_owned(),
            Verdict::Condition(Ok(_)) => "condition".to_owned(),
            Verdict::Condition(Err(err)) => format!("condition failed: {err}"),
            Verdict::Excluded | Verdict::NotListed => unreachable!("only compatible printers are listed"),
        };
        println!("  {:<60} {reason}", repo.preset(printer).name());
    }
    Ok(())
}

/// The table `Snapmaker_Orca_compat_export` writes: printer names, and for each filament and
/// process preset the indices of the printers OrcaSlicer finds it compatible with.
#[derive(serde::Deserialize)]
struct CompatTable {
    source_commit: String,
    printers: Vec<String>,
    filament: BTreeMap<String, Vec<usize>>,
    process: BTreeMap<String, Vec<usize>>,
}

fn compat_check(repo: &Repository, path: &std::path::Path, limit: usize) -> Result<ExitCode> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let table: CompatTable = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let index = repo.compatibility();
    let selectable = |kind: PresetKind| {
        repo.presets().iter().filter(move |preset| preset.kind == kind && repo.is_selectable(preset.id))
    };
    println!("OrcaSlicer table from {}", if table.source_commit.is_empty() { "?" } else { &table.source_commit });

    let ours: BTreeSet<&str> = selectable(PresetKind::Machine).map(|preset| preset.name()).collect();
    let theirs: BTreeSet<&str> = table.printers.iter().map(String::as_str).collect();
    let mut differences = 0;
    for (label, names) in [("only here", ours.difference(&theirs)), ("only in OrcaSlicer", theirs.difference(&ours))] {
        let names: Vec<_> = names.collect();
        if !names.is_empty() {
            differences += names.len();
            println!(
                "printers {label} ({}): {}",
                names.len(),
                names.iter().map(|name| name.to_string()).collect::<Vec<_>>().join(", ")
            );
        }
    }

    for (kind, presets) in [(PresetKind::Filament, &table.filament), (PresetKind::Process, &table.process)] {
        let mut mismatches = Vec::new();
        let mut missing = Vec::new();
        for (name, indices) in presets {
            let expected: BTreeSet<&str> = indices.iter().map(|&ix| table.printers[ix].as_str()).collect();
            // A name two vendors share is loaded once by OrcaSlicer; accept either preset.
            let candidates: Vec<_> =
                repo.find(kind, name).iter().copied().filter(|&id| repo.is_selectable(id)).collect();
            if candidates.is_empty() {
                missing.push(name.as_str());
                continue;
            }
            let actual = |id| -> BTreeSet<&str> {
                index
                    .compatible_printers(repo, id)
                    .into_iter()
                    .map(|(printer, _)| repo.preset(printer).name())
                    .filter(|name| theirs.contains(name))
                    .collect()
            };
            let sets: Vec<_> = candidates.iter().map(|&id| actual(id)).collect();
            if !sets.contains(&expected) {
                let set = &sets[0];
                let extra: Vec<_> = set.difference(&expected).copied().collect();
                let absent: Vec<_> = expected.difference(set).copied().collect();
                mismatches.push((name.as_str(), extra, absent));
            }
        }
        let extra_here: Vec<_> =
            selectable(kind).map(|preset| preset.name()).filter(|name| !presets.contains_key(*name)).collect();
        println!(
            "\n{kind}: {} compared, {} differ, {} missing here, {} only here",
            presets.len(),
            mismatches.len(),
            missing.len(),
            extra_here.len()
        );
        let shown = if limit == 0 { usize::MAX } else { limit };
        for (name, extra, absent) in mismatches.iter().take(shown) {
            println!("  {name}");
            if !extra.is_empty() {
                println!("    compatible here only ({}): {}", extra.len(), extra.join(", "));
            }
            if !absent.is_empty() {
                println!("    compatible in OrcaSlicer only ({}): {}", absent.len(), absent.join(", "));
            }
        }
        for name in missing.iter().take(shown) {
            println!("  missing here: {name}");
        }
        for name in extra_here.iter().take(shown) {
            println!("  only here: {name}");
        }
        differences += mismatches.len() + missing.len() + extra_here.len();
    }
    println!("\n{differences} difference(s)");
    Ok(if differences == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

fn search(repo: &Repository, query: &str, limit: usize) {
    let started = Instant::now();
    let index = SearchIndex::build(repo);
    let built = started.elapsed();
    let started = Instant::now();
    let results = index.search(query, limit);
    println!("index built in {built:.0?}, searched in {:.0?}", started.elapsed());
    let describe = |id: PresetId| {
        let preset = repo.preset(id);
        format!("{} / {} ({})", repo.vendor(preset.vendor).name, preset.name(), preset.kind)
    };
    println!("\npresets ({}):", results.total_presets);
    for &id in &results.presets {
        println!("  {}", describe(id));
    }
    println!("\noptions ({}):", results.total_options);
    for hit in &results.options {
        println!("  {}", hit.key);
    }
    println!("\nvalues ({}):", results.total_values);
    for hit in &results.values {
        println!("  {}  {} = {}", describe(hit.preset), hit.key, hit.snippet);
    }
}

fn resolve(repo: &Repository, name: &str, kind: Option<PresetKind>, key_filter: Option<&str>) -> Result<()> {
    let kinds = kind.map_or(PresetKind::ALL.to_vec(), |kind| vec![kind]);
    let matches: Vec<PresetId> = kinds.iter().flat_map(|&kind| repo.find(kind, name).iter().copied()).collect();
    if matches.is_empty() {
        let words: Vec<String> = name.split_whitespace().map(str::to_lowercase).collect();
        let similar: Vec<&str> = repo
            .presets()
            .iter()
            .filter(|preset| kinds.contains(&preset.kind) && repo.is_selectable(preset.id))
            .filter(|preset| {
                let candidate = preset.name().to_lowercase();
                words.iter().all(|word| candidate.contains(word))
            })
            .map(|preset| preset.name())
            .take(10)
            .collect();
        if similar.is_empty() {
            bail!("no preset named {name:?}");
        }
        bail!("no preset named {name:?}; similar names:\n  {}", similar.join("\n  "));
    }
    for (position, &id) in matches.iter().enumerate() {
        if position > 0 {
            println!("\n{}\n", "-".repeat(72));
        }
        print_preset(repo, id, key_filter);
    }
    Ok(())
}

fn print_preset(repo: &Repository, id: PresetId, key_filter: Option<&str>) {
    let preset = repo.preset(id);
    let selectable = if repo.is_selectable(id) { "selectable" } else { "template" };
    println!("{}", preset.name());
    println!("  {} · {} · {selectable}", preset.kind, repo.vendor(preset.vendor).name);
    println!("  file: {}", preset.path.display());
    print!("  alias: {}", repo.alias(id));
    if preset.kind == PresetKind::Filament {
        print!("   filament_id: {}", repo.filament_id(id).unwrap_or("-"));
    }
    println!();

    let chain = repo.chain(id);
    println!("\ninheritance (nearest first):");
    for (depth, &link) in chain.iter().enumerate() {
        let ancestor = repo.preset(link);
        println!("  [{depth}] {}  ({})", ancestor.name(), ancestor.path.display());
    }
    let root = repo.preset(*chain.last().expect("chain contains the preset itself"));
    if root.inherits_unresolved
        && let Some(parent) = root.meta.inherits()
    {
        println!("  [?] {parent}  (not found)");
    }

    let values = repo.resolve(id);
    let shown: Vec<_> = values.iter().filter(|(key, _)| key_filter.is_none_or(|filter| key.contains(filter))).collect();
    let width = shown.iter().map(|(key, _)| key.len()).max().unwrap_or(0).min(40);
    println!("\nvalues ({} of {} keys; [n] = set by chain entry n):", shown.len(), values.len());
    for (key, resolved) in shown {
        let Origin::Preset(origin) = resolved.origin;
        let layer = chain.iter().position(|&link| link == origin).unwrap_or(0);
        println!("  {key:<width$}  [{layer}] {}", format_value(resolved.value));
    }
}

fn format_value(value: &RawValue) -> String {
    const MAX: usize = 80;
    let text = match value {
        RawValue::Str(text) => format!("{text:?}"),
        RawValue::List(items) => {
            format!("[{}]", items.iter().map(|item| format!("{item:?}")).collect::<Vec<_>>().join(", "))
        }
    };
    let length = text.chars().count();
    if length <= MAX {
        return text;
    }
    let cut: String = text.chars().take(MAX).collect();
    format!("{cut}… ({length} chars)")
}
