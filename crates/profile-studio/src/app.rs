//! The main window: toolbar, vendor tree, details pane and status bar.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::list::ListItem;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tree::{TreeEntry, TreeState, tree};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, h_resizable, resizable_panel,
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use profile_core::{Repository, Severity};

use crate::catalog::Catalog;
use crate::details::{self, severity_color};
use crate::tree::{self as nav, Node, kind_icon};

enum LoadState {
    /// No profiles directory chosen yet.
    Empty,
    Loading(PathBuf),
    Ready(Arc<Catalog>),
    Failed(PathBuf, String),
}

pub struct StudioApp {
    profiles_dir: Option<PathBuf>,
    state: LoadState,
    tree: Entity<TreeState>,
    selected: Option<Node>,
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
        self.tree.update(cx, |tree, cx| tree.set_items(Vec::new(), cx));

        let job = cx.background_executor().spawn({
            let dir = dir.clone();
            async move {
                let started = Instant::now();
                Repository::load(&dir).map(|repo| Catalog::new(repo, started.elapsed())).map_err(|err| err.to_string())
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
                LoadState::Ready(catalog)
            }
            Err(err) => LoadState::Failed(dir, err),
        };
        self.load_task = None;
        cx.notify();
    }

    fn choose_folder(&mut self, cx: &mut Context<Self>) {
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
            .child(
                Button::new("reload")
                    .small()
                    .icon(IconName::RefreshCw)
                    .tooltip("Reload from disk")
                    .disabled(self.profiles_dir.is_none() || matches!(self.state, LoadState::Loading(_)))
                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
            )
    }

    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
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
            LoadState::Ready(catalog) => self.render_browser(catalog, cx),
        }
    }

    fn render_browser(&self, catalog: &Arc<Catalog>, cx: &mut Context<Self>) -> AnyElement {
        let rows = catalog.clone();
        let navigation = tree(&self.tree, move |ix, entry, _, _, cx| render_row(&rows, ix, entry, cx)).size_full();
        let details = match self.selected {
            Some(node) => details::render(catalog, node, cx),
            None => div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Select a vendor, model or preset in the tree.")
                .into_any_element(),
        };
        h_resizable("browser")
            .child(
                resizable_panel()
                    .size(px(380.))
                    .size_range(px(240.)..px(720.))
                    .child(div().size_full().py_1().child(navigation)),
            )
            .child(resizable_panel().child(div().id("details").size_full().overflow_y_scroll().p_4().child(details)))
            .into_any_element()
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_toolbar(cx))
            .child(div().flex_1().min_h_0().child(self.render_body(cx)))
            .child(self.render_status(cx))
    }
}

fn render_row(catalog: &Catalog, ix: usize, entry: &TreeEntry, cx: &App) -> ListItem {
    let muted = cx.theme().muted_foreground;
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
        std::env::var_os("ORCA_PROFILES_DIR").map(PathBuf::from).or_else(|| {
            let sibling = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../OrcaSlicer/resources/profiles");
            sibling.is_dir().then_some(sibling)
        })
    }

    fn open(cx: &mut TestAppContext, dir: PathBuf) -> (WindowHandle<Root>, Entity<StudioApp>) {
        cx.update(gpui_kit::init);
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
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn reports_a_directory_that_cannot_be_loaded(cx: &mut TestAppContext) {
        let missing = std::env::temp_dir().join("profile-studio-missing-profiles");
        let (_, app) = open(cx, missing);
        cx.run_until_parked();
        assert!(app.read_with(cx, |app, _| matches!(app.state, LoadState::Failed(..))));
    }
}
