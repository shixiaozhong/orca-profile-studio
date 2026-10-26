//! The right-hand pane: facts about the selected tree node.

use std::collections::BTreeMap;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use profile_core::{Origin, PresetKind, RawValue, Severity};

use crate::catalog::Catalog;
use crate::tree::{Group, Node, kind_icon};

pub fn render(catalog: &Catalog, node: Node, cx: &App) -> AnyElement {
    match node {
        Node::Vendor(vendor) => render_vendor(catalog, vendor, cx),
        Node::Group(vendor, group) => render_group(catalog, vendor, group, cx),
        Node::Model(model) => render_model(catalog, model, cx),
        Node::Preset(preset) => render_preset(catalog, preset, None, cx),
    }
}

pub fn severity_color(severity: Severity, cx: &App) -> Option<Hsla> {
    match severity {
        Severity::Fatal | Severity::Error => Some(cx.theme().danger),
        Severity::Warning => Some(cx.theme().warning),
        Severity::Info => None,
    }
}

fn render_vendor(catalog: &Catalog, vendor: usize, cx: &App) -> AnyElement {
    let info = &catalog.repo.vendors()[vendor];
    let contents = &catalog.vendors[vendor];
    let title = info.display_name.clone().unwrap_or_else(|| info.name.clone());
    v_flex()
        .child(header(IconName::Building2, title, t!("details.vendor_path", arg0 = info.index_path.display()), cx))
        .child(
            section(t!("details.vendor_index"), cx)
                .child(field(t!("details.version"), or_dash(&info.version), cx))
                .child(field(t!("details.description"), or_dash(&info.description), cx))
                .child(field(t!("details.url"), or_dash(&info.url), cx))
                .child(field(t!("details.force_update"), or_dash(&info.force_update), cx)),
        )
        .child(
            section(t!("details.contents"), cx)
                .child(field(t!("common.models"), contents.model_count().to_string(), cx))
                .child(field(t!("common.printers"), contents.printer_count().to_string(), cx))
                .child(field(t!("common.filaments"), contents.filaments.len().to_string(), cx))
                .child(field(t!("common.processes"), contents.processes.len().to_string(), cx))
                .child(field(t!("common.templates"), contents.templates.len().to_string(), cx)),
        )
        .child(issue_totals(&contents.issues, cx))
        .into_any_element()
}

fn render_group(catalog: &Catalog, vendor: usize, group: Group, cx: &App) -> AnyElement {
    let info = &catalog.repo.vendors()[vendor];
    let count = group.len(catalog, vendor);
    let summary = if group == Group::Models {
        t!("details.models_count", vendor = info.name, count = count)
    } else {
        t!("details.presets_count", vendor = info.name, count = count)
    };
    v_flex()
        .child(header(IconName::FolderOpen, group.label(), summary, cx))
        .child(div().pt_4().text_sm().text_color(cx.theme().muted_foreground).child(t!("details.expand")))
        .into_any_element()
}

fn render_model(catalog: &Catalog, model: usize, cx: &App) -> AnyElement {
    let repo = &catalog.repo;
    let info = &repo.models()[model];
    let printers: Vec<usize> = catalog.vendors[info.vendor.index()]
        .models
        .iter()
        .find(|entry| entry.model == Some(model))
        .map(|entry| entry.printers.clone())
        .unwrap_or_default();

    let mut details = v_flex()
        .child(header(IconName::Printer, info.id.clone(), t!("details.model_path", arg0 = info.path.display()), cx))
        .child(
            section(t!("details.model"), cx)
                .child(field(t!("details.name"), or_dash(&info.name), cx))
                .child(field(t!("details.model_id"), or_dash(&info.model_id), cx))
                .child(field(t!("details.family"), or_dash(&info.family), cx))
                .child(field(t!("details.technology"), or_dash(&info.technology), cx))
                .child(field(t!("details.nozzles"), join_or_dash(&info.variants), cx))
                .child(field(t!("details.bed_model"), or_dash(&info.bed_model), cx))
                .child(field(t!("details.bed_texture"), or_dash(&info.bed_texture), cx))
                .child(field(t!("details.hotend"), or_dash(&info.hotend_model), cx)),
        )
        .child(
            section(t!("details.printer_count", arg0 = printers.len()), cx)
                .children(printers.iter().map(|&index| list_line(repo.presets()[index].name().to_owned()))),
        )
        .child(
            section(t!("details.material_count", arg0 = info.default_materials.len()), cx)
                .children(info.default_materials.iter().map(|material| list_line(material.clone()))),
        );
    if !info.extra.is_empty() {
        let keys: Vec<&str> = info.extra.keys().map(String::as_str).collect();
        details =
            details.child(section(t!("details.other_keys"), cx).child(
                div().text_sm().text_color(cx.theme().muted_foreground).child(SharedString::from(keys.join(", "))),
            ));
    }
    details.into_any_element()
}

