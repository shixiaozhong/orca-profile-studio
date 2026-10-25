//! The main window: toolbar, vendor tree, details pane and status bar.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::command::{Command, CommandGroup, CommandState};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::resizable::{ResizablePanelEvent, ResizableState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tree::{TreeEntry, TreeItem, TreeState, tree};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _, h_flex, h_resizable,
    resizable_panel, v_flex, v_resizable,
};
use gpui_kit::component::{IndexPath, Theme, WindowExt as _};
use gpui_kit::prelude::*;
use gpui_kit::*;
use profile_core::{Repository, Schema, Severity};

use crate::catalog::Catalog;
use crate::details::{self, Compatibility, severity_color};
use crate::edits::Edits;
use crate::issues::{IssuesEvent, IssuesView};
use crate::palette::{self, AppCommand, Target};
use crate::settings::{self, ThemePreference};
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

actions!(studio, [OpenPalette, NewWindow]);

#[derive(Default)]
struct PaletteEntries {
    groups: Vec<CommandGroup>,
    targets: Vec<Vec<Target>>,
}

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("cmd-k", OpenPalette, None), KeyBinding::new("cmd-n", NewWindow, None)]);
    // Each window loads the directory on its own; a new one opens the directory loaded last.
    cx.on_action(|_: &NewWindow, cx| open_window(settings::get(cx).profiles_dir.clone(), cx));
}

