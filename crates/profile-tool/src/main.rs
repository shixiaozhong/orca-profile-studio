use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use profile_core::{
    Issue, Origin, PresetId, PresetKind, RawValue, Repository, Schema, SearchIndex, Severity, Stats, Verdict,
};

#[derive(Parser)]
#[command(name = "profile-tool", version, about = "Inspect and validate OrcaSlicer system profiles")]
struct Cli {
    /// OrcaSlicer's `resources/profiles` directory
    #[arg(long, short, env = "ORCA_PROFILES_DIR", global = true)]
    profiles: Option<PathBuf>,
    /// Option schema exported from OrcaSlicer; defaults to the bundled one
    #[arg(long, global = true, conflicts_with = "no_schema")]
    schema: Option<PathBuf>,
    /// Load without a schema: skip value checks and built-in defaults
    #[arg(long, global = true)]
    no_schema: bool,
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
        /// Also show keys no file in the chain sets (schema defaults)
        #[arg(long)]
        defaults: bool,
    },
    /// List the printers a filament or process is compatible with, or the presets a printer accepts
    Compat {
        /// Exact preset name
        name: String,
        #[arg(long, value_enum)]
        kind: Option<KindArg>,
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
    let schema = match (&cli.schema, cli.no_schema) {
        (_, true) => None,
        (Some(path), false) => {
            Some(Arc::new(Schema::load(path).with_context(|| format!("loading schema {}", path.display()))?))
        }
        (None, false) => Some(Schema::bundled()),
    };
    let started = Instant::now();
    let repo = match schema {
        Some(schema) => Repository::load_with_schema(&dir, schema),
        None => Repository::load(&dir),
    }
    .with_context(|| format!("loading {}", dir.display()))?;
    let elapsed = started.elapsed();
    match cli.command {
        Command::Stats => print_stats(&repo, elapsed),
        Command::Check { min, fail_on, vendor, limit } => {
            return Ok(check(&repo, min.into(), fail_on.into(), vendor.as_deref(), limit));
        }
        Command::Resolve { name, kind, key, defaults } => {
            resolve(&repo, &name, kind.map(Into::into), key.as_deref(), defaults)?
        }
        Command::Search { query, limit } => search(&repo, &query, limit),
        Command::Compat { name, kind } => compat(&repo, &name, kind.map(Into::into))?,
    }
    Ok(ExitCode::SUCCESS)
}

fn print_stats(repo: &Repository, elapsed: Duration) {
    let stats = Stats::collect(repo);
    println!("loaded {} in {:.0?}", repo.root().display(), elapsed);
    match repo.schema() {
        Some(schema) => println!("schema: {} options, OrcaSlicer {}", schema.options.len(), schema.source_commit),
        None => println!("schema: none, values are not checked"),
    }
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
        println!("  {:<40} {}", hit.key, hit.label);
    }
    println!("\nvalues ({}):", results.total_values);
    for hit in &results.values {
        println!("  {}  {} = {}", describe(hit.preset), hit.key, hit.snippet);
    }
}

fn resolve(
    repo: &Repository,
    name: &str,
    kind: Option<PresetKind>,
    key_filter: Option<&str>,
    defaults: bool,
) -> Result<()> {
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
        print_preset(repo, id, key_filter, defaults);
    }
    Ok(())
}

fn print_preset(repo: &Repository, id: PresetId, key_filter: Option<&str>, defaults: bool) {
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
    let shown: Vec<_> = values
        .iter()
        .filter(|(key, _)| key_filter.is_none_or(|filter| key.contains(filter)))
        .filter(|(_, resolved)| defaults || resolved.origin != Origin::Default)
        .collect();
    let width = shown.iter().map(|(key, _)| key.len()).max().unwrap_or(0).min(40);
    println!("\nvalues ({} of {} keys; [n] = set by chain entry n, [d] = schema default):", shown.len(), values.len());
    for (key, resolved) in shown {
        let layer = match resolved.origin {
            Origin::Preset(origin) => chain.iter().position(|&link| link == origin).unwrap_or(0).to_string(),
            Origin::Default => "d".to_owned(),
        };
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