/// What a preset is compatible with. Computed when the preset is selected, not while rendering:
/// a filament is checked against every printer.
pub struct Compatibility {
    pub preset: usize,
    /// The condition deciding compatibility with printers, or why it does not parse.
    pub condition: Option<Result<String, String>>,
    /// Names of the compatible printers (filaments and processes).
    pub printers: Vec<String>,
    /// Compatible process and filament presets (printers).
    pub processes: usize,
    pub filaments: usize,
}

impl Compatibility {
    pub fn compute(catalog: &Catalog, preset: usize) -> Self {
        let repo = &catalog.repo;
        let id = repo.presets()[preset].id;
        let index = repo.compatibility();
        let mut compatibility =
            Compatibility { preset, condition: None, printers: Vec::new(), processes: 0, filaments: 0 };
        if repo.presets()[preset].kind == PresetKind::Machine {
            compatibility.processes = index.compatible_with_printer(repo, id, PresetKind::Process).len();
            compatibility.filaments = index.compatible_with_printer(repo, id, PresetKind::Filament).len();
        } else {
            compatibility.condition = index.condition(id, PresetKind::Machine).map(|condition| match condition {
                Ok(condition) => Ok(condition.source().to_owned()),
                Err(err) => Err(err.to_string()),
            });
            let mut printers: Vec<String> = index
                .compatible_printers(repo, id)
                .into_iter()
                .map(|(printer, _)| repo.preset(printer).name().to_owned())
                .collect();
            printers.sort();
            compatibility.printers = printers;
        }
        compatibility
    }
}

fn compatibility_section(
    repo: &profile_core::Repository,
    preset: usize,
    compatibility: &Compatibility,
    cx: &App,
) -> Div {
    let section = section(t!("details.compatibility"), cx);
    if repo.presets()[preset].kind == PresetKind::Machine {
        let summary = t!("details.compat_summary", arg0 = compatibility.processes, arg1 = compatibility.filaments);
        return section
            .child(field(t!("common.processes"), compatibility.processes.to_string(), cx))
            .child(field(t!("common.filaments"), compatibility.filaments.to_string(), cx))
            .child(div().id("compat-summary").test_support().aria_label(summary));
    }
    const SHOWN: usize = 12;
    let printers = &compatibility.printers;
    let plural = if printers.len() == 1 || crate::i18n::is_chinese() { "" } else { "s" };
    let summary = t!("details.compat_printers", arg0 = printers.len(), plural = plural);
    let mut section = section.child(div().id("compat-summary").test_support().aria_label(summary.clone()));
    if let Some(condition) = &compatibility.condition {
        let text = match condition {
            Ok(source) => source.clone(),
            Err(err) => t!("details.invalid_condition", err = err),
        };
        section = section.child(field(t!("details.condition"), text, cx));
    }
    section = section.child(field(t!("common.printers"), printers.len().to_string(), cx));
    section = section.children(printers.iter().take(SHOWN).map(|name| list_line(name.clone())));
    if printers.len() > SHOWN {
        let more = t!("details.more", arg0 = printers.len() - SHOWN);
        section = section.child(div().text_sm().text_color(cx.theme().muted_foreground).child(more));
    }
    section
}

