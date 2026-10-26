//! The navigation tree: vendor → machine models (→ printers) / filaments / processes / templates.

use std::collections::HashSet;

use gpui_kit::SharedString;
use gpui_kit::assets::IconName;
use gpui_kit::component::tree::{TreeEntry, TreeItem, TreeState};
use profile_core::PresetKind;

use crate::catalog::Catalog;

/// What a tree row stands for. Encoded into the row id so render callbacks can recover it.
/// All numbers are indexes into the corresponding [`profile_core::Repository`] slices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Node {
    Vendor(usize),
    Group(usize, Group),
    Model(usize),
    Preset(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Models,
    OtherPrinters,
    Filaments,
    Processes,
    Templates,
}

impl Group {
    const ALL: [Group; 5] = [Group::Models, Group::OtherPrinters, Group::Filaments, Group::Processes, Group::Templates];

    pub fn label(self) -> &'static str {
        match self {
            Group::Models => "Machine models",
            Group::OtherPrinters => "Other printers",
            Group::Filaments => "Filaments",
            Group::Processes => "Processes",
            Group::Templates => "Templates",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Group::Models => "models",
            Group::OtherPrinters => "other",
            Group::Filaments => "filaments",
            Group::Processes => "processes",
            Group::Templates => "templates",
        }
    }

    /// Number of children shown under this group.
    pub fn len(self, catalog: &Catalog, vendor: usize) -> usize {
        let contents = &catalog.vendors[vendor];
        match self {
            Group::Models => contents.model_count(),
            Group::OtherPrinters => contents.other_printers().map_or(0, |entry| entry.printers.len()),
            Group::Filaments => contents.filaments.len(),
            Group::Processes => contents.processes.len(),
            Group::Templates => contents.templates.len(),
        }
    }
}

impl Node {
    /// Indexes are local to one load; file paths retain identity across reloads.
    pub fn remap(self, old: &Catalog, new: &Catalog) -> Option<Self> {
        match self {
            Self::Vendor(index) => new.remap_vendor(old, index).map(Self::Vendor),
            Self::Group(index, group) => new.remap_vendor(old, index).map(|index| Self::Group(index, group)),
            Self::Model(index) => new.remap_model(old, index).map(Self::Model),
            Self::Preset(index) => new.remap_preset(old, index).map(Self::Preset),
        }
    }

    pub fn id(self) -> SharedString {
        match self {
            Node::Vendor(vendor) => format!("v:{vendor}"),
            Node::Group(vendor, group) => format!("g:{vendor}:{}", group.key()),
            Node::Model(model) => format!("m:{model}"),
            Node::Preset(preset) => format!("p:{preset}"),
        }
        .into()
    }

    pub fn parse(id: &str) -> Option<Self> {
        let mut parts = id.split(':');
        let node = match (parts.next()?, parts.next()?, parts.next()) {
            ("v", vendor, None) => Node::Vendor(vendor.parse().ok()?),
            ("g", vendor, Some(key)) => {
                Node::Group(vendor.parse().ok()?, Group::ALL.into_iter().find(|group| group.key() == key)?)
            }
            ("m", model, None) => Node::Model(model.parse().ok()?),
            ("p", preset, None) => Node::Preset(preset.parse().ok()?),
            _ => return None,
        };
        parts.next().is_none().then_some(node)
    }
}

pub fn kind_icon(kind: PresetKind) -> IconName {
    match kind {
        PresetKind::Machine => IconName::Settings2,
        PresetKind::Filament => IconName::Spool,
        PresetKind::Process => IconName::Layers,
    }
}

/// Builds every row up front; the tree only flattens expanded folders, so this stays cheap.
pub fn build(catalog: &Catalog) -> Vec<TreeItem> {
    let repo = &catalog.repo;
    let preset_item = |index: usize| TreeItem::new(Node::Preset(index).id(), repo.presets()[index].name().to_owned());
    let preset_group = |vendor: usize, group: Group, presets: &[usize]| {
        TreeItem::new(Node::Group(vendor, group).id(), group.label())
            .children(presets.iter().map(|&index| preset_item(index)))
    };

    catalog
        .vendors
        .iter()
        .enumerate()
        .map(|(vendor, contents)| {
            let mut groups = Vec::new();
            let models: Vec<TreeItem> = contents
                .models
                .iter()
                .filter_map(|entry| {
                    let model = entry.model?;
                    let printers = entry.printers.iter().map(|&index| preset_item(index));
                    Some(TreeItem::new(Node::Model(model).id(), entry.label.clone()).children(printers))
                })
                .collect();
            if !models.is_empty() {
                groups.push(
                    TreeItem::new(Node::Group(vendor, Group::Models).id(), Group::Models.label()).children(models),
                );
            }
            if let Some(other) = contents.other_printers() {
                groups.push(preset_group(vendor, Group::OtherPrinters, &other.printers));
            }
            for (group, presets) in [
                (Group::Filaments, &contents.filaments),
                (Group::Processes, &contents.processes),
                (Group::Templates, &contents.templates),
            ] {
                if !presets.is_empty() {
                    groups.push(preset_group(vendor, group, presets));
                }
            }
            let info = &repo.vendors()[vendor];
            let label = info.display_name.clone().unwrap_or_else(|| info.name.clone());
            TreeItem::new(Node::Vendor(vendor).id(), label).children(groups)
        })
        .collect()
}

/// Expanded folders anywhere in the tree, including inside collapsed ones.
pub fn expanded_nodes(tree: &TreeState) -> Vec<Node> {
    let mut pending: Vec<&TreeItem> =
        (0..).map_while(|ix| tree.entry(ix)).filter(|entry| entry.is_root()).map(TreeEntry::item).collect();
    let mut nodes = Vec::new();
    while let Some(item) = pending.pop() {
        if item.is_expanded()
            && let Some(node) = Node::parse(&item.id)
        {
            nodes.push(node);
        }
        pending.extend(&item.children);
    }
    nodes
}

/// Expands the folders of `nodes` in freshly built `items`.
pub fn expand(items: &[TreeItem], nodes: &[Node]) {
    let ids: HashSet<SharedString> = nodes.iter().map(|node| node.id()).collect();
    let mut pending: Vec<&TreeItem> = items.iter().collect();
    while let Some(item) = pending.pop() {
        if ids.contains(&item.id) {
            item.clone().expanded(true);
        }
        pending.extend(&item.children);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_ids_round_trip() {
        let nodes = [
            Node::Vendor(3),
            Node::Group(3, Group::Models),
            Node::Group(0, Group::OtherPrinters),
            Node::Group(12, Group::Templates),
            Node::Model(41),
            Node::Preset(7000),
        ];
        for node in nodes {
            assert_eq!(Node::parse(&node.id()), Some(node));
        }
        assert_eq!(Node::parse("g:1"), None);
        assert_eq!(Node::parse("p:1:2"), None);
        assert_eq!(Node::parse("x:1"), None);
    }
}
