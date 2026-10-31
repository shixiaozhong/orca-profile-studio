//! The "Used by" section of a preset or printer model (M19): what inherits from it or names it.

use std::collections::HashSet;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use profile_core::Referrer;

use crate::app::StudioApp;
use crate::catalog::Catalog;
use crate::details::{field, section};
use crate::tree::Node;

/// Rows shown per group before "Show all".
const SHOWN: usize = 12;

/// Where clicking a reference goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A preset, on the Values tab at `key` when the reference is a value.
    Preset {
        preset: usize,
        key: Option<&'static str>,
    },
    Model(usize),
}

/// A group of references, keyed by what refers: the children, or a key such as `compatible_printers`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GroupKey {
    Children,
    Key(&'static str),
}

/// The references of the selected node. Computed when it is selected, not while rendering:
/// every preset is checked.
pub struct ReferenceList {
    pub node: Node,
    /// `None` for a printer model, which has no descendants.
    descendants: Option<usize>,
    ambiguous: bool,
    groups: Vec<(GroupKey, Vec<Referrer>)>,
}

impl ReferenceList {
    fn of_preset(catalog: &Catalog, preset: usize) -> Self {
        let refs = catalog.repo.references(catalog.repo.presets()[preset].id);
        let mut groups = Vec::new();
        if !refs.children.is_empty() {
            let children = refs.children.iter().map(|&id| Referrer::Preset { id, key: "inherits", set_by: id });
            groups.push((GroupKey::Children, children.collect()));
        }
        let mut list = ReferenceList {
            node: Node::Preset(preset),
            descendants: Some(refs.descendants),
            ambiguous: refs.ambiguous,
            groups,
        };
        list.group(refs.referrers);
        list
    }

    fn of_model(catalog: &Catalog, model: usize) -> Self {
        let mut list =
            ReferenceList { node: Node::Model(model), descendants: None, ambiguous: false, groups: Vec::new() };
        list.group(catalog.repo.model_references(model));
        list
    }

    pub fn of(catalog: &Catalog, node: Option<Node>) -> Option<Self> {
        match node? {
            Node::Preset(preset) => Some(Self::of_preset(catalog, preset)),
            Node::Model(model) => Some(Self::of_model(catalog, model)),
            Node::Vendor(_) | Node::Group(..) => None,
        }
    }

    /// Groups referrers by key, in the order the keys first appear.
    fn group(&mut self, referrers: Vec<Referrer>) {
        for referrer in referrers {
            let key = match referrer {
                Referrer::Preset { key, .. } => key,
                Referrer::Model(_) => "default_materials",
            };
            match self.groups.iter_mut().find(|(group, _)| *group == GroupKey::Key(key)) {
                Some((_, rows)) => rows.push(referrer),
                None => self.groups.push((GroupKey::Key(key), vec![referrer])),
            }
        }
    }
}

/// The section body. `expanded` names the groups showing every row; `unsaved` notes that edits
/// are not included.
pub fn render(
    catalog: &Catalog,
    list: &ReferenceList,
    expanded: &HashSet<GroupKey>,
    unsaved: bool,
    cx: &mut Context<StudioApp>,
) -> Div {
    let repo = &catalog.repo;
    let muted = cx.theme().muted_foreground;
    let mut section = section(t!("refs.title"), cx);
    if let Some(descendants) = list.descendants {
        section = section.child(field(t!("refs.descendants"), descendants.to_string(), cx));
    }
    if list.ambiguous {
        section = section.child(
            h_flex()
                .id("refs-ambiguous")
                .gap_2()
                .text_sm()
                .text_color(cx.theme().warning)
                .child(Icon::new(IconName::TriangleAlert).small())
                .child(t!("refs.ambiguous")),
        );
    }
    if list.groups.is_empty() {
        let empty = if list.descendants.is_some() { t!("refs.none") } else { t!("refs.no_printers") };
        return section.child(div().id("refs-empty").text_sm().text_color(muted).child(empty));
    }
    if unsaved {
        section = section.child(div().text_sm().text_color(muted).child(t!("refs.unsaved")));
    }
    for (group, referrers) in &list.groups {
        let title = match group {
            GroupKey::Children => t!("refs.children", arg0 = referrers.len()),
            GroupKey::Key(key) => format!("{key} ({})", referrers.len()),
        };
        let group_id = match group {
            GroupKey::Children => "children",
            GroupKey::Key(key) => key,
        };
        let shown = if expanded.contains(group) { referrers.len() } else { SHOWN.min(referrers.len()) };
        let rows = referrers[..shown].iter().map(|referrer| {
            let (target, label, detail) = match *referrer {
                Referrer::Preset { id, key, set_by } => {
                    let preset = repo.preset(id);
                    let key = (*group != GroupKey::Children).then_some(key);
                    let mut detail = repo.vendor(preset.vendor).name.clone();
                    if id != set_by {
                        detail = t!("refs.inherited", vendor = detail, parent = repo.preset(set_by).name());
                    }
                    (Target::Preset { preset: id.index(), key }, preset.name().to_owned(), detail)
                }
                Referrer::Model(model) => {
                    let info = &repo.models()[model];
                    let detail = t!("refs.model", vendor = repo.vendor(info.vendor).name);
                    (Target::Model(model), info.id.clone(), detail)
                }
            };
            let index = match target {
                Target::Preset { preset, .. } => preset,
                Target::Model(model) => model,
            };
            h_flex()
                .gap_2()
                .min_w_0()
                .child(
                    Button::new((SharedString::from(format!("ref-{group_id}")), index))
                        .ghost()
                        .xsmall()
                        .label(label)
                        .on_click(cx.listener(move |app, _, window, cx| app.open_reference(target, window, cx))),
                )
                .child(div().min_w_0().truncate().text_sm().text_color(muted).child(detail))
        });
        let mut rows = v_flex().children(rows);
        if shown < referrers.len() {
            let group = *group;
            rows = rows.child(
                h_flex().child(
                    Button::new(SharedString::from(format!("refs-all-{group_id}")))
                        .ghost()
                        .xsmall()
                        .label(t!("refs.show_all", arg0 = referrers.len()))
                        .on_click(cx.listener(move |app, _, _, cx| app.expand_references(group, cx))),
                ),
            );
        }
        section = section.child(
            v_flex().pt_2().child(div().text_xs().text_color(muted).child(title)).child(
                rows.id(SharedString::from(format!("refs-{group_id}")))
                    .test_support()
                    .aria_label(SharedString::from(format!("{group_id} {}", referrers.len()))),
            ),
        );
    }
    section
}