/// Opens a studio window, sized as the last window was when it closed.
pub fn open_window(profiles_dir: Option<PathBuf>, cx: &mut App) {
    let (width, height) = settings::get(cx).window_size.unwrap_or((1280., 820.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions { title: Some("Orca Profile Studio".into()), ..Default::default() }),
        ..Default::default()
    };
    if let Err(err) =
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| StudioApp::new(profiles_dir, window, cx)))
    {
        eprintln!("cannot open a window: {err}");
    }
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
    /// The command palette's state; created when first opened, shown in a `Root` dialog.
    palette: Option<Entity<CommandState>>,
    /// The palette's groups and what each row targets, by section and row; rebuilt when the
    /// palette opens and when its query changes, never during render.
    palette_entries: PaletteEntries,
    /// The selected preset's compatibility, computed when it is selected.
    compatibility: Option<Compatibility>,
    /// The issues panel; created when first opened.
    issues: Option<Entity<IssuesView>>,
    issues_open: bool,
    _issues_events: Option<Subscription>,
    /// The window's default focus target, so app shortcuts such as ⌘K always have a dispatch path.
    focus: FocusHandle,
    _focus_lost: Subscription,
    /// The tree / details split and the browser / issues split; their sizes are saved.
    browser_layout: Entity<ResizableState>,
    workspace_layout: Entity<ResizableState>,
    load_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl StudioApp {
    pub fn new(profiles_dir: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let tree_observer = cx.observe_in(&tree, window, |this, tree, window, cx| {
            let selected = tree.read(cx).selected_item().and_then(|item| Node::parse(&item.id));
            if selected != this.selected {
                this.selected = selected;
                this.sync_values(window, cx);
                this.compatibility = match (&this.state, selected) {
                    (LoadState::Ready(catalog), Some(Node::Preset(preset))) => {
                        Some(Compatibility::compute(catalog, preset))
                    }
                    _ => None,
                };
                cx.notify();
            }
        });
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        // When the focused element goes away (a closed view, a reload), shortcuts need a new home.
        let focus_lost = cx.on_focus_lost(window, |this, window, cx| this.focus.focus(window, cx));
        let browser_layout = cx.new(|_| ResizableState::default());
        let workspace_layout = cx.new(|_| ResizableState::default());
        let subscriptions = vec![
            tree_observer,
            cx.subscribe(&browser_layout, |_, layout, _: &ResizablePanelEvent, cx| {
                if let Some(&width) = layout.read(cx).sizes().first() {
                    settings::update(cx, |settings| settings.sidebar_width = Some(width.into()));
                }
            }),
            cx.subscribe(&workspace_layout, |this, layout, _: &ResizablePanelEvent, cx| {
                if this.issues_open
                    && let Some(&height) = layout.read(cx).sizes().get(1)
                {
                    settings::update(cx, |settings| settings.issues_height = Some(height.into()));
                }
            }),
            // The System theme follows the appearance while the app runs.
            cx.observe_window_appearance(window, |_, window, cx| {
                if settings::get(cx).theme == ThemePreference::System {
                    Theme::sync_system_appearance(Some(window), cx);
                }
            }),
        ];
        window.on_window_should_close(cx, |window, cx| {
            // The bounds to reopen the window with, the same kind `WindowOptions` takes.
            let size = window.window_bounds().get_bounds().size;
            settings::update(cx, |settings| settings.window_size = Some((size.width.into(), size.height.into())));
            true
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
            palette: None,
            palette_entries: PaletteEntries::default(),
            compatibility: None,
            issues: None,
            issues_open: false,
            _issues_events: None,
            focus: focus.clone(),
            _focus_lost: focus_lost,
            browser_layout,
            workspace_layout,
            load_task: None,
            _subscriptions: subscriptions,
        };
        if let Some(dir) = profiles_dir {
            app.load(dir, window, cx);
        }
        app
    }

    /// Loads `dir` on the background executor; a newer request supersedes a running one.
    fn load(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // Absolute and without `..`, so the saved setting does not depend on where the app started.
        let dir = dir.canonicalize().or_else(|_| std::path::absolute(&dir)).unwrap_or(dir);
        self.profiles_dir = Some(dir.clone());
        self.state = LoadState::Loading(dir.clone());
        self.selected = None;
        self.values = None;
        self.edits = None;
        self.confirm_discard = false;
        self.palette_entries = PaletteEntries::default();
        self.compatibility = None;
        self.issues = None;
        self.issues_open = false;
        self._issues_events = None;
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
        self.load_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            this.update_in(cx, |this, window, cx| this.finish_load(dir, result, window, cx)).ok();
        }));
        cx.notify();
    }

    fn finish_load(
        &mut self,
        dir: PathBuf,
        result: Result<Catalog, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                settings::update(cx, |settings| settings.profiles_dir = Some(dir.clone()));
                LoadState::Ready(catalog)
            }
            Err(err) => LoadState::Failed(dir, err),
        };
        self.load_task = None;
        if matches!(self.state, LoadState::Ready(_)) && settings::get(cx).issues_open {
            self.open_issues(window, cx);
        }
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

    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.may_discard(cx) {
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = picked.await
                && let Some(dir) = paths.pop()
            {
                this.update_in(cx, |this, window, cx| this.load(dir, window, cx)).ok();
            }
        })
        .detach();
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.may_discard(cx) {
            return;
        }
        if let Some(dir) = self.profiles_dir.clone() {
            self.load(dir, window, cx);
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
                Button::new("open-palette")
                    .small()
                    .icon(IconName::Search)
                    .label("Search…")
                    .tooltip("Search presets, options and values; run commands (⌘K)")
                    .disabled(!matches!(self.state, LoadState::Ready(_)))
                    .on_click(cx.listener(|this, _, window, cx| this.open_palette(window, cx))),
            )
            .child(
                Button::new("toggle-issues")
                    .small()
                    .icon(IconName::TriangleAlert)
                    .label("Issues")
                    .selected(self.issues_open)
                    .disabled(!matches!(self.state, LoadState::Ready(_)))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_issues(window, cx))),
            )
            .child(
                Button::new("open")
                    .small()
                    .icon(IconName::FolderOpen)
                    .label("Open…")
                    .on_click(cx.listener(|this, _, window, cx| this.choose_folder(window, cx))),
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
                    .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
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
            LoadState::Ready(catalog) => {
                let catalog = catalog.clone();
                self.render_browser(&catalog, cx)
            }
        }
    }

    fn render_browser(&self, catalog: &Arc<Catalog>, cx: &mut Context<Self>) -> AnyElement {
        let rows = catalog.clone();
        let edits = self.edits.clone();
        let navigation = tree(&self.tree, move |ix, entry, _, _, cx| {
            let dirty = |preset| edits.as_ref().is_some_and(|edits| edits.read(cx).is_dirty(preset));
            render_row(&rows, ix, entry, &dirty, cx)
        })
        .size_full();
        let scrolling = |content: AnyElement| div().id("details").size_full().overflow_y_scroll().child(content);
        let details = match self.selected {
            Some(Node::Preset(index)) => self.render_preset(catalog, index, cx),
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
        let layout = settings::get(cx);
        let (sidebar_width, issues_height) =
            (layout.sidebar_width.unwrap_or(380.), layout.issues_height.unwrap_or(280.));
        let browser = h_resizable("browser")
            .with_state(&self.browser_layout)
            .child(
                resizable_panel()
                    .size(px(sidebar_width))
                    .size_range(px(240.)..px(720.))
                    .child(div().size_full().py_1().child(navigation)),
            )
            .child(resizable_panel().child(div().size_full().p_4().child(details)));
        let issues = self.issues.clone().filter(|_| self.issues_open);
        v_resizable("workspace")
            .with_state(&self.workspace_layout)
            .child(resizable_panel().child(browser))
            .when_some(issues, |workspace, issues| {
                workspace.child(
                    resizable_panel().size(px(issues_height)).size_range(px(140.)..px(720.)).child(
                        div().size_full().px_4().pb_2().border_t_1().border_color(cx.theme().border).child(issues),
                    ),
                )
            })
            .into_any_element()
    }

    /// A preset's details: the overview, or the values table with every key and its layers.
    fn render_preset(&self, catalog: &Arc<Catalog>, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let tabs = TabBar::new("preset-tabs")
            .underline()
            .small()
            .selected_index(self.preset_tab as usize)
            .child(Tab::new().label("Overview"))
            .child(Tab::new().label("Values"))
            .on_click(cx.listener(|this, ix: &usize, window, cx| {
                let tab = if *ix == 1 { PresetTab::Values } else { PresetTab::Overview };
                this.set_preset_tab(tab, window, cx);
            }));
        let content = match self.preset_tab {
            PresetTab::Overview => div()
                .id("details")
                .size_full()
                .overflow_y_scroll()
                .child(details::render_preset(catalog, index, self.compatibility.as_ref(), cx))
                .into_any_element(),
            // The view is created by the handlers that select a preset or the tab (`sync_values`).
            PresetTab::Values => div()
                .size_full()
                .children(self.values.clone().filter(|values| values.read(cx).preset() == index))
                .into_any_element(),
        };
        v_flex().size_full().child(tabs).child(div().flex_1().min_h_0().pt_3().child(content)).into_any_element()
    }

    /// The Values view of `preset`, created on first use; it needs a `Window` for its inputs.
    fn ensure_values(
        &mut self,
        preset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ValuesView>> {
        if let Some(values) = self.values.as_ref().filter(|values| values.read(cx).preset() == preset) {
            return Some(values.clone());
        }
        let (LoadState::Ready(catalog), Some(edits)) = (&self.state, &self.edits) else { return None };
        let (catalog, edits) = (catalog.clone(), edits.clone());
        let values = cx.new(|cx| ValuesView::new(&catalog, &edits, preset, window, cx));
        self.values = Some(values.clone());
        Some(values)
    }

    /// Creates the Values view when a preset is selected and the Values tab is showing.
    fn sync_values(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let (Some(Node::Preset(preset)), PresetTab::Values) = (self.selected, self.preset_tab) {
            self.ensure_values(preset, window, cx);
        }
    }

    fn set_preset_tab(&mut self, tab: PresetTab, window: &mut Window, cx: &mut Context<Self>) {
        self.preset_tab = tab;
        self.sync_values(window, cx);
        cx.notify();
    }

    /// Rebuilds the palette's groups and targets for `query` and the current selection.
    fn refresh_palette(&mut self, query: &str, cx: &mut Context<Self>) {
        let LoadState::Ready(catalog) = &self.state else { return };
        let preset_selected = matches!(self.selected, Some(Node::Preset(_)));
        let (groups, targets) = palette::build(catalog, query, preset_selected, settings::get(cx).theme);
        self.palette_entries = PaletteEntries { groups, targets };
        cx.notify();
    }

    /// Opens the palette in a `Root` dialog, which traps focus, closes on Escape or a click
    /// outside and restores the previous focus.
    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.state, LoadState::Ready(_)) || window.has_active_dialog(cx) {
            return;
        }
        let palette = self.palette.get_or_insert_with(|| cx.new(|cx| CommandState::new(window, cx))).clone();
        palette.update(cx, |palette, cx| palette.set_query("", window, cx));
        self.refresh_palette("", cx);
        let app = cx.entity();
        let initial_focus = Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            // The builder may run again when the dialog layer is rebuilt; focus only once.
            if initial_focus.replace(false) {
                palette.update(cx, |palette, cx| palette.focus(window, cx));
            }
            let groups = app.read(cx).palette_entries.groups.clone();
            let command = groups
                .into_iter()
                .fold(Command::new(&palette).filterable(false), |command, group| command.group(group))
                .placeholder("Search presets, options and values, or type a command…")
                .max_h(rems(27.5))
                .on_query({
                    let app = app.clone();
                    move |query, _, cx| app.update(cx, |this, cx| this.refresh_palette(query, cx))
                })
                .on_confirm({
                    let app = app.clone();
                    move |index, window, cx| {
                        window.close_dialog(cx);
                        app.update(cx, |this, cx| this.confirm_palette(index, window, cx));
                    }
                });
            // Dialog sizes take Pixels.
            dialog.w(px(680.)).close_button(false).margin_top(px(64.)).child(command)
        });
    }

    fn confirm_palette(&mut self, index: IndexPath, window: &mut Window, cx: &mut Context<Self>) {
        let targets = &self.palette_entries.targets;
        let Some(target) = targets.get(index.section).and_then(|row| row.get(index.row)).cloned() else {
            return;
        };
        let reveal = |this: &mut Self, preset: usize, key: &str, window: &mut Window, cx: &mut Context<Self>| {
            this.preset_tab = PresetTab::Values;
            if let Some(values) = this.ensure_values(preset, window, cx) {
                values.update(cx, |values, cx| values.reveal_key(key, window, cx));
            }
        };
        match target {
            Target::Command(AppCommand::Reload) => self.reload(window, cx),
            Target::Command(AppCommand::Open) => self.choose_folder(window, cx),
            Target::Command(AppCommand::Theme(theme)) => settings::set_theme(theme, window, cx),
            Target::Command(AppCommand::NewWindow) => window.dispatch_action(Box::new(NewWindow), cx),
            Target::Command(AppCommand::ShowOverview) => self.set_preset_tab(PresetTab::Overview, window, cx),
            Target::Command(AppCommand::ShowValues) => self.set_preset_tab(PresetTab::Values, window, cx),
            Target::Command(AppCommand::ShowIssues) => self.open_issues(window, cx),
            Target::Preset(preset) => self.select_node(Node::Preset(preset), true, window, cx),
            Target::Option(key) => {
                if let Some(Node::Preset(preset)) = self.selected {
                    reveal(self, preset, &key, window, cx);
                }
            }
            Target::Value(preset, key) => {
                self.select_node(Node::Preset(preset), true, window, cx);
                reveal(self, preset, &key, window, cx);
            }
        }
        cx.notify();
    }

    /// Selects a preset in the tree, expanding its ancestors and scrolling it into view.
    /// Selects a node in the tree, expanding its ancestors and scrolling it into view; with
    /// `focus`, keyboard focus moves to the tree.
    fn select_node(&mut self, node: Node, focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        let id = node.id();
        self.tree.update(cx, |tree, cx| {
            tree.set_selected_item(Some(&TreeItem::new(id.clone(), "")), cx);
            tree.reveal_item(&id, ScrollStrategy::Center, cx);
            if focus {
                tree.focus(window, cx);
            }
        });
    }

    fn open_issues(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let LoadState::Ready(catalog) = &self.state else { return };
        if self.issues.is_none() {
            let catalog = catalog.clone();
            let issues = cx.new(|cx| IssuesView::new(&catalog, window, cx));
            self._issues_events = Some(cx.subscribe_in(&issues, window, |this, _, event, window, cx| match event {
                IssuesEvent::Reveal(index) => this.reveal_issue(*index, window, cx),
                IssuesEvent::Close => this.close_issues(cx),
            }));
            self.issues = Some(issues);
        }
        self.issues_open = true;
        settings::update(cx, |settings| settings.issues_open = true);
        cx.notify();
    }

    fn close_issues(&mut self, cx: &mut Context<Self>) {
        self.issues_open = false;
        settings::update(cx, |settings| settings.issues_open = false);
        cx.notify();
    }

    fn toggle_issues(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.issues_open {
            self.close_issues(cx);
        } else {
            self.open_issues(window, cx);
        }
    }

    /// Shows where an issue is: its preset, and the key in the Values tab when it names one.
    /// Keyboard focus stays in the issues list, so the arrow keys keep walking through issues.
    fn reveal_issue(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let LoadState::Ready(catalog) = &self.state else { return };
        let repo = &catalog.repo;
        let issue = &repo.issues()[index];
        // Some file-level issues are recorded before the preset gets an id.
        let preset = issue
            .preset
            .map(|id| id.index())
            .or_else(|| repo.presets().iter().position(|preset| preset.path == issue.path));
        let key = issue.kind.key().map(str::to_owned);
        let vendor = issue.vendor.index();
        match preset {
            Some(preset) => {
                self.select_node(Node::Preset(preset), false, window, cx);
                if let Some(key) = key {
                    self.preset_tab = PresetTab::Values;
                    if let Some(values) = self.ensure_values(preset, window, cx) {
                        values.update(cx, |values, cx| values.reveal_key(&key, window, cx));
                    }
                }
            }
            None => self.select_node(Node::Vendor(vendor), false, window, cx),
        }
        cx.notify();
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
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| this.open_palette(window, cx)))
            .child(self.render_toolbar(cx))
            .child(div().flex_1().min_h_0().child(self.render_body(cx)))
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

    ListItem::new(("tree-row", ix)).py_0p5().pl(rems(0.5 + 0.875 * entry.depth() as f32)).pr_2().child(
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
        .child(div().max_w(rems(35.)).text_sm().text_color(cx.theme().muted_foreground).child(message))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    // Explicit imports: the `gpui_kit::*` glob would shadow `#[test]` with GPUI's test macro.
    use gpui_kit::base::Root;
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{
        AppContext as _, Bounds, Entity, Point, ScrollStrategy, TestAppContext, WindowBounds, WindowHandle,
        WindowOptions, px, size,
    };
    use profile_core::{PresetKind, Severity};

    use super::{Catalog, LoadState, Node, PresetTab, StudioApp, ThemePreference, nav};
    use gpui_kit::component::ActiveTheme as _;

    fn profiles_dir() -> Option<PathBuf> {
        // Tests run from the package directory; a relative `ORCA_PROFILES_DIR` means the workspace root.
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        std::env::var_os("ORCA_PROFILES_DIR").map(|dir| workspace.join(dir)).or_else(|| {
            let sibling = workspace.join("../OrcaSlicer/resources/profiles");
            sibling.is_dir().then_some(sibling)
        })
    }

    fn open(cx: &mut TestAppContext, dir: PathBuf) -> (WindowHandle<Root>, Entity<StudioApp>) {
        open_with_settings(cx, dir, None)
    }

    /// Opens a window; `settings` is the settings file, `None` keeps settings in memory only.
    fn open_with_settings(
        cx: &mut TestAppContext,
        dir: PathBuf,
        settings: Option<PathBuf>,
    ) -> (WindowHandle<Root>, Entity<StudioApp>) {
        cx.update(gpui_kit::init);
        cx.update(|cx| crate::settings::init(settings, cx));
        cx.update(crate::values::init);
        cx.update(super::init);
        cx.update(|cx| {
            let bounds = Bounds { origin: Point::default(), size: size(px(1200.), px(900.)) };
            let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..Default::default() };
            let (window, app) =
                gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| StudioApp::new(Some(dir), window, cx)))
                    .unwrap();
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

    /// Opens the palette with ⌘K, types `query` and confirms the highlighted entry.
    fn run_palette(window: WindowHandle<Root>, query: &str, cx: &mut TestAppContext) {
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("cmd-k", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.input(query, cx)).unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn palette_searches_and_navigates(cx: &mut TestAppContext) {
        let dir = editable_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        let loaded = catalog(&app, cx);
        let child = loaded.repo.find(PresetKind::Process, "Acme 0.2")[0];

        // A preset: selected and revealed in the collapsed tree.
        run_palette(window, "acme 0.2", cx);
        let dialog_open = |cx: &mut TestAppContext| {
            cx.update_window(window.into(), |_, window, cx| window.has_active_dialog(cx)).unwrap()
        };
        assert!(!dialog_open(cx), "confirming closes the palette");
        app.read_with(cx, |app, cx| {
            assert_eq!(app.selected, Some(Node::Preset(child.index())));
            assert!(app.tree.read(cx).index_of(&Node::Preset(child.index()).id()).is_some());
        });

        // A value set in a file: its preset's Values tab, filtered to the key.
        run_palette(window, "wall_loops = 2", cx);
        let values = app.read_with(cx, |app, _| {
            assert_eq!(app.preset_tab, PresetTab::Values);
            app.values.clone().unwrap()
        });
        let rows = |cx: &mut TestAppContext| values.read_with(cx, |values, cx| values.table().read(cx).dump(cx).1);
        assert_eq!(rows(cx).iter().map(|row| row[2].as_str()).collect::<Vec<_>>(), ["wall_loops"]);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("value-detail").label(), Some("wall_loops"));
        })
        .unwrap();

        // An option, shown in the selected preset.
        run_palette(window, "enable_support", cx);
        assert_eq!(rows(cx)[0][2], "enable_support");

        // Escape closes the palette without doing anything.
        cx.update_window(window.into(), |_, window, cx| window.press("cmd-k", cx)).unwrap();
        cx.run_until_parked();
        assert!(dialog_open(cx));
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(!dialog_open(cx));

        // A command.
        run_palette(window, "reload", cx);
        assert!(!Arc::ptr_eq(&catalog(&app, cx), &loaded), "profiles were loaded again");
    }

    #[gpui_kit::test]
    fn issues_panel_filters_and_reveals_issues(cx: &mut TestAppContext) {
        let Some(dir) = profiles_dir() else { return };
        let (window, app) = open(cx, dir);
        cx.run_until_parked();
        let loaded = catalog(&app, cx);
        let repo = &loaded.repo;

        click(window, "toggle-issues", cx);
        let issues = app.read_with(cx, |app, _| app.issues.clone()).expect("the panel is created when opened");
        let rows = |cx: &mut TestAppContext| issues.read_with(cx, |issues, cx| issues.table().read(cx).dump(cx).1);
        // Warnings and above by default.
        let expected = repo.issues().iter().filter(|issue| issue.severity() >= Severity::Warning).count();
        assert_eq!(rows(cx).len(), expected);

        // Pick a code through its searchable select.
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.within("issues-code").click("input", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.input("item-unparsable", cx)).unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("down", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let filtered = rows(cx);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0][1], "item-unparsable");

        // Selecting the row shows the preset and the key, without leaving the list.
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.within("issues-list").click(("row", 0usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        let asa = repo.find(PresetKind::Filament, "Bambu ASA @BBL X1 0.6 nozzle")[0];
        app.read_with(cx, |app, _| {
            assert_eq!(app.selected, Some(Node::Preset(asa.index())));
            assert_eq!(app.preset_tab, PresetTab::Values);
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("value-detail").label(), Some("textured_plate_temp_initial_layer"));
        })
        .unwrap();

        click(window, "close-issues", cx);
        assert!(!app.read_with(cx, |app, _| app.issues_open));
    }

    #[gpui_kit::test]
    fn overview_shows_compatibility(cx: &mut TestAppContext) {
        let Some(dir) = profiles_dir() else { return };
        let (window, app) = open(cx, dir);
        cx.run_until_parked();
        // A process whose condition selects one printer.
        run_palette(window, "0.07mm DETAIL @CORE One 0.25", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("compat-summary").label(), Some("1 compatible printer"));
        })
        .unwrap();
        let printer = app.read_with(cx, |app, _| app.compatibility.as_ref().unwrap().printers.clone());
        assert_eq!(printer, ["Prusa CORE One 0.25 nozzle"]);
    }

    #[gpui_kit::test]
    fn theme_commands_switch_colors(cx: &mut TestAppContext) {
        let dir = editable_fixture();
        let (window, _) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        let colors = |cx: &mut TestAppContext| cx.update(|cx| (cx.theme().is_dark(), cx.theme().background));

        run_palette(window, "theme: light", cx);
        let (dark, light_background) = colors(cx);
        assert!(!dark);
        run_palette(window, "theme: dark", cx);
        let (dark, dark_background) = colors(cx);
        assert!(dark);
        assert_ne!(dark_background, light_background, "the window is painted with the dark theme");
        cx.update(|cx| assert_eq!(crate::settings::get(cx).theme, ThemePreference::Dark));
    }

    /// `path` written relative to the current directory.
    fn pathdiff_relative(path: &Path) -> PathBuf {
        let current = std::env::current_dir().unwrap();
        let depth = current.components().count() - 1;
        let up: PathBuf = std::iter::repeat_n("..", depth).collect();
        up.join(path.strip_prefix("/").unwrap())
    }

    #[gpui_kit::test]
    fn layout_is_restored_and_saved(cx: &mut TestAppContext) {
        let dir = editable_fixture();
        let settings_dir = tempfile::tempdir().unwrap();
        let path = settings_dir.path().join("settings.json");
        let saved = crate::settings::Settings {
            sidebar_width: Some(300.),
            issues_open: true,
            issues_height: Some(200.),
            ..Default::default()
        };
        saved.save(&path).unwrap();

        // A relative directory is saved as an absolute path.
        let relative = pathdiff_relative(dir.path());
        let (window, app) = open_with_settings(cx, relative, Some(path.clone()));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.render_frame(cx)).unwrap();
        app.read_with(cx, |app, cx| {
            assert!(app.issues_open, "the issues panel reopens");
            assert_eq!(app.browser_layout.read(cx).sizes().first().copied(), Some(px(300.)));
            assert_eq!(app.workspace_layout.read(cx).sizes().get(1).copied(), Some(px(200.)));
        });

        // Resizing (as a drag would), closing the panel and loading a directory are remembered.
        let (browser, workspace) =
            app.read_with(cx, |app, _| (app.browser_layout.clone(), app.workspace_layout.clone()));
        cx.update_window(window.into(), |_, window, cx| {
            browser.update(cx, |layout, cx| layout.resize_panel(0, px(320.), window, cx));
            workspace.update(cx, |layout, cx| layout.resize_panel(1, px(240.), window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        click(window, "toggle-issues", cx);
        let stored = crate::settings::Settings::load(&path);
        assert!(!stored.issues_open);
        assert_eq!(stored.profiles_dir, Some(dir.path().canonicalize().unwrap()));
        assert_eq!(stored.sidebar_width, Some(320.));
        assert_eq!(stored.issues_height, Some(240.));
    }

    #[gpui_kit::test]
    fn new_window_opens_another_window(cx: &mut TestAppContext) {
        let dir = editable_fixture();
        let (window, _) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "new window", cx);
        assert_eq!(cx.update(|cx| cx.windows().len()), 2);
    }

    #[gpui_kit::test]
    fn reports_a_directory_that_cannot_be_loaded(cx: &mut TestAppContext) {
        let missing = std::env::temp_dir().join("profile-studio-missing-profiles");
        let (_, app) = open(cx, missing);
        cx.run_until_parked();
        assert!(app.read_with(cx, |app, _| matches!(app.state, LoadState::Failed(..))));
    }

    fn choose_comparison(window: WindowHandle<Root>, query: &str, cx: &mut TestAppContext) {
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.within("compare-picker").click("input", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.input(query, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("down", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn compares_presets_with_edits_filters_and_sources(cx: &mut TestAppContext) {
        let dir = editable_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        let catalog = catalog(&app, cx);
        let child = catalog.repo.find(PresetKind::Process, "Acme 0.2")[0];
        let base = catalog.repo.find(PresetKind::Process, "fdm_process_common")[0];
        click_node(window, &app, Node::Vendor(0), cx);
        click_node(window, &app, Node::Group(0, nav::Group::Processes), cx);
        click_node(window, &app, Node::Preset(child.index()), cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.within("preset-tabs").click(1usize, cx);
        })
        .unwrap();
        cx.run_until_parked();
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
        click(window, "compare-with", cx);
        choose_comparison(window, "fdm_process_common", cx);
        let values = app.read_with(cx, |app, _| app.values.clone()).unwrap();
        let compare = values.read_with(cx, |values, _| values.comparison().clone());
        let dump = |cx: &mut TestAppContext| compare.read_with(cx, |view, cx| view.table().read(cx).dump(cx).1);
        let rows = dump(cx);
        let wall = rows.iter().find(|row| row[2] == "wall_loops").expect("edited wall count differs");
        assert_eq!(
            (wall[3].as_str(), wall[4].as_str(), wall[5].as_str(), wall[6].as_str()),
            ("4", "2", "this file", "default")
        );
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("compare-left-title").label().unwrap().contains("unsaved changes"));
            window.click("compare-search", cx);
            window.input("wall_loops", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(dump(cx).len(), 1);
        cx.update_window(window.into(), |_, window, cx| window.click(("row", 0usize), cx)).unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("compare-detail").label(), Some("wall_loops"));
            assert_eq!(window.find(("Left", 0usize)).label(), Some("this file: 4"));
            assert_eq!(window.find(("Right", 0usize)).label(), Some("default: 2"));
        })
        .unwrap();
        // Editing the other side refreshes an open comparison, including the empty-results case.
        let edits = app.read_with(cx, |app, _| app.edits.clone()).unwrap();
        edits.update(cx, |edits, cx| {
            edits.session_mut(base.index()).set("wall_loops", Some(profile_core::RawValue::Str("4".into())));
            cx.notify();
        });
        cx.run_until_parked();
        assert!(dump(cx).is_empty());
        click(window, "only-differences", cx);
        assert_eq!(dump(cx).len(), 1);
        assert_eq!(dump(cx)[0][6], "this file");
        click(window, "close-comparison", cx);
        assert_eq!(first_row(&app, cx).1, "4", "returning preserves the edit and filter");
        assert_eq!(std::fs::read_to_string(dir.path().join("Acme/process/Acme 0.2.json")).unwrap(), CHILD);
    }

    #[gpui_kit::test]
    fn compares_real_abs_nozzle_variants(cx: &mut TestAppContext) {
        let Some(dir) = profiles_dir() else { return };
        let (window, app) = open(cx, dir);
        cx.run_until_parked();
        let catalog = catalog(&app, cx);
        let repo = &catalog.repo;
        let left = repo.find(PresetKind::Filament, "Generic ABS @U1 0.2 nozzle")[0];
        let vendor = repo.preset(left).vendor.index();
        click_node(window, &app, Node::Vendor(vendor), cx);
        click_node(window, &app, Node::Group(vendor, nav::Group::Filaments), cx);
        click_node(window, &app, Node::Preset(left.index()), cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.within("preset-tabs").click(1usize, cx);
        })
        .unwrap();
        cx.run_until_parked();
        click(window, "compare-with", cx);
        choose_comparison(window, "Snapmaker / Generic ABS —", cx);
        let values = app.read_with(cx, |app, _| app.values.clone()).unwrap();
        let compare = values.read_with(cx, |values, _| values.comparison().clone());
        let rows = compare.read_with(cx, |view, cx| view.table().read(cx).dump(cx).1);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][2], "compatible_printers");
        assert!(rows[0][3].contains("0.2 nozzle"));
        assert!(rows[0][4].contains("0.4 nozzle"));
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("row", 0usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        click(window, "only-differences", cx);
        assert!(compare.read_with(cx, |view, cx| view.table().read(cx).dump(cx).1.len()) > 100);
        // Sorting keeps the selected key and its detail aligned, even when its row index changes.
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("icon-sort", 2usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        compare.read_with(cx, |view, cx| {
            let table = view.table().read(cx);
            let rows = table.dump(cx).1;
            assert!(rows.windows(2).all(|pair| pair[0][2].to_lowercase() >= pair[1][2].to_lowercase()));
            assert_eq!(rows[table.selected_row().unwrap()][2], "compatible_printers");
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("compare-detail").label(), Some("compatible_printers"));
        })
        .unwrap();
    }
}
