//! The command palette (⌘K): app commands plus global search over presets, options and values.

use gpui_kit::assets::IconName;
use gpui_kit::component::command::{CommandGroup, CommandItem};

use crate::catalog::Catalog;

/// Hits shown per result group.
const LIMIT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCommand {
    Reload,
    Open,
    ShowOverview,
    ShowValues,
}

impl AppCommand {
    const ALL: [AppCommand; 4] =
        [AppCommand::Reload, AppCommand::Open, AppCommand::ShowOverview, AppCommand::ShowValues];

    fn label(self) -> &'static str {
        match self {
            AppCommand::Reload => "Reload profiles from disk",
            AppCommand::Open => "Open profiles directory…",
            AppCommand::ShowOverview => "Show preset overview",
            AppCommand::ShowValues => "Show preset values",
        }
    }

    fn icon(self) -> IconName {
        match self {
            AppCommand::Reload => IconName::RefreshCw,
            AppCommand::Open => IconName::FolderOpen,
            AppCommand::ShowOverview => IconName::Info,
            AppCommand::ShowValues => IconName::Table,
        }
    }

    /// Whether the command applies to the current selection.
    fn available(self, preset_selected: bool) -> bool {
        match self {
            AppCommand::ShowOverview | AppCommand::ShowValues => preset_selected,
            AppCommand::Reload | AppCommand::Open => true,
        }
    }
}

/// What confirming a palette entry does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Command(AppCommand),
    /// Select a preset (index into the repository's presets).
    Preset(usize),
    /// Show an option in the Values tab of the selected preset.
    Option(String),
    /// Show a value a preset file sets.
    Value(usize, String),
}

/// The palette's groups for `query`, and what each row of each group targets, by
/// `IndexPath { section, row }`. Empty groups are left out.
pub fn build(catalog: &Catalog, query: &str, preset_selected: bool) -> (Vec<CommandGroup>, Vec<Vec<Target>>) {
    let repo = &catalog.repo;
    let lower = query.trim().to_lowercase();
    let mut groups: Vec<(String, Vec<(CommandItem, Target)>)> = Vec::new();

    let commands = AppCommand::ALL
        .into_iter()
        .filter(|command| command.available(preset_selected))
        .filter(|command| lower.split_whitespace().all(|term| command.label().to_lowercase().contains(term)))
        .map(|command| (CommandItem::new().label(command.label()).icon(command.icon()), Target::Command(command)))
        .collect();
    groups.push(("Commands".to_owned(), commands));

    if !lower.is_empty() {
        let results = catalog.search.search(query, LIMIT);
        let describe = |index: usize| {
            let preset = &repo.presets()[index];
            format!("{} / {}", repo.vendor(preset.vendor).name, preset.name())
        };
        let presets = results
            .presets
            .iter()
            .map(|id| {
                let label = format!("{} ({})", describe(id.index()), repo.preset(*id).kind);
                (CommandItem::new().label(label).icon(IconName::File), Target::Preset(id.index()))
            })
            .collect();
        groups.push((format!("Presets ({})", results.total_presets), presets));
        if preset_selected {
            let options = results
                .options
                .iter()
                .map(|hit| {
                    let label = format!("{} — {}", hit.label, hit.key);
                    (CommandItem::new().label(label).icon(IconName::SlidersHorizontal), Target::Option(hit.key.clone()))
                })
                .collect();
            groups.push((format!("Options ({})", results.total_options), options));
        }
        let values = results
            .values
            .iter()
            .map(|hit| {
                let label = format!("{} · {} = {}", describe(hit.preset.index()), hit.key, hit.snippet);
                (
                    CommandItem::new().label(label).icon(IconName::Search),
                    Target::Value(hit.preset.index(), hit.key.clone()),
                )
            })
            .collect();
        groups.push((format!("Values set in files ({})", results.total_values), values));
    }

    groups
        .into_iter()
        .filter(|(_, items)| !items.is_empty())
        .map(|(label, items)| {
            let (items, targets): (Vec<_>, Vec<_>) = items.into_iter().unzip();
            (CommandGroup::new().label(label).items(items), targets)
        })
        .unzip()
}
