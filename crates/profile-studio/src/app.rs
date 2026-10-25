//! The main window: toolbar, vendor tree, details pane and status bar.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tree::{TreeEntry, TreeState, tree};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, h_resizable, resizable_panel,
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use profile_core::{Repository, Schema, Severity};

use crate::catalog::Catalog;
use crate::details::{self, severity_color};
use crate::edits::Edits;
use crate::tree::{self as nav, Node, kind_icon};
use crate::values::ValuesView;

enum LoadState {
    /// No profiles directory chosen yet.
    Empty,
    Loading(PathBuf),
    Ready(Arc<Catalog>),
    Failed(PathBuf, String),
}

/// Tabs of the details pane when a preset is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PresetTab {
    Overview,
    Values,
}

pub struct StudioApp {
    profiles_dir: Option<PathBuf>,
    state: LoadState,
    tree: Entity<TreeState>,
    selected: Option<Node>,
    preset_tab: PresetTab,
    /// The values table of the selected preset; created when the Values tab is first shown.
    values: Option<Entity<ValuesView>>,
    /// Unsaved edits of the loaded profiles.
    edits: Option<Entity<Edits>>,
    /// Reload or Open was asked for while edits are unsaved; the next click discards them.
    confirm_discard: bool,
    load_task: Option<Task<()>>,
    _tree_observer: Subscription,
}

impl StudioApp {
    pub fn new(profiles_dir: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let tree_observer = cx.observe(&tree, |this, tree, cx| {
            let selected = tree.read(cx).selected_item().and_then(|item| Node::parse(&item.id));
            if selected != this.selected {
                this.selected = selected;
                cx.notify();
            }
        });
        let mut app = StudioApp {
            profiles_dir: None,
            state: LoadState::Empty,
            tree,
            selected: None,
            preset_tab: PresetTab::Overview,
            values: None,
            edits: None,
            confirm_discard: false,
            load_task: None,
            _tree_observer: tree_observer,
        };
        if let Some(dir) = profiles_dir {
            app.load(dir, cx);
        }
        app
    }