pub fn render_preset(catalog: &Catalog, index: usize, compatibility: Option<&Compatibility>, cx: &App) -> AnyElement {
    let repo = &catalog.repo;
    let preset = &repo.presets()[index];
    let id = preset.id;
    let vendor = &repo.vendors()[preset.vendor.index()];
    let selectable = if repo.is_selectable(id) { t!("details.selectable") } else { t!("details.template") };
    let muted = cx.theme().muted_foreground;

    let mut file =
        section(t!("common.file"), cx).child(field(t!("details.path"), preset.path.display().to_string(), cx));
    if preset.list_name != preset.name() {
        file = file.child(field(t!("details.list_name"), preset.list_name.clone(), cx));
    }
    file = file
        .child(field(t!("details.from"), or_dash(&preset.meta.from), cx))
        .child(field(t!("details.instantiation"), or_dash(&preset.meta.instantiation), cx))
        .child(field(t!("details.setting_id"), or_dash(&preset.meta.setting_id), cx))
        .child(field(t!("details.alias"), repo.alias(id).to_owned(), cx));
    if preset.kind == PresetKind::Filament {
        file = file.child(field(t!("details.filament_id"), repo.filament_id(id).unwrap_or("—").to_owned(), cx));
    }

    let chain = repo.chain(id);
    let mut inheritance =
        section(t!("details.inheritance"), cx).children(chain.iter().enumerate().map(|(depth, &link)| {
            let ancestor = repo.preset(link);
            h_flex()
                .gap_2()
                .text_sm()
                .child(div().w_7().flex_shrink_0().text_color(muted).child(SharedString::from(format!("[{depth}]"))))
                .child(div().flex_shrink_0().child(ancestor.name().to_owned()))
                .child(div().min_w_0().truncate().text_color(muted).child(ancestor.path.display().to_string()))
        }));
    let root = repo.preset(*chain.last().expect("a chain contains its preset"));
    if root.inherits_unresolved
        && let Some(parent) = root.meta.inherits()
    {
        inheritance = inheritance.child(
            div()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(SharedString::from(t!("details.parent_missing", parent = parent))),
        );
    }

    let highlights: &[&str] = match preset.kind {
        PresetKind::Machine => &["printer_model", "printer_variant", "nozzle_diameter", "printable_height"],
        PresetKind::Filament => &["filament_type", "filament_vendor", "nozzle_temperature", "compatible_printers"],
        PresetKind::Process => &["layer_height", "compatible_printers", "compatible_printers_condition"],
    };
    let resolved = repo.resolve(id);
    let from_defaults = resolved.values().filter(|resolved| resolved.origin == Origin::Default).count();
    let mut values = section(t!("common.values"), cx)
        .child(field(t!("details.own_count"), preset.params.len().to_string(), cx))
        .child(field(t!("details.chain_count"), (resolved.len() - from_defaults).to_string(), cx));
    if repo.schema().is_some() {
        values = values.child(field(t!("details.default_count"), from_defaults.to_string(), cx));
    }
    let values = values.children(highlights.iter().map(|&key| {
        let option = repo.schema().and_then(|schema| schema.option(key));
        let label = catalog.translate(option.map_or(key, |option| option.display_label()));
        let value = resolved.get(key).map_or_else(
            || "—".to_owned(),
            |resolved| {
                let mut text = with_unit(
                    format_value(resolved.value),
                    resolved.value,
                    &catalog.translate(option.map_or("", |option| option.sidetext.as_str())),
                );
                if resolved.origin == Origin::Default {
                    text.push_str(&t!("details.default_suffix"));
                }
                text
            },
        );
        field(label.clone(), value, cx)
            .id(SharedString::from(format!("value-{key}")))
            .test_support()
            .aria_label(label.to_owned())
    }));

    let issues: Vec<_> = catalog.issues_of(index).collect();
    let issue_list = section(t!("details.issue_count", arg0 = issues.len()), cx).children(issues.iter().map(|issue| {
        let color = severity_color(issue.severity(), cx).unwrap_or(muted);
        h_flex()
            .gap_2()
            .items_start()
            .text_sm()
            .child(Icon::new(IconName::TriangleAlert).small().text_color(color))
            .child(div().w_48().flex_shrink_0().text_color(muted).child(format!(
                "{} · {}",
                crate::i18n::severity(issue.severity()),
                issue.kind.code()
            )))
            .child(div().flex_1().min_w_0().child(crate::issue_text::issue(&issue.kind)))
    }));

    v_flex()
        .child(header(
            kind_icon(preset.kind),
            preset.name().to_owned(),
            format!("{} · {} · {selectable}", crate::i18n::kind(preset.kind), vendor.name),
            cx,
        ))
        .child(file)
        .child(inheritance)
        .child(values)
        .children(
            compatibility
                .filter(|compatibility| compatibility.preset == index)
                .map(|compatibility| compatibility_section(repo, index, compatibility, cx)),
        )
        .child(issue_list)
        .into_any_element()
}

