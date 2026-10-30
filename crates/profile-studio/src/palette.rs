//! The command palette (secondary-k): app commands plus global search over presets, options and values.

use gpui_kit::assets::IconName;
use gpui_kit::component::command::{CommandGroup, CommandItem};

use crate::catalog::Catalog;
use crate::i18n::LanguagePreference;
use crate::settings::ThemePreference;

/// Hits shown per result group.
const LIMIT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCommand {
    Reload,
    Open,
    ShowOverview,
    ShowValues,
    ShowIssues,
    ManagePresets,
    ManageModels,
    BatchEdit,
    NewWindow,
    Theme(ThemePreference),
    Language(LanguagePreference),
}

impl AppCommand {
    const ALL: [AppCommand; 15] = [
        AppCommand::Language(LanguagePreference::System),
        AppCommand::Language(LanguagePreference::English),
        AppCommand::Language(LanguagePreference::Chinese),
        AppCommand::Reload,
        AppCommand::Open,
        AppCommand::ShowOverview,
        AppCommand::ShowValues,
        AppCommand::ShowIssues,
        AppCommand::ManagePresets,
        AppCommand::ManageModels,
        AppCommand::BatchEdit,
        AppCommand::NewWindow,
        AppCommand::Theme(ThemePreference::System),
        AppCommand::Theme(ThemePreference::Light),
        AppCommand::Theme(ThemePreference::Dark),
    ];

    fn label(self) -> String {
        match self {
            AppCommand::Language(language) => t!("language.command", language = language.label()),
            AppCommand::Reload => t!("palette.reload"),
            AppCommand::Open => t!("palette.open"),
            AppCommand::ShowOverview => t!("palette.overview"),
            AppCommand::ShowValues => t!("palette.values"),
            AppCommand::ShowIssues => t!("palette.issues"),
            AppCommand::BatchEdit => t!("common.batch_edit"),
            AppCommand::ManageModels => t!("models.title"),
            AppCommand::ManagePresets => t!("common.manage_presets"),
            AppCommand::NewWindow => t!("palette.new_window"),
            AppCommand::Theme(theme) => t!("palette.theme", arg0 = theme.label()),
        }
    }

    fn icon(self) -> IconName {
        match self {
            AppCommand::Reload => IconName::RefreshCw,
            AppCommand::Open => IconName::FolderOpen,
            AppCommand::ShowOverview => IconName::Info,
            AppCommand::ShowValues => IconName::Table,
            AppCommand::ShowIssues => IconName::TriangleAlert,
            AppCommand::ManagePresets => IconName::Files,
            AppCommand::ManageModels => IconName::Printer,
            AppCommand::BatchEdit => IconName::SlidersHorizontal,
            AppCommand::NewWindow => IconName::AppWindow,
            AppCommand::Theme(_) => IconName::Palette,
            AppCommand::Language(_) => IconName::Languages,
        }
    }

    /// Whether the command applies to the current selection.
    fn available(self, preset_selected: bool) -> bool {
        match self {
            AppCommand::ShowOverview | AppCommand::ShowValues => preset_selected,
            AppCommand::Reload
            | AppCommand::Open
            | AppCommand::ShowIssues
            | AppCommand::BatchEdit
            | AppCommand::ManageModels
            | AppCommand::ManagePresets
            | AppCommand::NewWindow
            | AppCommand::Theme(_)
            | AppCommand::Language(_) => true,
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
pub fn build(
    catalog: &Catalog,
    query: &str,
    preset_selected: bool,
    theme: ThemePreference,
    language: LanguagePreference,
) -> (Vec<CommandGroup>, Vec<Vec<Target>>) {
    let repo = &catalog.repo;
    let lower = query.trim().to_lowercase();
    let mut groups: Vec<(String, Vec<(CommandItem, Target)>)> = Vec::new();

    let commands = AppCommand::ALL
        .into_iter()
        .filter(|command| command.available(preset_selected))
        .filter(|command| lower.split_whitespace().all(|term| command.label().to_lowercase().contains(term)))
        .map(|command| {
            let item = CommandItem::new().label(command.label()).icon(command.icon());
            // The current theme carries a check mark.
            let item = item.checked(command == AppCommand::Theme(theme) || command == AppCommand::Language(language));
            (item, Target::Command(command))
        })
        .collect();
    groups.push((t!("palette.commands"), commands));

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
                let label = format!("{} ({})", describe(id.index()), crate::i18n::kind(repo.preset(*id).kind));
                (CommandItem::new().label(label).icon(IconName::File), Target::Preset(id.index()))
            })
            .collect();
        groups.push((t!("palette.presets", arg0 = results.total_presets), presets));
        if preset_selected {
            let options = results
                .options
                .iter()
                .map(|hit| {
                    (
                        CommandItem::new().label(hit.key.clone()).icon(IconName::SlidersHorizontal),
                        Target::Option(hit.key.clone()),
                    )
                })
                .collect();
            groups.push((t!("palette.options", arg0 = results.total_options), options));
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
        groups.push((t!("palette.values_group", arg0 = results.total_values), values));
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