    /// Loads `dir` on the background executor; a newer request supersedes a running one.
    fn load(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        self.profiles_dir = Some(dir.clone());
        self.state = LoadState::Loading(dir.clone());
        self.selected = None;
        self.values = None;
        self.edits = None;
        self.confirm_discard = false;
        self.tree.update(cx, |tree, cx| tree.set_items(Vec::new(), cx));

        let job = cx.background_executor().spawn({
            let dir = dir.clone();
            async move {
                let started = Instant::now();
                Repository::load_with_schema(&dir, Schema::bundled())
                    .map(|repo| Catalog::new(repo, started.elapsed()))
                    .map_err(|err| err.to_string())
            }
        });
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            this.update(cx, |this, cx| this.finish_load(dir, result, cx)).ok();
        }));
        cx.notify();
    }

    fn finish_load(&mut self, dir: PathBuf, result: Result<Catalog, String>, cx: &mut Context<Self>) {
        if self.profiles_dir.as_ref() != Some(&dir) {
            return;
        }
        self.state = match result {
            Ok(catalog) => {
                let catalog = Arc::new(catalog);
                let items = nav::build(&catalog);
                self.tree.update(cx, |tree, cx| tree.set_items(items, cx));
                let edits = cx.new(|_| Edits::new(catalog.clone()));
                cx.observe(&edits, |_, _, cx| cx.notify()).detach();
                self.edits = Some(edits);
                LoadState::Ready(catalog)
            }
            Err(err) => LoadState::Failed(dir, err),
        };
        self.load_task = None;
        cx.notify();
    }

    /// Whether unsaved edits may be dropped now; the first request only arms the confirmation.
    fn may_discard(&mut self, cx: &mut Context<Self>) -> bool {
        let dirty = self.edits.as_ref().is_some_and(|edits| !edits.read(cx).dirty().is_empty());
        if dirty && !self.confirm_discard {
            self.confirm_discard = true;
            cx.notify();
            return false;
        }
        true
    }

    fn choose_folder(&mut self, cx: &mut Context<Self>) {
        if !self.may_discard(cx) {
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = picked.await
                && let Some(dir) = paths.pop()
            {
                this.update(cx, |this, cx| this.load(dir, cx)).ok();
            }
        })
        .detach();
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        if !self.may_discard(cx) {
            return;
        }
        if let Some(dir) = self.profiles_dir.clone() {
            self.load(dir, cx);
        }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let location = self
            .profiles_dir
            .as_ref()
            .map_or_else(|| "No profiles directory".to_owned(), |dir| dir.display().to_string());
        h_flex()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .child(Icon::new(IconName::FolderTree).text_color(theme.muted_foreground))
            .child(div().font_semibold().child("Orca Profile Studio"))
            .child(div().flex_1().min_w_0().truncate().text_sm().text_color(theme.muted_foreground).child(location))
            .child(
                Button::new("open")
                    .small()
                    .icon(IconName::FolderOpen)
                    .label("Open…")
                    .on_click(cx.listener(|this, _, _, cx| this.choose_folder(cx))),
            )
            .when(self.confirm_discard, |bar| {
                bar.child(
                    div()
                        .id("discard-warning")
                        .test_support()
                        .text_sm()
                        .text_color(theme.warning)
                        .child("Unsaved edits will be lost — click again to continue"),
                )
            })
            .child(
                Button::new("reload")
                    .small()
                    .icon(IconName::RefreshCw)
                    .tooltip("Reload from disk")
                    .disabled(self.profiles_dir.is_none() || matches!(self.state, LoadState::Loading(_)))
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
    }

    fn render_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match &self.state {
            LoadState::Empty => placeholder(
                IconName::FolderOpen,
                "Open OrcaSlicer's resources/profiles directory to start.".to_owned(),
                cx,
            ),
            LoadState::Loading(dir) => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .child(Spinner::new().large())
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(format!("Loading {}…", dir.display()))),
                )
                .into_any_element(),
            LoadState::Failed(dir, err) => {
                placeholder(IconName::TriangleAlert, format!("Could not load {}: {err}", dir.display()), cx)
            }
            LoadState::Ready(catalog) => {
                let catalog = catalog.clone();
                self.render_browser(&catalog, window, cx)
            }
        }
    }

    fn render_browser(&mut self, catalog: &Arc<Catalog>, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let rows = catalog.clone();
        let edits = self.edits.clone();
        let navigation = tree(&self.tree, move |ix, entry, _, _, cx| {
            let dirty = |preset| edits.as_ref().is_some_and(|edits| edits.read(cx).is_dirty(preset));
            render_row(&rows, ix, entry, &dirty, cx)
        })
        .size_full();
        let scrolling = |content: AnyElement| div().id("details").size_full().overflow_y_scroll().child(content);
        let details = match self.selected {
            Some(Node::Preset(index)) => self.render_preset(catalog, index, window, cx),
            Some(node) => scrolling(details::render(catalog, node, cx)).into_any_element(),
            None => scrolling(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Select a vendor, model or preset in the tree.")
                    .into_any_element(),
            )
            .into_any_element(),
        };
        h_resizable("browser")
            .child(
                resizable_panel()
                    .size(px(380.))
                    .size_range(px(240.)..px(720.))
                    .child(div().size_full().py_1().child(navigation)),
            )
            .child(resizable_panel().child(div().size_full().p_4().child(details)))
            .into_any_element()
    }

    /// A preset's details: the overview, or the values table with every key and its layers.
    fn render_preset(
        &mut self,
        catalog: &Arc<Catalog>,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tabs = TabBar::new("preset-tabs")
            .underline()
            .small()
            .selected_index(self.preset_tab as usize)
            .child(Tab::new().label("Overview"))
            .child(Tab::new().label("Values"))
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                this.preset_tab = if *ix == 1 { PresetTab::Values } else { PresetTab::Overview };
                cx.notify();
            }));
        let content = match self.preset_tab {
            PresetTab::Overview => div()
                .id("details")
                .size_full()
                .overflow_y_scroll()
                .child(details::render(catalog, Node::Preset(index), cx))
                .into_any_element(),
            PresetTab::Values => {
                let current = self.values.as_ref().filter(|values| values.read(cx).preset() == index).cloned();
                let edits = self.edits.clone().expect("loaded profiles have an edit model");
                let values = current.unwrap_or_else(|| {
                    let values = cx.new(|cx| ValuesView::new(catalog, &edits, index, window, cx));
                    self.values = Some(values.clone());
                    values
                });
                div().size_full().child(values).into_any_element()
            }
        };
        v_flex().size_full().child(tabs).child(div().flex_1().min_h_0().pt_3().child(content)).into_any_element()
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (summary, issues) = match &self.state {
            LoadState::Ready(catalog) => {
                let repo = &catalog.repo;
                let summary = format!(
                    "{} vendors · {} machine models · {} presets · loaded in {} ms",
                    repo.vendors().len(),
                    repo.models().len(),
                    repo.presets().len(),
                    catalog.load_time.as_millis()
                );
                let issues: Vec<String> = catalog
                    .issue_totals()
                    .iter()
                    .rev()
                    .map(|(severity, count)| format!("{count} {severity}"))
                    .collect();
                (summary, if issues.is_empty() { "no issues".to_owned() } else { issues.join(" · ") })
            }
            LoadState::Loading(_) => ("Loading…".to_owned(), String::new()),
            LoadState::Empty | LoadState::Failed(..) => (String::new(), String::new()),
        };
        h_flex()
            .justify_between()
            .gap_4()
            .px_3()
            .py_1()
            .border_t_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(div().id("status-summary").test_support().aria_label(summary.clone()).child(summary))
            .child(issues)
    }
}