fn header(icon: IconName, title: impl Into<SharedString>, subtitle: impl Into<SharedString>, cx: &App) -> Div {
    let muted = cx.theme().muted_foreground;
    h_flex()
        .gap_3()
        .items_center()
        .pb_3()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(Icon::new(icon).large().text_color(muted))
        .child(
            v_flex()
                .min_w_0()
                .child({
                    let title = title.into();
                    div()
                        .id("details-title")
                        .test_support()
                        .aria_label(title.clone())
                        .text_lg()
                        .font_semibold()
                        .child(title)
                })
                .child(div().text_sm().text_color(muted).child(subtitle.into())),
        )
}

fn section(title: impl Into<SharedString>, cx: &App) -> Div {
    v_flex()
        .gap_1()
        .pt_4()
        .child(div().pb_1().text_xs().font_semibold().text_color(cx.theme().muted_foreground).child(title.into()))
}

fn field(label: impl Into<SharedString>, value: impl Into<SharedString>, cx: &App) -> Div {
    h_flex()
        .gap_3()
        .items_start()
        .text_sm()
        .child(div().w_40().flex_shrink_0().text_color(cx.theme().muted_foreground).child(label.into()))
        .child(div().flex_1().min_w_0().child(value.into()))
}

fn list_line(text: String) -> Div {
    div().text_sm().truncate().child(text)
}

fn issue_totals(totals: &BTreeMap<Severity, usize>, cx: &App) -> Div {
    let section = section(t!("common.issues"), cx);
    if totals.is_empty() {
        return section.child(div().text_sm().child(t!("common.no_issues")));
    }
    section.children(
        totals.iter().rev().map(|(severity, count)| field(crate::i18n::severity(*severity), count.to_string(), cx)),
    )
}

fn or_dash(value: &Option<String>) -> String {
    value.clone().filter(|value| !value.is_empty()).unwrap_or_else(|| "—".to_owned())
}

fn join_or_dash(values: &[String]) -> String {
    if values.is_empty() { "—".to_owned() } else { values.join(", ") }
}

pub fn format_value(value: &RawValue) -> String {
    match value {
        RawValue::Str(text) => text.clone(),
        RawValue::List(items) => items.join(", "),
    }
}

/// Percent values already carry their unit; preserve that spelling without adding a second `%`.
pub fn with_unit(text: String, value: &RawValue, unit: &str) -> String {
    let has_percent = |value: &str| value.trim_end().ends_with('%');
    let percent_encoded = unit == "%"
        && match value {
            RawValue::Str(value) => has_percent(value),
            RawValue::List(values) => !values.is_empty() && values.iter().all(|value| has_percent(value)),
        };
    if unit.is_empty() || percent_encoded { text } else { format!("{text} {unit}") }
}

#[cfg(test)]
mod tests {
    use super::{format_value, with_unit};
    use profile_core::RawValue;

    #[test]
    fn units_preserve_encoded_percentages_and_array_shape() {
        let _locale = crate::i18n::test_locale();
        let percent = RawValue::Str("15%".into());
        assert_eq!(with_unit(format_value(&percent), &percent, "%"), "15%");
        let list = RawValue::List(vec!["10%".into(), "20%".into()]);
        assert_eq!(with_unit("[\"10%\",\"20%\"]".into(), &list, "%"), "[\"10%\",\"20%\"]");
        let temperature = RawValue::Str("210".into());
        assert_eq!(with_unit(format_value(&temperature), &temperature, "℃"), "210 ℃");
        assert_eq!(with_unit("15".into(), &RawValue::Str("15".into()), "%"), "15 %");
    }
}
