//! The right-hand pane: facts about the selected tree node.

use std::collections::BTreeMap;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use profile_core::{PresetKind, RawValue, Severity};

use crate::catalog::Catalog;
use crate::tree::{Group, Node, kind_icon};

pub fn render(catalog: &Catalog, node: Node, cx: &App) -> AnyElement {
    match node {
        Node::Vendor(vendor) => render_vendor(catalog, vendor, cx),
        Node::Group(vendor, group) => render_group(catalog, vendor, group, cx),
        Node::Model(model) => render_model(catalog, model, cx),
        Node::Preset(preset) => render_preset(catalog, preset, cx),
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
        .child(header(IconName::Building2, title, format!("vendor · {}", info.index_path.display()), cx))
        .child(
            section("Vendor index", cx)
                .child(field("Version", or_dash(&info.version), cx))
                .child(field("Description", or_dash(&info.description), cx))
                .child(field("Update URL", or_dash(&info.url), cx))
                .child(field("Force update", or_dash(&info.force_update), cx)),
        )
        .child(
            section("Contents", cx)
                .child(field("Machine models", contents.model_count().to_string(), cx))
                .child(field("Printers", contents.printer_count().to_string(), cx))
                .child(field("Filaments", contents.filaments.len().to_string(), cx))
                .child(field("Processes", contents.processes.len().to_string(), cx))
                .child(field("Templates", contents.templates.len().to_string(), cx)),
        )
        .child(issue_totals(&contents.issues, cx))
        .into_any_element()
}

fn render_group(catalog: &Catalog, vendor: usize, group: Group, cx: &App) -> AnyElement {
    let info = &catalog.repo.vendors()[vendor];
    let count = group.len(catalog, vendor);
    let noun = if group == Group::Models { "models" } else { "presets" };
    v_flex()
        .child(header(IconName::FolderOpen, group.label(), format!("{} · {count} {noun}", info.name), cx))
        .child(
            div()
                .pt_4()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Expand the group and select an entry to see its details."),
        )
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
        .child(header(IconName::Printer, info.id.clone(), format!("machine model · {}", info.path.display()), cx))
        .child(
            section("Model", cx)
                .child(field("Name", or_dash(&info.name), cx))
                .child(field("Model ID", or_dash(&info.model_id), cx))
                .child(field("Family", or_dash(&info.family), cx))
                .child(field("Technology", or_dash(&info.technology), cx))
                .child(field("Nozzle variants", join_or_dash(&info.variants), cx))
                .child(field("Bed model", or_dash(&info.bed_model), cx))
                .child(field("Bed texture", or_dash(&info.bed_texture), cx))
                .child(field("Hotend model", or_dash(&info.hotend_model), cx)),
        )
        .child(
            section(&format!("Printers ({})", printers.len()), cx)
                .children(printers.iter().map(|&index| list_line(repo.presets()[index].name().to_owned()))),
        )
        .child(
            section(&format!("Default materials ({})", info.default_materials.len()), cx)
                .children(info.default_materials.iter().map(|material| list_line(material.clone()))),
        );
    if !info.extra.is_empty() {
        let keys: Vec<&str> = info.extra.keys().map(String::as_str).collect();
        details =
            details.child(section("Other keys", cx).child(
                div().text_sm().text_color(cx.theme().muted_foreground).child(SharedString::from(keys.join(", "))),
            ));
    }
    details.into_any_element()
}

fn render_preset(catalog: &Catalog, index: usize, cx: &App) -> AnyElement {
    let repo = &catalog.repo;
    let preset = &repo.presets()[index];
    let id = preset.id;
    let vendor = &repo.vendors()[preset.vendor.index()];
    let selectable = if repo.is_selectable(id) { "selectable" } else { "template" };
    let muted = cx.theme().muted_foreground;

    let mut file = section("File", cx).child(field("Path", preset.path.display().to_string(), cx));
    if preset.list_name != preset.name() {
        file = file.child(field("Listed as", preset.list_name.clone(), cx));
    }
    file = file
        .child(field("From", or_dash(&preset.meta.from), cx))
        .child(field("Instantiation", or_dash(&preset.meta.instantiation), cx))
        .child(field("Setting ID", or_dash(&preset.meta.setting_id), cx))
        .child(field("Alias", repo.alias(id).to_owned(), cx));
    if preset.kind == PresetKind::Filament {
        file = file.child(field("Filament ID", repo.filament_id(id).unwrap_or("—").to_owned(), cx));
    }

    let chain = repo.chain(id);
    let mut inheritance =
        section("Inheritance (nearest first)", cx).children(chain.iter().enumerate().map(|(depth, &link)| {
            let ancestor = repo.preset(link);
            h_flex()
                .gap_2()
                .text_sm()
                .child(
                    div().w(px(28.)).flex_shrink_0().text_color(muted).child(SharedString::from(format!("[{depth}]"))),
                )
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
                .child(SharedString::from(format!("[?] {parent} (not found)"))),
        );
    }

    let highlights: &[&str] = match preset.kind {
        PresetKind::Machine => &["printer_model", "printer_variant", "nozzle_diameter", "printable_height"],
        PresetKind::Filament => &["filament_type", "filament_vendor", "nozzle_temperature", "compatible_printers"],
        PresetKind::Process => &["layer_height", "compatible_printers", "compatible_printers_condition"],
    };
    let resolved = repo.resolve(id);
    let values = section("Values", cx)
        .child(field("Set in this file", preset.params.len().to_string(), cx))
        .child(field("Effective from files", resolved.len().to_string(), cx))
        .children(highlights.iter().map(|&key| {
            let value = resolved.get(key).map_or_else(|| "—".to_owned(), |resolved| format_value(resolved.value));
            field(key, value, cx)
        }));

    let issues: Vec<_> = catalog.issues_of(index).collect();
    let issue_list = section(&format!("Issues ({})", issues.len()), cx).children(issues.iter().map(|issue| {
        let color = severity_color(issue.severity(), cx).unwrap_or(muted);
        h_flex()
            .gap_2()
            .items_start()
            .text_sm()
            .child(Icon::new(IconName::TriangleAlert).small().text_color(color))
            .child(div().w(px(200.)).flex_shrink_0().text_color(muted).child(issue.kind.code()))
            .child(div().flex_1().min_w_0().child(issue.kind.to_string()))
    }));

    v_flex()
        .child(header(
            kind_icon(preset.kind),
            preset.name().to_owned(),
            format!("{} · {} · {selectable}", preset.kind, vendor.name),
            cx,
        ))
        .child(file)
        .child(inheritance)
        .child(values)
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

fn section(title: &str, cx: &App) -> Div {
    v_flex().gap_1().pt_4().child(
        div().pb_1().text_xs().font_semibold().text_color(cx.theme().muted_foreground).child(title.to_uppercase()),
    )
}

fn field(label: &str, value: impl Into<SharedString>, cx: &App) -> Div {
    h_flex()
        .gap_3()
        .items_start()
        .text_sm()
        .child(div().w(px(160.)).flex_shrink_0().text_color(cx.theme().muted_foreground).child(label.to_owned()))
        .child(div().flex_1().min_w_0().child(value.into()))
}

fn list_line(text: String) -> Div {
    div().text_sm().truncate().child(text)
}

fn issue_totals(totals: &BTreeMap<Severity, usize>, cx: &App) -> Div {
    let section = section("Issues", cx);
    if totals.is_empty() {
        return section.child(div().text_sm().child("No issues"));
    }
    section.children(totals.iter().rev().map(|(severity, count)| field(severity.as_str(), count.to_string(), cx)))
}

fn or_dash(value: &Option<String>) -> String {
    value.clone().filter(|value| !value.is_empty()).unwrap_or_else(|| "—".to_owned())
}

fn join_or_dash(values: &[String]) -> String {
    if values.is_empty() { "—".to_owned() } else { values.join(", ") }
}

fn format_value(value: &RawValue) -> String {
    match value {
        RawValue::Str(text) => text.clone(),
        RawValue::List(items) => items.join(", "),
    }
}