impl Render for StudioApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_toolbar(cx))
            .child(div().flex_1().min_h_0().child(self.render_body(window, cx)))
            .child(self.render_status(cx))
    }
}

fn render_row(catalog: &Catalog, ix: usize, entry: &TreeEntry, dirty: &dyn Fn(usize) -> bool, cx: &App) -> ListItem {
    let muted = cx.theme().muted_foreground;
    let edited = matches!(Node::parse(&entry.item().id), Some(Node::Preset(index)) if dirty(index));
    let (icon, count, severity, dimmed) = match Node::parse(&entry.item().id) {
        Some(Node::Vendor(vendor)) => {
            let contents = &catalog.vendors[vendor];
            let total = contents.printer_count() + contents.filaments.len() + contents.processes.len();
            let has_errors = contents.issues.range(Severity::Error..).next().is_some();
            (IconName::Building2, Some(total), has_errors.then_some(Severity::Error), false)
        }
        Some(Node::Group(vendor, group)) => {
            let icon = if entry.is_expanded() { IconName::FolderOpen } else { IconName::Folder };
            (icon, Some(group.len(catalog, vendor)), None, false)
        }
        Some(Node::Model(model)) => (IconName::Printer, Some(catalog.model_printer_counts[model]), None, false),
        Some(Node::Preset(index)) => {
            let preset = &catalog.repo.presets()[index];
            let severity = catalog.preset_severity[index].filter(|severity| *severity >= Severity::Warning);
            (kind_icon(preset.kind), None, severity, !catalog.repo.is_selectable(preset.id))
        }
        None => (IconName::Circle, None, None, false),
    };
    let chevron = if entry.is_folder() {
        let icon = if entry.is_expanded() { IconName::ChevronDown } else { IconName::ChevronRight };
        Icon::new(icon).xsmall().text_color(muted).into_any_element()
    } else {
        div().size_3().flex_shrink_0().into_any_element()
    };

    ListItem::new(("tree-row", ix)).py_0p5().pl(px(8. + 14. * entry.depth() as f32)).pr_2().child(
        h_flex()
            .w_full()
            .min_w_0()
            .gap_1p5()
            .items_center()
            .child(chevron)
            .child(Icon::new(icon).small().text_color(muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .when(dimmed, |label| label.text_color(muted))
                    .child(entry.item().label.clone()),
            )
            .when(edited, |row| row.child(Icon::new(IconName::Pencil).xsmall().text_color(cx.theme().primary)))
            .when_some(severity.and_then(|severity| severity_color(severity, cx)), |row, color| {
                row.child(Icon::new(IconName::TriangleAlert).xsmall().text_color(color))
            })
            .when_some(count, |row, count| {
                row.child(div().text_xs().text_color(muted).child(SharedString::from(count.to_string())))
            }),
    )
}

fn placeholder(icon: IconName, message: String, cx: &App) -> AnyElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .child(Icon::new(icon).large().text_color(cx.theme().muted_foreground))
        .child(div().max_w(px(560.)).text_sm().text_color(cx.theme().muted_foreground).child(message))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    // Explicit imports: the `gpui_kit::*` glob would shadow `#[test]` with GPUI's test macro.
    use gpui_kit::base::Root;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        AppContext as _, Bounds, Entity, Point, ScrollStrategy, TestAppContext, WindowBounds, WindowHandle,
        WindowOptions, px, size,
    };
    use profile_core::PresetKind;

    use super::{Catalog, LoadState, Node, StudioApp, nav};

    fn profiles_dir() -> Option<PathBuf> {
        // Tests run from the package directory; a relative `ORCA_PROFILES_DIR` means the workspace root.
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        std::env::var_os("ORCA_PROFILES_DIR").map(|dir| workspace.join(dir)).or_else(|| {
            let sibling = workspace.join("../OrcaSlicer/resources/profiles");
            sibling.is_dir().then_some(sibling)
        })
    }

    fn open(cx: &mut TestAppContext, dir: PathBuf) -> (WindowHandle<Root>, Entity<StudioApp>) {
        cx.update(gpui_kit::init);
        cx.update(crate::values::init);
        cx.update(|cx| {
            let bounds = Bounds { origin: Point::default(), size: size(px(1200.), px(900.)) };
            let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
            let (window, app) =
                gpui_kit::open_window(options, cx, |_, cx| cx.new(|cx| StudioApp::new(Some(dir), cx))).unwrap();
            (window.downcast::<Root>().unwrap(), app)
        })
    }

    fn catalog(app: &Entity<StudioApp>, cx: &mut TestAppContext) -> Arc<Catalog> {
        app.read_with(cx, |app, _| match &app.state {
            LoadState::Ready(catalog) => catalog.clone(),
            _ => panic!("profiles are not loaded"),
        })
    }

    /// Scrolls the virtualized tree to `node`, clicks its row and returns the row index.
    fn click_node(window: WindowHandle<Root>, app: &Entity<StudioApp>, node: Node, cx: &mut TestAppContext) -> usize {
        let tree = app.read_with(cx, |app, _| app.tree.clone());
        let ix = tree.read_with(cx, |tree, _| tree.index_of(&node.id())).expect("node is visible in the tree");
        tree.update(cx, |tree, _| tree.scroll_to_item(ix, ScrollStrategy::Top));
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(ix, cx);
        })
        .unwrap();
        ix
    }

    #[gpui_kit::test]
    fn loads_in_background_and_drills_down_to_a_printer(cx: &mut TestAppContext) {
        let Some(dir) = profiles_dir() else { return };
        let (window, app) = open(cx, dir);
        assert!(app.read_with(cx, |app, _| matches!(app.state, LoadState::Loading(_))));

        cx.run_until_parked();
        let catalog = catalog(&app, cx);
        let repo = &catalog.repo;
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let summary = window.find("status-summary").label().map(str::to_owned).unwrap_or_default();
            assert!(summary.starts_with("57 vendors · 326 machine models · 7095 presets"), "{summary}");
        })
        .unwrap();

        let vendor = repo.vendors().iter().position(|vendor| vendor.name == "Snapmaker").unwrap();
        let vendor_row = click_node(window, &app, Node::Vendor(vendor), cx);
        assert_eq!(app.read_with(cx, |app, _| app.selected), Some(Node::Vendor(vendor)));
        cx.update_window(window.into(), |_, window, _| {
            assert_eq!(window.find(vendor_row).expanded(), Some(true));
            assert_eq!(window.find(vendor_row + 1).label(), Some("Machine models"));
            assert_eq!(window.find("details-title").label(), Some("Snapmaker"));
        })
        .unwrap();

        click_node(window, &app, Node::Group(vendor, nav::Group::Models), cx);
        let model = repo
            .models()
            .iter()
            .position(|model| model.vendor.index() == vendor && model.id == "Snapmaker U1")
            .unwrap();
        click_node(window, &app, Node::Model(model), cx);
        assert_eq!(app.read_with(cx, |app, _| app.selected), Some(Node::Model(model)));

        let &[printer] = repo.find(PresetKind::Machine, "Snapmaker U1 (0.4 nozzle)") else {
            panic!("one U1 0.4 printer")
        };
        let printer_row = click_node(window, &app, Node::Preset(printer.index()), cx);
        assert_eq!(app.read_with(cx, |app, _| app.selected), Some(Node::Preset(printer.index())));
        cx.update_window(window.into(), |_, window, _| {
            assert_eq!(window.find(printer_row).selected(), Some(true));
            assert_eq!(window.find("details-title").label(), Some("Snapmaker U1 (0.4 nozzle)"));
            // Highlighted values are labelled with the schema's option labels.
            assert_eq!(window.find("value-nozzle_diameter").label(), Some("Nozzle diameter"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn values_tab_traces_each_key_to_its_layer(cx: &mut TestAppContext) {
        let Some(dir) = profiles_dir() else { return };
        let (window, app) = open(cx, dir);
        cx.run_until_parked();
        let catalog = catalog(&app, cx);
        let repo = &catalog.repo;
        let vendor = repo.vendors().iter().position(|vendor| vendor.name == "Snapmaker").unwrap();
        let &[abs] = repo.find(PresetKind::Filament, "Generic ABS @U1 0.2 nozzle") else {
            panic!("one Generic ABS @U1 0.2 nozzle")
        };
        click_node(window, &app, Node::Vendor(vendor), cx);
        click_node(window, &app, Node::Group(vendor, nav::Group::Filaments), cx);
        click_node(window, &app, Node::Preset(abs.index()), cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.within("preset-tabs").click(1usize, cx);
        })
        .unwrap();
        let values = app.read_with(cx, |app, _| app.values.clone()).expect("the Values tab creates its view");
        let dump = |cx: &mut TestAppContext| values.read_with(cx, |values, cx| values.table().read(cx).dump(cx).1);
        assert_eq!(dump(cx).len(), repo.resolve(abs).len());

        cx.update_window(window.into(), |_, window, cx| {
            window.click("values-search", cx);
            window.input("nozzle_temperature", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let rows = dump(cx);
        let keys: Vec<&str> = rows.iter().map(|row| row[2].as_str()).collect();
        assert!(keys.iter().all(|key| key.contains("nozzle_temperature")), "{keys:?}");
        let position = keys.iter().position(|&key| key == "nozzle_temperature").expect("the key is listed");
        assert_eq!(rows[position][4], "this file");

        cx.update_window(window.into(), |_, window, cx| window.click(("row", position), cx)).unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("value-detail").label(), Some("nozzle_temperature"));
            assert_eq!(window.find(("layer", 0usize)).label(), Some("this file: 270 ℃"));
            // One line per layer that sets the key, ending with the schema default.
            let trace = repo.trace(abs).into_iter().find(|trace| trace.key == "nozzle_temperature").unwrap();
            assert!(trace.layers.len() >= 2, "the preset overrides at least the schema default");
            let last = window.find(("layer", trace.layers.len() - 1)).label().unwrap().to_owned();
            assert!(last.starts_with("default: "), "{last}");
            assert!(window.try_find(("layer", trace.layers.len())).is_none());
        })
        .unwrap();

        // A key set by no file in the chain disappears when defaults are hidden.
        cx.update_window(window.into(), |_, window, cx| {
            window.click("values-search", cx);
            window.press("cmd-a", cx);
            window.input("filament_tower_interface_print_temp", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(dump(cx).iter().map(|row| row[4].as_str()).collect::<Vec<_>>(), ["default"]);
        cx.update_window(window.into(), |_, window, cx| window.click("hide-defaults", cx)).unwrap();
        cx.run_until_parked();
        assert!(dump(cx).is_empty());
    }

    const CHILD: &str = "{\n    \"type\": \"process\",\n    \"name\": \"Acme 0.2\",\n    \"inherits\": \"fdm_process_common\",\n    \"from\": \"system\",\n    \"instantiation\": \"true\",\n    \"wall_loops\": \"2\",\n    \"enable_support\": \"0\"\n}";

    /// A vendor with one template and one selectable process, in a temporary directory.
    fn editable_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let write = |path: &str, text: &str| {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "Acme.json",
            r#"{ "name": "Acme", "version": "01.00.00.00", "process_list": [
                { "name": "fdm_process_common", "sub_path": "process/fdm_process_common.json" },
                { "name": "Acme 0.2", "sub_path": "process/Acme 0.2.json" } ] }"#,
        );
        write(
            "Acme/process/fdm_process_common.json",
            r#"{ "type": "process", "name": "fdm_process_common", "from": "system", "instantiation": "false",
                "sparse_infill_density": "15%" }"#,
        );
        write("Acme/process/Acme 0.2.json", CHILD);
        dir
    }

    /// Types `text` into the values search box, replacing what is there, and selects the first row.
    fn find_key(window: WindowHandle<Root>, text: &str, cx: &mut TestAppContext) {
        cx.update_window(window.into(), |_, window, cx| {
            window.click("values-search", cx);
            window.press("cmd-a", cx);
            window.input(text, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.click(("row", 0usize), cx)).unwrap();
        cx.run_until_parked();
    }

    fn click(window: WindowHandle<Root>, id: &'static str, cx: &mut TestAppContext) {
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    /// The first visible row as (key, value, source).
    fn first_row(app: &Entity<StudioApp>, cx: &mut TestAppContext) -> (String, String, String) {
        let values = app.read_with(cx, |app, _| app.values.clone()).unwrap();
        let rows = values.read_with(cx, |values, cx| values.table().read(cx).dump(cx).1);
        (rows[0][2].clone(), rows[0][3].clone(), rows[0][4].clone())
    }

    #[gpui_kit::test]
    fn edits_undoes_and_saves_values(cx: &mut TestAppContext) {
        let dir = editable_fixture();
        let file = dir.path().join("Acme/process/Acme 0.2.json");
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        let catalog = catalog(&app, cx);
        let &[child] = catalog.repo.find(PresetKind::Process, "Acme 0.2") else { panic!("one child preset") };
        click_node(window, &app, Node::Vendor(0), cx);
        click_node(window, &app, Node::Group(0, nav::Group::Processes), cx);
        click_node(window, &app, Node::Preset(child.index()), cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.within("preset-tabs").click(1usize, cx);
        })
        .unwrap();
        cx.run_until_parked();

        // Edit a number through the text input.
        find_key(window, "wall_loops", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("value-input").value(), Some("2"));
            window.click("value-input", cx);
            window.press("cmd-a", cx);
            window.input("3", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(first_row(&app, cx), ("wall_loops".into(), "3".into(), "this file (edited)".into()));
        let dirty = |app: &Entity<StudioApp>, cx: &mut TestAppContext| {
            app.read_with(cx, |app, cx| app.edits.as_ref().unwrap().read(cx).is_dirty(child.index()))
        };
        assert!(dirty(&app, cx));

        click(window, "undo", cx);
        assert_eq!(first_row(&app, cx), ("wall_loops".into(), "2".into(), "this file".into()));
        assert!(!dirty(&app, cx));
        click(window, "redo", cx);
        assert_eq!(first_row(&app, cx).1, "3");

        // Toggle a bool, and override a value the template sets.
        find_key(window, "enable_support", cx);
        click(window, "value-bool", cx);
        assert_eq!(first_row(&app, cx).1, "1");
        find_key(window, "sparse_infill_density", cx);
        assert_eq!(first_row(&app, cx).2, "[1] fdm_process_common");
        click(window, "override", cx);
        assert_eq!(first_row(&app, cx).2, "this file (edited)");

        // Saving asks first, then writes only the changed lines.
        click(window, "save", cx);
        cx.update_window(window.into(), |_, window, _| {
            assert_eq!(window.find("value-detail").label(), Some("save-confirmation"));
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), CHILD, "nothing is written before confirming");
        click(window, "confirm-save", cx);
        let expected = CHILD
            .replace("\"wall_loops\": \"2\"", "\"wall_loops\": \"3\"")
            .replace("\"enable_support\": \"0\"", "\"enable_support\": \"1\",\n    \"sparse_infill_density\": \"15%\"");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), expected);
        assert!(!dirty(&app, cx));
        cx.update_window(window.into(), |_, window, _| {
            let status = window.find("save-status").label().unwrap_or_default().to_owned();
            assert!(status.starts_with("Saved 1 file(s)"), "{status}");
        })
        .unwrap();

        // Reloading with unsaved edits asks for a second click.
        find_key(window, "wall_loops", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("value-input", cx);
            window.press("cmd-a", cx);
            window.input("4", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        click(window, "reload", cx);
        assert!(app.read_with(cx, |app, _| matches!(app.state, LoadState::Ready(_))));
        cx.update_window(window.into(), |_, window, _| assert!(window.try_find("discard-warning").is_some())).unwrap();
        click(window, "reload", cx);
        assert!(app.read_with(cx, |app, cx| app.edits.as_ref().is_some_and(|edits| edits.read(cx).dirty().is_empty())));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), expected, "discarded edits are not written");
    }

    #[gpui_kit::test]
    fn reports_a_directory_that_cannot_be_loaded(cx: &mut TestAppContext) {
        let missing = std::env::temp_dir().join("profile-studio-missing-profiles");
        let (_, app) = open(cx, missing);
        cx.run_until_parked();
        assert!(app.read_with(cx, |app, _| matches!(app.state, LoadState::Failed(..))));
    }
}
