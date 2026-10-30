//! The main window: toolbar, vendor tree, details pane and status bar.

use std::cell::Cell;
use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant};
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
use profile_core::Severity;

use crate::batch::{BatchEvent, BatchView};
use crate::catalog::Catalog;
use crate::details::{self, Compatibility, severity_color};
use crate::edits::{Edits, EditsEvent, RefreshConflict};
use crate::issues::{IssuesEvent, IssuesView};
use crate::manage::{ManagementEvent, ManagementView};
use crate::models::ModelsView;
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
    crate::lifecycle::init(cx);
    cx.set_app_identity("org.orca-profile-studio.app", "Orca Profile Studio");
    cx.bind_keys([KeyBinding::new("secondary-k", OpenPalette, None), KeyBinding::new("secondary-n", NewWindow, None)]);
    // Each window loads the directory on its own; a new one opens the directory loaded last.
    cx.on_action(|_: &NewWindow, cx| open_window(settings::get(cx).profiles_dir.clone(), cx));
}

pub fn remember_window_size(window: &Window, cx: &mut App) {
    let size = window.window_bounds().get_bounds().size;
    settings::update(cx, |settings| settings.window_size = Some((size.width.into(), size.height.into())));
}

/// Opens a studio window, sized as the last window was when it closed.
pub fn open_window(profiles_dir: Option<PathBuf>, cx: &mut App) {
    let (width, height) = settings::get(cx).window_size.unwrap_or((1280., 820.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(960.), px(720.))),
        titlebar: Some(TitlebarOptions { title: Some("Orca Profile Studio".into()), ..Default::default() }),
        ..Default::default()
    };
    if let Err(err) =
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| StudioApp::new(profiles_dir, window, cx)))
    {
        crate::feedback::error("open-window", t!("app.window_error"), t!("app.window_retry", err = err), cx);
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
    management: Option<Entity<ManagementView>>,
    models: Option<Entity<ModelsView>>,
    managing_models: bool,
    _model_events: Option<Subscription>,
    managing: bool,
    batch: Option<Entity<BatchView>>,
    batching: bool,
    _batch_events: Option<Subscription>,
    _management_events: Option<Subscription>,
    /// Unsaved edits of the loaded profiles.
    edits: Option<Entity<Edits>>,
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
    /// The window's default focus target, so app shortcuts such as secondary-k always have a dispatch path.
    focus: FocusHandle,
    _focus_lost: Subscription,
    /// The tree / details split and the browser / issues split; their sizes are saved.
    browser_layout: Entity<ResizableState>,
    workspace_layout: Entity<ResizableState>,
    load_task: Option<Task<()>>,
    load_generation: u64,
    refresh_error: Option<String>,
    refresh_conflicts: Vec<RefreshConflict>,
    _edit_events: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl StudioApp {
    pub fn new(profiles_dir: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        settings::report_load_error(cx);
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
        let focus_lost = cx.on_focus_lost(window, |this, window, cx| {
            window.focus_lost_restore_target(cx).unwrap_or_else(|| this.focus.clone()).focus(window, cx);
        });
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
        let app = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let Some(app) = app.upgrade() else { return true };
            if app.read(cx).is_writing_files(cx) {
                crate::feedback::window_error(
                    window.window_handle(),
                    "manage-writing",
                    t!("lifecycle.writing"),
                    t!("app.wait_close"),
                    cx,
                );
                return false;
            }
            if app.read(cx).unsaved_files(cx).is_empty() {
                remember_window_size(window, cx);
                true
            } else {
                app.update(cx, |app, cx| app.request_close(window, cx));
                false
            }
        });
        let mut app = StudioApp {
            profiles_dir: None,
            state: LoadState::Empty,
            tree,
            selected: None,
            preset_tab: PresetTab::Overview,
            values: None,
            management: None,
            models: None,
            managing_models: false,
            _model_events: None,
            managing: false,
            batch: None,
            batching: false,
            _batch_events: None,
            _management_events: None,
            edits: None,
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
            load_generation: 0,
            refresh_error: None,
            refresh_conflicts: Vec::new(),
            _edit_events: None,
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
        let refreshing = self.profiles_dir.as_ref() == Some(&dir) && matches!(self.state, LoadState::Ready(_));
        self.load_generation += 1;
        let generation = self.load_generation;
        self.profiles_dir = Some(dir.clone());
        self.refresh_error = None;
        self.refresh_conflicts.clear();
        if !refreshing {
            self.state = LoadState::Loading(dir.clone());
            self.selected = None;
            self.values = None;
            self.management = None;
            self.models = None;
            self._model_events = None;
            self.batch = None;
            self._batch_events = None;
            self.managing = false;
            self.managing_models = false;
            self.batching = false;
            self._management_events = None;
            self.edits = None;
            self._edit_events = None;
            self.palette_entries = PaletteEntries::default();
            self.compatibility = None;
            self.issues = None;
            self.issues_open = false;
            self._issues_events = None;
            self.tree.update(cx, |tree, cx| tree.set_items(Vec::new(), cx));
        }

        let job = cx.background_executor().spawn({
            let dir = dir.clone();
            async move { Catalog::load(&dir) }
        });
        self.load_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = job.await;
            this.update_in(cx, |this, window, cx| {
                // A save emits `EditsEvent::Saved`, which starts a newer load: a snapshot read during
                // a save is always superseded here.
                if this.load_generation != generation {
                    return;
                }
                this.finish_load(dir, result, window, cx);
            })
            .ok();
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
        self.load_task = None;
        if let LoadState::Ready(old) = &self.state {
            let old = old.clone();
            match result {
                Ok(catalog) => self.finish_refresh(&old, Arc::new(catalog), window, cx),
                Err(err) => self.refresh_error = Some(t!("app.refresh_failed", err = err)),
            }
            cx.notify();
            return;
        }
        self.state = match result {
            Ok(catalog) => {
                let catalog = Arc::new(catalog);
                let items = nav::build(&catalog);
                self.tree.update(cx, |tree, cx| tree.set_items(items, cx));
                let edits = cx.new(|_| Edits::new(catalog.clone()));
                cx.observe(&edits, |_, _, cx| cx.notify()).detach();
                self._edit_events = Some(cx.subscribe_in(&edits, window, |this, _, _: &EditsEvent, window, cx| {
                    this.reload(window, cx);
                }));
                self.edits = Some(edits);
                settings::update(cx, |settings| settings.profiles_dir = Some(dir.clone()));
                LoadState::Ready(catalog)
            }
            Err(err) => LoadState::Failed(dir, err),
        };
        if matches!(self.state, LoadState::Ready(_)) && settings::get(cx).issues_open {
            self.open_issues(window, cx);
        }
        cx.notify();
    }

    fn finish_refresh(
        &mut self,
        old: &Arc<Catalog>,
        catalog: Arc<Catalog>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(edits) = self.edits.clone() else { return };
        if let Err(conflicts) = edits.update(cx, |edits, _| edits.rebase(&catalog)) {
            self.refresh_error = Some(t!("app.refresh_conflict", arg0 = conflicts.len()));
            self.refresh_conflicts = conflicts;
            return;
        }
        // Read the current view state at completion, so navigation and edits during loading survive.
        self.selected = self.selected.and_then(|node| node.remap(old, &catalog));
        if let Some(values) = self.values.clone() {
            let remapped = Node::Preset(values.read(cx).preset()).remap(old, &catalog);
            if let Some(Node::Preset(preset)) = remapped {
                values.update(cx, |values, cx| values.refresh(&catalog, preset, window, cx));
            } else {
                self.values = None;
            }
        }
        if let Some(issues) = &self.issues {
            issues.update(cx, |issues, cx| issues.refresh(&catalog, window, cx));
        }
        if let Some(management) = &self.management {
            management.update(cx, |management, cx| management.refresh(&catalog, window, cx));
        }
        if let Some(models) = &self.models {
            models.update(cx, |models, cx| models.refresh(&catalog, window, cx));
        }
        if let Some(batch) = &self.batch {
            batch.update(cx, |batch, cx| batch.refresh(&catalog, window, cx));
        }
        self.compatibility = match self.selected {
            Some(Node::Preset(preset)) => Some(Compatibility::compute(&catalog, preset)),
            _ => None,
        };
        self.state = LoadState::Ready(catalog.clone());
        self.tree.update(cx, |tree, cx| {
            let expanded: Vec<Node> =
                nav::expanded_nodes(tree).into_iter().filter_map(|node| node.remap(old, &catalog)).collect();
            let items = nav::build(&catalog);
            nav::expand(&items, &expanded);
            tree.set_items(items, cx);
            if let Some(node) = self.selected {
                let id = node.id();
                tree.set_selected_item(Some(&TreeItem::new(id.clone(), "")), cx);
                tree.reveal_item(&id, ScrollStrategy::Center, cx);
            }
        });
        self.sync_values(window, cx);
        let query = self.palette.as_ref().map(|palette| palette.read(cx).query(cx)).unwrap_or_default();
        self.refresh_palette(&query, cx);
        edits.update(cx, |_, cx| cx.notify());
    }

    pub fn refresh_language(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let LoadState::Ready(catalog) = &self.state {
            let catalog = catalog.clone();
            if let Some(values) = &self.values {
                values.update(cx, |values, cx| values.refresh(&catalog, values.preset(), window, cx));
            }
            if let Some(issues) = &self.issues {
                issues.update(cx, |issues, cx| issues.refresh_language(window, cx));
            }
            if let Some(management) = &self.management {
                management.update(cx, |view, cx| view.refresh_language(window, cx));
            }
            if let Some(models) = &self.models {
                models.update(cx, |view, cx| view.refresh_language(window, cx));
            }
            if let Some(batch) = &self.batch {
                batch.update(cx, |view, cx| view.refresh_language(window, cx));
            }
            self.tree.update(cx, |tree, cx| {
                let expanded = nav::expanded_nodes(tree);
                let items = nav::build(&catalog);
                nav::expand(&items, &expanded);
                tree.set_items(items, cx);
                if let Some(node) = self.selected {
                    tree.set_selected_item(Some(&TreeItem::new(node.id(), "")), cx);
                }
            });
            let query = self.palette.as_ref().map(|p| p.read(cx).query(cx)).unwrap_or_default();
            self.refresh_palette(&query, cx);
        }
        cx.notify();
    }

    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.may_leave_during_write(window, cx) {
            return;
        }
        if window.has_active_dialog(cx) {
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t!("app.open").into()),
        });
        cx.spawn_in(window, async move |this, cx| match picked.await {
            Ok(Ok(Some(mut paths))) => {
                if let Some(dir) = paths.pop() {
                    this.update_in(cx, |this, window, cx| this.request_open(dir, window, cx)).ok();
                }
            }
            Ok(Err(err)) => {
                cx.update(|window, cx| {
                    let window = window.window_handle();
                    crate::feedback::window_error(window, "open-folder", t!("app.folder_error"), err.to_string(), cx)
                })
                .ok();
            }
            _ => {}
        })
        .detach();
    }

    fn request_open(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if !self.may_leave_during_write(window, cx) {
            return;
        }
        let dir = dir.canonicalize().or_else(|_| std::path::absolute(&dir)).unwrap_or(dir);
        let dirty = self.unsaved_files(cx).len();
        if dirty == 0 || self.profiles_dir.as_ref() == Some(&dir) {
            self.load(dir, window, cx);
            return;
        }
        let app = cx.entity();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let app = app.clone();
            let dir = dir.clone();
            dialog
                .title(t!("app.open_title"))
                .confirm()
                .description(t!("app.open_description", arg0 = dir.display(), dirty = dirty))
                .ok_text(t!("app.open_confirm"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    app.update(cx, |this, cx| this.load(dir.clone(), window, cx));
                    true
                })
        });
    }

    pub fn unsaved_files(&self, cx: &App) -> Vec<PathBuf> {
        let (LoadState::Ready(catalog), Some(edits)) = (&self.state, &self.edits) else { return Vec::new() };
        let mut presets = edits.read(cx).dirty();
        if let Some(values) = &self.values {
            let values = values.read(cx);
            if values.has_unapplied_input(cx) && !presets.contains(&values.preset()) {
                presets.push(values.preset());
            }
        }
        if let Some(batch) = &self.batch {
            presets.extend(batch.read(cx).unapplied_presets(cx));
        }
        presets.sort_unstable();
        presets.dedup();
        let mut files: Vec<_> =
            presets.into_iter().map(|index| catalog.repo.root().join(&catalog.repo.presets()[index].path)).collect();
        if let Some(path) = self.models.as_ref().and_then(|models| models.read(cx).unsaved_path(cx)) {
            files.push(path);
        }
        files
    }

    pub fn is_writing_files(&self, cx: &App) -> bool {
        self.management.as_ref().is_some_and(|view| view.read(cx).is_applying())
            || self.models.as_ref().is_some_and(|view| view.read(cx).is_applying())
    }

    fn may_leave_during_write(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.is_writing_files(cx) {
            crate::feedback::window_error(
                window.window_handle(),
                "manage-writing",
                t!("lifecycle.writing"),
                t!("app.wait_write"),
                cx,
            );
            false
        } else {
            true
        }
    }

    fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.may_leave_during_write(window, cx) {
            return;
        }
        if window.has_active_dialog(cx) {
            return;
        }
        let files = self.unsaved_files(cx);
        if files.is_empty() {
            remember_window_size(window, cx);
            window.remove_window();
            return;
        }
        window.open_alert_dialog(cx, move |dialog, _, _| {
            dialog
                .title(t!("app.close_title"))
                .width(px(640.))
                .confirm()
                .description(t!("lifecycle.unsaved"))
                .child(
                    div()
                        .id("close-files")
                        .max_h(rems(24.))
                        .overflow_y_scroll()
                        .children(files.iter().map(|path| div().child(path.display().to_string()))),
                )
                .ok_text(t!("app.close_confirm"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(|_, window, cx| {
                    remember_window_size(window, cx);
                    window.remove_window();
                    true
                })
        });
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.may_leave_during_write(window, cx) {
            return;
        }
        if let Some(dir) = self.profiles_dir.clone() {
            self.load(dir, window, cx);
        }
    }

    fn review_refresh_conflicts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.refresh_conflicts.is_empty() || window.has_active_dialog(cx) {
            return;
        }
        let conflicts = self.refresh_conflicts.clone();
        let generation = self.load_generation;
        let app = cx.entity();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let paths: Vec<_> = conflicts.iter().map(|conflict| conflict.path.clone()).collect();
            let app = app.clone();
            let description = t!("app.conflict_help");
            let files = div().id("refresh-conflict-files").max_h_64().overflow_y_scroll().children(
                conflicts.iter().map(|conflict| {
                    div().py_1().text_sm().child(format!("{} — {}", conflict.path.display(), conflict.reason))
                }),
            );
            dialog
                .title(t!("app.files_changed"))
                .width(px(640.))
                .confirm()
                .description(description)
                .child(files)
                .ok_text(t!("app.discard_reload"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    app.update(cx, |this, cx| {
                        // A newer refresh supersedes the decision shown in this dialog.
                        if this.load_generation != generation {
                            return;
                        }
                        if let Some(edits) = &this.edits {
                            edits.update(cx, |edits, cx| {
                                edits.discard_sessions(&paths);
                                cx.notify();
                            });
                        }
                        this.reload(window, cx);
                    });
                    true
                })
        });
    }

    fn open_batch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_writing_files(cx) {
            return;
        }
        let (LoadState::Ready(catalog), Some(edits)) = (&self.state, &self.edits) else { return };
        if self.batch.is_none() {
            let initial = match self.selected {
                Some(Node::Preset(ix)) => Some(ix),
                _ => None,
            };
            let view = cx.new(|cx| BatchView::new(catalog, edits, initial, window, cx));
            self._batch_events = Some(cx.subscribe(&view, |this, _, _: &BatchEvent, cx| {
                this.batching = false;
                cx.notify();
            }));
            self.batch = Some(view);
        }
        self.managing = false;
        self.managing_models = false;
        self.batching = true;
        self.batch.as_ref().unwrap().update(cx, |batch, cx| batch.focus(window, cx));
        cx.notify();
    }

    fn open_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_writing_files(cx) {
            return;
        }
        // Reopening the retained form must remain possible while it has input.
        if self.edits.as_ref().is_some_and(|edits| !edits.read(cx).dirty().is_empty())
            || self.values.as_ref().is_some_and(|values| values.read(cx).has_unapplied_input(cx))
            || self.batch.as_ref().is_some_and(|batch| !batch.read(cx).unapplied_presets(cx).is_empty())
        {
            crate::feedback::window_error(
                window.window_handle(),
                "manage-unsaved",
                t!("app.save_first"),
                t!("app.save_first_help"),
                cx,
            );
            return;
        }
        let LoadState::Ready(catalog) = &self.state else { return };
        if self.models.is_none() {
            let initial = match self.selected {
                Some(Node::Model(ix)) => Some(ix),
                _ => None,
            };
            let vendor = match self.selected {
                Some(Node::Vendor(ix) | Node::Group(ix, _)) => Some(ix),
                Some(Node::Preset(ix)) => Some(catalog.repo.presets()[ix].vendor.index()),
                Some(Node::Model(ix)) => Some(catalog.repo.models()[ix].vendor.index()),
                None => None,
            };
            let models = cx.new(|cx| ModelsView::new(catalog, initial, vendor, window, cx));
            self._model_events = Some(cx.subscribe_in(&models, window, |this, _, event, window, cx| {
                match event {
                    ManagementEvent::Applied => this.reload(window, cx),
                    ManagementEvent::Close => this.managing_models = false,
                    ManagementEvent::Models => {}
                }
                cx.notify();
            }));
            self.models = Some(models);
        }
        self.managing = false;
        self.batching = false;
        self.managing_models = true;
        self.models.as_ref().unwrap().update(cx, |view, cx| view.focus(window, cx));
        cx.notify();
    }

    fn open_management(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.unsaved_files(cx).is_empty() {
            crate::feedback::window_error(
                window.window_handle(),
                "manage-unsaved",
                t!("app.save_first"),
                t!("app.save_first_help"),
                cx,
            );
            return;
        }
        let LoadState::Ready(catalog) = &self.state else { return };
        if self.management.is_none() {
            let catalog = catalog.clone();
            let selected = match self.selected {
                Some(Node::Preset(ix)) => Some(ix),
                _ => None,
            };
            let management = cx.new(|cx| ManagementView::new(&catalog, selected, window, cx));
            self._management_events = Some(cx.subscribe_in(&management, window, |this, _, event, window, cx| {
                match event {
                    ManagementEvent::Applied => this.reload(window, cx),
                    ManagementEvent::Models => this.open_models(window, cx),
                    ManagementEvent::Close => this.managing = false,
                }
                cx.notify();
            }));
            self.management = Some(management);
        }
        self.managing = true;
        self.managing_models = false;
        self.batching = false;
        cx.notify();
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let location = self
            .profiles_dir
            .as_ref()
            .map_or_else(|| t!("app.no_directory").to_owned(), |dir| dir.display().to_string());
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
                Button::new("batch-edit")
                    .small()
                    .label(t!("common.batch_edit"))
                    .selected(self.batching)
                    .disabled(!matches!(self.state, LoadState::Ready(_)) || self.is_writing_files(cx))
                    .on_click(cx.listener(|this, _, window, cx| this.open_batch(window, cx))),
            )
            .child(
                Button::new("manage-presets")
                    .small()
                    .label(t!("app.presets"))
                    .selected(self.managing)
                    .disabled(!matches!(self.state, LoadState::Ready(_)))
                    .on_click(cx.listener(|this, _, window, cx| this.open_management(window, cx))),
            )
            .child(
                Button::new("open-palette")
                    .small()
                    .icon(IconName::Search)
                    .label(t!("app.search"))
                    .tooltip(t!("app.search_hint", key = crate::i18n::shortcut("secondary-k")))
                    .disabled(!matches!(self.state, LoadState::Ready(_)))
                    .on_click(cx.listener(|this, _, window, cx| this.open_palette(window, cx))),
            )
            .child(
                Button::new("toggle-issues")
                    .small()
                    .icon(IconName::TriangleAlert)
                    .label(t!("common.issues"))
                    .selected(self.issues_open)
                    .disabled(!matches!(self.state, LoadState::Ready(_)))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_issues(window, cx))),
            )
            .child(
                Button::new("open")
                    .small()
                    .icon(IconName::FolderOpen)
                    .label(t!("app.open_folder"))
                    .on_click(cx.listener(|this, _, window, cx| this.choose_folder(window, cx))),
            )
            .child(
                Button::new("reload")
                    .small()
                    .accessibility_label(t!("app.reload"))
                    .icon(IconName::RefreshCw)
                    .tooltip(t!("app.reload_hint"))
                    .disabled(self.profiles_dir.is_none() || self.load_task.is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
            )
            .when(self.load_task.is_some() && matches!(self.state, LoadState::Ready(_)), |bar| {
                bar.child(Spinner::new().small()).child(div().text_sm().child(t!("app.refreshing")))
            })
    }

    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.managing_models
            && let Some(models) = &self.models
        {
            return models.clone().into_any_element();
        }
        if self.batching
            && let Some(batch) = &self.batch
        {
            return batch.clone().into_any_element();
        }
        if self.managing
            && let Some(management) = &self.management
        {
            return management.clone().into_any_element();
        }
        match &self.state {
            LoadState::Empty => placeholder(IconName::FolderOpen, t!("app.empty").to_owned(), cx),
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
                        .child(SharedString::from(t!("app.loading_path", arg0 = dir.display()))),
                )
                .into_any_element(),
            LoadState::Failed(dir, err) => {
                placeholder(IconName::TriangleAlert, t!("app.load_failed", arg0 = dir.display(), err = err), cx)
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
                div().text_sm().text_color(cx.theme().muted_foreground).child(t!("app.select_hint")).into_any_element(),
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
            .child(Tab::new().label(t!("common.overview")))
            .child(Tab::new().label(t!("common.values")))
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
        self.managing = false;
        self.managing_models = false;
        self.batching = false;
        self.preset_tab = tab;
        self.sync_values(window, cx);
        cx.notify();
    }

    /// Rebuilds the palette's groups and targets for `query` and the current selection.
    fn refresh_palette(&mut self, query: &str, cx: &mut Context<Self>) {
        let LoadState::Ready(catalog) = &self.state else { return };
        let preset_selected = matches!(self.selected, Some(Node::Preset(_)));
        let (groups, targets) =
            palette::build(catalog, query, preset_selected, settings::get(cx).theme, settings::get(cx).language);
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
                .placeholder(t!("app.palette_placeholder"))
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
            this.managing = false;
            this.managing_models = false;
            this.batching = false;
            this.preset_tab = PresetTab::Values;
            if let Some(values) = this.ensure_values(preset, window, cx) {
                values.update(cx, |values, cx| values.reveal_key(key, window, cx));
            }
        };
        match target {
            Target::Command(AppCommand::Language(language)) => crate::i18n::set_language(language, cx),
            Target::Command(AppCommand::Reload) => self.reload(window, cx),
            Target::Command(AppCommand::Open) => self.choose_folder(window, cx),
            Target::Command(AppCommand::Theme(theme)) => settings::set_theme(theme, window, cx),
            Target::Command(AppCommand::NewWindow) => window.dispatch_action(Box::new(NewWindow), cx),
            Target::Command(AppCommand::ShowOverview) => self.set_preset_tab(PresetTab::Overview, window, cx),
            Target::Command(AppCommand::ShowValues) => self.set_preset_tab(PresetTab::Values, window, cx),
            Target::Command(AppCommand::ShowIssues) => self.open_issues(window, cx),
            Target::Command(AppCommand::BatchEdit) => self.open_batch(window, cx),
            Target::Command(AppCommand::ManageModels) => self.open_models(window, cx),
            Target::Command(AppCommand::ManagePresets) => self.open_management(window, cx),
            Target::Preset(preset) => {
                self.managing = false;
                self.managing_models = false;
                self.batching = false;
                self.select_node(Node::Preset(preset), true, window, cx);
            }
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
        self.managing = false;
        self.managing_models = false;
        self.batching = false;
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
        if self.issues_open && !self.managing {
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
                let summary = t!(
                    "app.summary",
                    arg0 = repo.vendors().len(),
                    arg1 = repo.models().len(),
                    arg2 = repo.presets().len(),
                    arg3 = catalog.load_time.as_millis()
                );
                let issues: Vec<String> = catalog
                    .issue_totals()
                    .iter()
                    .rev()
                    .map(|(severity, count)| format!("{count} {}", crate::i18n::severity(*severity)))
                    .collect();
                (summary, if issues.is_empty() { t!("app.no_issues").to_owned() } else { issues.join(" · ") })
            }
            LoadState::Loading(_) => (t!("app.loading").to_owned(), String::new()),
            LoadState::Empty | LoadState::Failed(..) => (String::new(), String::new()),
        };
        h_flex()
            .justify_between()
            .flex_wrap()
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
            .on_action(
                cx.listener(|this, _: &crate::lifecycle::CloseWindow, window, cx| this.request_close(window, cx)),
            )
            .child(self.render_toolbar(cx))
            .when_some(self.refresh_error.clone(), |view, message| {
                view.child(
                    h_flex()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(Icon::new(IconName::TriangleAlert).text_color(cx.theme().warning))
                        .child(
                            div()
                                .id("refresh-error")
                                .test_support()
                                .aria_label(message.clone())
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .child(message),
                        )
                        .when(!self.refresh_conflicts.is_empty(), |bar| {
                            bar.child(
                                Button::new("review-refresh-conflicts").small().label(t!("app.review_files")).on_click(
                                    cx.listener(|this, _, window, cx| this.review_refresh_conflicts(window, cx)),
                                ),
                            )
                        }),
                )
            })
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
        cx.update(|cx| cx.set_reduce_motion(true));
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
        let _locale = crate::i18n::test_locale();
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
            // Highlighted values are labelled with their keys.
            assert_eq!(window.find("value-nozzle_diameter").label(), Some("nozzle_diameter"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn values_tab_traces_each_key_to_its_layer(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
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
        let keys: Vec<&str> = rows.iter().map(|row| row[0].as_str()).collect();
        assert!(keys.iter().all(|key| key.contains("nozzle_temperature")), "{keys:?}");
        let position = keys.iter().position(|&key| key == "nozzle_temperature").expect("the key is listed");
        assert_eq!(rows[position][2], "this file");

        cx.update_window(window.into(), |_, window, cx| window.click(("row", position), cx)).unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("value-detail").label(), Some("nozzle_temperature"));
            assert_eq!(window.find(("layer", 0usize)).label(), Some("this file: 270"));
            // One line per layer that sets the key.
            let trace = repo.trace(abs).into_iter().find(|trace| trace.key == "nozzle_temperature").unwrap();
            assert!(window.try_find(("layer", trace.layers.len() - 1)).is_some());
            assert!(window.try_find(("layer", trace.layers.len())).is_none());
        })
        .unwrap();

        // Inherited keys disappear when only this file's values are shown.
        cx.update_window(window.into(), |_, window, cx| {
            window.click("values-search", cx);
            window.press("secondary-a", cx);
            window.press("backspace", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(dump(cx).iter().any(|row| row[2] != "this file"));
        cx.update_window(window.into(), |_, window, cx| window.click("only-this-file", cx)).unwrap();
        cx.run_until_parked();
        let own = dump(cx);
        assert_eq!(own.len(), repo.preset(abs).params.len());
        assert!(own.iter().all(|row| row[2] == "this file"));
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
            window.press("secondary-a", cx);
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
        (rows[0][0].clone(), rows[0][1].clone(), rows[0][2].clone())
    }

    #[gpui_kit::test]
    fn edits_undoes_and_saves_values(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
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
            window.press("secondary-a", cx);
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

        // Change a flag as text, and override a value the template sets.
        find_key(window, "enable_support", cx);
        enter_value(window, "1", cx);
        assert_eq!(first_row(&app, cx).1, "1");
        find_key(window, "sparse_infill_density", cx);
        assert_eq!(first_row(&app, cx).2, "[1] fdm_process_common");
        click(window, "override", cx);
        assert_eq!(first_row(&app, cx).2, "this file (edited)");

        // Saving asks first, then writes only the changed lines.
        click(window, "save", cx);
        cx.update_window(window.into(), |_, window, _| {
            assert_eq!(window.find("save-preview").label(), Some("Changes to save"));
        })
        .unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), CHILD, "nothing is written before confirming");
        click(window, "ok", cx);
        let expected = CHILD
            .replace("\"wall_loops\": \"2\"", "\"wall_loops\": \"3\"")
            .replace("\"enable_support\": \"0\"", "\"enable_support\": \"1\",\n    \"sparse_infill_density\": \"15%\"");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), expected);
        assert!(!dirty(&app, cx));
        cx.update_window(window.into(), |_, window, _| {
            let status = window.find("save-status").label().unwrap_or_default().to_owned();
            assert!(status.starts_with("Saved 1 file."), "{status}");
        })
        .unwrap();

        // Reloading preserves unsaved edits, selection, filters and undo history.
        find_key(window, "wall_loops", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("value-input", cx);
            window.press("secondary-a", cx);
            window.input("4", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        click(window, "reload", cx);
        assert!(app.read_with(cx, |app, _| matches!(app.state, LoadState::Ready(_))));
        cx.update_window(window.into(), |_, window, _| assert!(window.try_find("discard-warning").is_none())).unwrap();
        assert!(dirty(&app, cx));
        assert_eq!(first_row(&app, cx).1, "4");
        click(window, "undo", cx);
        assert_eq!(first_row(&app, cx).1, "3");
        assert!(!dirty(&app, cx));
        click(window, "redo", cx);
        assert_eq!(first_row(&app, cx).1, "4");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), expected, "Reload never writes unsaved edits");
    }

    /// Opens the palette with secondary-k, types `query` and confirms the highlighted entry.
    fn run_palette(window: WindowHandle<Root>, query: &str, cx: &mut TestAppContext) {
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("secondary-k", cx);
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

    fn enter_value(window: WindowHandle<Root>, value: &str, cx: &mut TestAppContext) {
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("value-input", cx);
            window.press("secondary-a", cx);
            window.input(value, cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn saving_refreshes_children_and_keeps_edits_made_while_loading(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = editable_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        let old = catalog(&app, cx);
        let child = old.repo.find(PresetKind::Process, "Acme 0.2")[0].index();
        run_palette(window, "fdm_process_common", cx);
        run_palette(window, "show preset values", cx);
        find_key(window, "sparse_infill_density", cx);
        enter_value(window, "25%", cx);
        run_palette(window, "Acme 0.2", cx);
        find_key(window, "sparse_infill_density", cx);
        let values = app.read_with(cx, |app, _| app.values.clone().unwrap());
        click(window, "save", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("ok", cx);
            // Create a new edit after the synchronous save, before the background load completes.
            let edits = app.read(cx).edits.clone().unwrap();
            edits.update(cx, |edits, cx| {
                edits.session_mut(child).set("wall_loops", Some(profile_core::RawValue::Str("7".into())));
                edits.session_mut(child).set("wall_loops", Some(profile_core::RawValue::Str("8".into())));
                edits.session_mut(child).undo();
                cx.notify();
            });
        })
        .unwrap();
        cx.run_until_parked();
        let fresh = catalog(&app, cx);
        assert!(!Arc::ptr_eq(&old, &fresh));
        assert_eq!(
            first_row(&app, cx),
            ("sparse_infill_density".into(), "25%".into(), "[1] fdm_process_common".into())
        );
        app.read_with(cx, |app, cx| {
            assert_eq!(app.selected, Some(Node::Preset(child)));
            assert_eq!(app.preset_tab, PresetTab::Values);
            assert_eq!(app.values.as_ref(), Some(&values));
            let edits = app.edits.as_ref().unwrap().read(cx);
            assert!(edits.is_dirty(child));
            assert!(edits.session(child).unwrap().can_redo());
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("values-search").value(), Some("sparse_infill_density"));
            assert_eq!(window.find("value-detail").label(), Some("sparse_infill_density"));
        })
        .unwrap();
        find_key(window, "wall_loops", cx);
        assert_eq!(first_row(&app, cx).1, "7");
        click(window, "redo", cx);
        assert_eq!(first_row(&app, cx).1, "8");
        click(window, "undo", cx);
        click(window, "undo", cx);
        assert_eq!(first_row(&app, cx).1, "2");
        assert_eq!(std::fs::read_to_string(dir.path().join("Acme/process/Acme 0.2.json")).unwrap(), CHILD);
        run_palette(window, "sparse_infill_density = 25%", cx);
        assert_eq!(first_row(&app, cx).1, "25%");
        assert_eq!(first_row(&app, cx).2, "this file");
    }

    #[gpui_kit::test]
    fn reload_remaps_sessions_and_selection_after_preset_ids_shift(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = editable_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Acme 0.2", cx);
        run_palette(window, "show preset values", cx);
        find_key(window, "wall_loops", cx);
        enter_value(window, "5", cx);
        let old = catalog(&app, cx).repo.find(PresetKind::Process, "Acme 0.2")[0];
        let path = dir.path().join("Acme.json");
        let manifest = std::fs::read_to_string(&path).unwrap().replace(
            "\"process_list\": [",
            "\"process_list\": [{\"name\":\"extra\",\"sub_path\":\"process/extra.json\"},",
        );
        std::fs::write(&path, manifest).unwrap();
        std::fs::write(
            dir.path().join("Acme/process/extra.json"),
            CHILD.replace("Acme 0.2", "extra").replace("fdm_process_common", ""),
        )
        .unwrap();
        click(window, "reload", cx);
        let fresh = catalog(&app, cx);
        let new = fresh.repo.find(PresetKind::Process, "Acme 0.2")[0];
        assert_ne!(old, new);
        app.read_with(cx, |app, cx| {
            assert_eq!(app.selected, Some(Node::Preset(new.index())));
            let session = app.edits.as_ref().unwrap().read(cx).session(new.index()).unwrap();
            assert_eq!(session.preset(), new);
        });
        assert_eq!(first_row(&app, cx).1, "5");
        click(window, "undo", cx);
        assert_eq!(first_row(&app, cx).1, "2");
        click(window, "redo", cx);
        click(window, "save", cx);
        click(window, "ok", cx);
        assert!(
            std::fs::read_to_string(dir.path().join("Acme/process/Acme 0.2.json"))
                .unwrap()
                .contains("\"wall_loops\": \"5\"")
        );
        assert!(
            std::fs::read_to_string(dir.path().join("Acme/process/extra.json"))
                .unwrap()
                .contains("\"wall_loops\": \"2\"")
        );
    }

    #[gpui_kit::test]
    fn refresh_conflicts_keep_history_until_explicitly_discarded(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = editable_fixture();
        let file = dir.path().join("Acme/process/Acme 0.2.json");
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Acme 0.2", cx);
        run_palette(window, "show preset values", cx);
        find_key(window, "wall_loops", cx);
        enter_value(window, "5", cx);
        let old = catalog(&app, cx);
        std::fs::write(&file, CHILD.replace("\"wall_loops\": \"2\"", "\"wall_loops\": \"9\"")).unwrap();
        click(window, "reload", cx);
        assert!(Arc::ptr_eq(&old, &catalog(&app, cx)));
        assert_eq!(first_row(&app, cx).1, "5");
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("refresh-error").label().unwrap().contains("1 edited file"));
        })
        .unwrap();
        click(window, "review-refresh-conflicts", cx);
        cx.update_window(window.into(), |_, window, cx| window.press("escape", cx)).unwrap();
        cx.run_until_parked();
        click(window, "undo", cx);
        assert_eq!(first_row(&app, cx).1, "2");
        click(window, "redo", cx);
        assert_eq!(first_row(&app, cx).1, "5");
        click(window, "review-refresh-conflicts", cx);
        click(window, "ok", cx);
        assert!(
            !Arc::ptr_eq(&old, &catalog(&app, cx)),
            "{:?}",
            app.read_with(cx, |app, cx| (
                app.load_generation,
                app.refresh_error.clone(),
                app.edits.as_ref().unwrap().read(cx).dirty()
            ))
        );
        assert_eq!(first_row(&app, cx).1, "9");
        app.read_with(cx, |app, cx| {
            assert!(app.refresh_error.is_none());
            assert!(app.edits.as_ref().unwrap().read(cx).dirty().is_empty());
        });
    }

    #[gpui_kit::test]
    fn library_save_refreshes_cross_vendor_values_issues_and_compatibility(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = editable_fixture();
        let write = |path: &str, text: &str| {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("OrcaFilamentLibrary.json", r#"{"name":"OrcaFilamentLibrary","version":"1.0","filament_list":[]}"#);
        write(
            "OrcaFilamentLibrary/filament/base.json",
            r#"{
            "type":"filament","name":"Library PLA","from":"system","instantiation":"true","filament_id":"PLA",
            "nozzle_temperature":["5000"],"compatible_printers_condition":"nozzle_diameter[0] =="
        }"#,
        );
        write(
            "Acme/filament/pla.json",
            r#"{
            "type":"filament","name":"Acme PLA","from":"system","instantiation":"true","inherits":"Library PLA"
        }"#,
        );
        write("Acme/machine/model.json", r#"{"type":"machine_model","name":"A","nozzle_diameter":"0.4"}"#);
        write(
            "Acme/machine/printer.json",
            r#"{
            "type":"machine","name":"A 0.4 nozzle","from":"system","instantiation":"true",
            "printer_model":"A","printer_variant":"0.4","nozzle_diameter":["0.4"]
        }"#,
        );
        let path = dir.path().join("Acme.json");
        let mut manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        manifest["filament_list"] = serde_json::json!([{"name":"Acme PLA","sub_path":"filament/pla.json"}]);
        manifest["machine_model_list"] = serde_json::json!([{"name":"A","sub_path":"machine/model.json"}]);
        manifest["machine_list"] = serde_json::json!([{"name":"A 0.4 nozzle","sub_path":"machine/printer.json"}]);
        std::fs::write(path, manifest.to_string()).unwrap();

        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        let old = catalog(&app, cx);
        run_palette(window, "Acme PLA", cx);
        run_palette(window, "show preset values", cx);
        find_key(window, "nozzle_temperature", cx);
        click(window, "toggle-issues", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("issues-search", cx);
            window.input("compatible_printers_condition", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let issues = app.read_with(cx, |app, _| app.issues.clone().unwrap());
        assert!(!issues.read_with(cx, |issues, cx| issues.table().read(cx).dump(cx).1).is_empty());
        click(window, "toggle-issues", cx);
        let base = old.repo.find(PresetKind::Filament, "Library PLA")[0].index();
        let edits = app.read_with(cx, |app, _| app.edits.clone().unwrap());
        edits.update(cx, |edits, cx| {
            let session = edits.session_mut(base);
            session.set("nozzle_temperature", Some(profile_core::RawValue::List(vec!["210".into()])));
            session.set("compatible_printers_condition", Some(profile_core::RawValue::Str("true".into())));
            cx.notify();
        });
        cx.run_until_parked();
        click(window, "save", cx);
        click(window, "ok", cx);
        app.read_with(cx, |app, cx| {
            let rows = app.values.as_ref().unwrap().read(cx).table().read(cx).dump(cx).1;
            assert_eq!(rows.iter().find(|row| row[0] == "nozzle_temperature").unwrap()[1], "210");
        });
        let fresh = catalog(&app, cx);
        assert!(fresh.repo.issues().len() < old.repo.issues().len(), "saved condition removes the parse error");
        assert_eq!(fresh.search.search("nozzle_temperature 210", 10).values.len(), 1);
        app.read_with(cx, |app, _| assert_eq!(app.compatibility.as_ref().unwrap().printers, ["A 0.4 nozzle"]));
        run_palette(window, "show preset overview", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("compat-summary").label(), Some("1 compatible printer"));
        })
        .unwrap();
        click(window, "toggle-issues", cx);
        app.read_with(cx, |app, cx| {
            assert_eq!(app.issues.as_ref(), Some(&issues));
            let rows = app.issues.as_ref().unwrap().read(cx).table().read(cx).dump(cx).1;
            assert!(rows.is_empty(), "the previous condition error is gone");
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("issues-search").value(), Some("compatible_printers_condition"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn missing_edited_file_and_failed_reload_keep_the_previous_snapshot(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = editable_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Acme 0.2", cx);
        run_palette(window, "show preset values", cx);
        find_key(window, "wall_loops", cx);
        enter_value(window, "6", cx);
        let old = catalog(&app, cx);
        let path = dir.path().join("Acme/process/Acme 0.2.json");
        std::fs::remove_file(&path).unwrap();
        click(window, "reload", cx);
        assert!(Arc::ptr_eq(&old, &catalog(&app, cx)));
        app.read_with(cx, |app, _| {
            assert_eq!(app.refresh_conflicts[0].reason, "File is missing or could not be loaded")
        });
        assert_eq!(first_row(&app, cx).1, "6");
        // Restoring the file allows a retry without sacrificing the edit.
        std::fs::write(&path, CHILD).unwrap();
        click(window, "reload", cx);
        assert!(!Arc::ptr_eq(&old, &catalog(&app, cx)));
        let old = catalog(&app, cx);
        let moved = dir.path().with_extension("moved");
        std::fs::rename(dir.path(), &moved).unwrap();
        click(window, "reload", cx);
        std::fs::rename(&moved, dir.path()).unwrap();
        assert!(Arc::ptr_eq(&old, &catalog(&app, cx)));
        assert_eq!(first_row(&app, cx).1, "6");
        app.read_with(cx, |app, _| assert!(app.refresh_error.as_ref().unwrap().starts_with("Could not refresh")));
        click(window, "reload", cx);
        click(window, "undo", cx);
        assert_eq!(first_row(&app, cx).1, "2");
    }

    #[gpui_kit::test]
    fn partial_save_reports_the_failure_and_refreshes_after_conflict_resolution(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = editable_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Acme 0.2", cx);
        run_palette(window, "show preset values", cx);
        find_key(window, "sparse_infill_density", cx);
        let old = catalog(&app, cx);
        let base = old.repo.find(PresetKind::Process, "fdm_process_common")[0].index();
        let child = old.repo.find(PresetKind::Process, "Acme 0.2")[0].index();
        let edits = app.read_with(cx, |app, _| app.edits.clone().unwrap());
        edits.update(cx, |edits, cx| {
            edits.session_mut(base).set("sparse_infill_density", Some(profile_core::RawValue::Str("30%".into())));
            edits.session_mut(child).set("wall_loops", Some(profile_core::RawValue::Str("5".into())));
            cx.notify();
        });
        std::fs::write(
            dir.path().join("Acme/process/Acme 0.2.json"),
            CHILD.replace("\"wall_loops\": \"2\"", "\"wall_loops\": \"9\""),
        )
        .unwrap();
        click(window, "save", cx);
        click(window, "ok", cx);
        assert!(Arc::ptr_eq(&old, &catalog(&app, cx)));
        app.read_with(cx, |app, cx| {
            assert_eq!(app.refresh_conflicts.len(), 1);
            let edits = app.edits.as_ref().unwrap().read(cx);
            assert!(!edits.is_dirty(base));
            assert!(edits.is_dirty(child));
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let status = window.find("save-status").label().unwrap().to_owned();
            assert!(status.contains("Saved 1 file. Could not save Acme 0.2"), "{status}");
        })
        .unwrap();
        // A separate edit made while resolving the conflict must also survive.
        edits.update(cx, |edits, cx| {
            edits.session_mut(base).set("wall_loops", Some(profile_core::RawValue::Str("6".into())));
            cx.notify();
        });
        click(window, "review-refresh-conflicts", cx);
        // The standard alert dialog also confirms through the keyboard.
        cx.update_window(window.into(), |_, window, cx| window.press("enter", cx)).unwrap();
        cx.run_until_parked();
        assert_eq!(first_row(&app, cx).1, "30%");
        app.read_with(cx, |app, cx| {
            let edits = app.edits.as_ref().unwrap().read(cx);
            assert!(edits.is_dirty(base));
            assert!(edits.session(base).unwrap().can_undo());
            assert!(edits.session(child).is_none());
        });
        assert!(
            std::fs::read_to_string(dir.path().join("Acme/process/Acme 0.2.json"))
                .unwrap()
                .contains("\"wall_loops\": \"9\"")
        );
    }

    #[gpui_kit::test]
    fn palette_searches_and_navigates(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
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
        assert_eq!(rows(cx).iter().map(|row| row[0].as_str()).collect::<Vec<_>>(), ["wall_loops"]);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("value-detail").label(), Some("wall_loops"));
        })
        .unwrap();

        // An option, shown in the selected preset.
        run_palette(window, "enable_support", cx);
        assert_eq!(rows(cx)[0][0], "enable_support");

        // Escape closes the palette without doing anything.
        cx.update_window(window.into(), |_, window, cx| window.press("secondary-k", cx)).unwrap();
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
        let _locale = crate::i18n::test_locale();
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
        cx.update_window(window.into(), |_, window, cx| window.input("dangling-compatible-printer", cx)).unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("down", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let filtered = rows(cx);
        let dangling = repo.issues().iter().filter(|issue| issue.kind.code() == "dangling-compatible-printer").count();
        assert!(dangling > 0);
        assert_eq!(filtered.len(), dangling);
        assert!(filtered.iter().all(|row| row[1] == "dangling-compatible-printer"));

        // Selecting the row shows the preset and the key, without leaving the list.
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.within("issues-list").click(("row", 0usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        app.read_with(cx, |app, _| {
            let Some(Node::Preset(preset)) = app.selected else { panic!("a preset is selected") };
            let id = repo.presets()[preset].id;
            assert!(
                repo.issues().iter().any(|i| i.preset == Some(id) && i.kind.code() == "dangling-compatible-printer")
            );
            assert_eq!(app.preset_tab, PresetTab::Values);
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("value-detail").label(), Some("compatible_printers"));
        })
        .unwrap();

        click(window, "close-issues", cx);
        assert!(!app.read_with(cx, |app, _| app.issues_open));
    }

    #[gpui_kit::test]
    fn overview_shows_compatibility(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
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
        let _locale = crate::i18n::test_locale();
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
        let _locale = crate::i18n::test_locale();
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
        let stored = crate::settings::Settings::load(&path).unwrap();
        assert!(!stored.issues_open);
        assert_eq!(stored.profiles_dir, Some(dir.path().canonicalize().unwrap()));
        assert_eq!(stored.sidebar_width, Some(320.));
        assert_eq!(stored.issues_height, Some(240.));
    }

    #[gpui_kit::test]
    fn new_window_opens_another_window(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = editable_fixture();
        let (window, _) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "new window", cx);
        assert_eq!(cx.update(|cx| cx.windows().len()), 2);
    }

    /// The batch fixture under `resources/profiles`, so a settings file can sit outside it.
    fn localized_fixture() -> (tempfile::TempDir, PathBuf) {
        let source = batch_fixture();
        let dir = tempfile::tempdir().unwrap();
        let profiles = dir.path().join("resources/profiles");
        std::fs::create_dir_all(profiles.parent().unwrap()).unwrap();
        std::fs::rename(source.path(), &profiles).unwrap();
        (dir, profiles)
    }

    #[gpui_kit::test]
    fn language_switch_preserves_unapplied_values_and_persists_preference(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale_switching();
        let (dir, profiles) = localized_fixture();
        let settings_path = dir.path().join("settings.json");
        crate::settings::Settings { language: crate::i18n::LanguagePreference::English, ..Default::default() }
            .save(&settings_path)
            .unwrap();
        let (window, app) = open_with_settings(cx, profiles, Some(settings_path.clone()));
        cx.run_until_parked();
        run_palette(window, "Acme 0.2", cx);
        run_palette(window, "show preset values", cx);
        find_key(window, "wall_loops", cx);
        management_input(window, "value-input", "7", cx);
        let generation = app.read_with(cx, |app, _| app.load_generation);
        run_palette(window, "Language: 中文", cx);
        assert_eq!(
            app.read_with(cx, |app, _| app.load_generation),
            generation,
            "locale switching does not reload profiles"
        );
        assert_eq!(
            crate::settings::Settings::load(&settings_path).unwrap().language,
            crate::i18n::LanguagePreference::Chinese
        );
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("batch-edit").label(), Some("批量编辑"));
            assert_eq!(window.find("value-input").value(), Some("7"));
            assert!(!app.read(cx).unsaved_files(cx).is_empty());
        })
        .unwrap();
        let values = app.read_with(cx, |app, _| app.values.clone().unwrap());
        let (headers, rows) = values.read_with(cx, |view, cx| view.table().read(cx).dump(cx));
        assert_eq!(headers, ["键名", "值", "来源"]);
        assert_eq!(rows[0], ["wall_loops", "2", "本文件"]);
        click(window, "apply", cx);
        click(window, "save", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("save-preview").label(), Some("将要保存的修改"));
            assert_eq!(window.find("cancel").label(), Some("取消"));
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        run_palette(window, "语言 English", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("batch-edit").label(), Some("Batch edit"));
            assert_eq!(window.find("value-input").value(), Some("7"));
        })
        .unwrap();
        assert_eq!(first_row(&app, cx).1, "7");
        click(window, "undo", cx);
        assert_eq!(first_row(&app, cx).1, "2");
        run_palette(window, "Language: 中文", cx);
        cx.simulate_window_resize(window.into(), size(px(960.), px(720.)));
        cx.update(|cx| gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(20.)));
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            for id in ["batch-edit", "manage-presets", "save", "value-input"] {
                let bounds = window.find(id).bounds();
                assert!(bounds.right() <= px(960.) && bounds.bottom() <= px(720.), "{id}: {bounds:?}");
            }
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn language_switch_keeps_batch_selection_and_input_in_every_window(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale_switching();
        let (_dir, profiles) = localized_fixture();
        let (window, app) = open(cx, profiles);
        cx.run_until_parked();
        run_palette(window, "new window", cx);
        let other = cx.windows().into_iter().find(|w| w.window_id() != window.window_id()).unwrap();
        click(window, "batch-edit", cx);
        click(window, "manage-select-shown", cx);
        management_input(window, "value-input", "6", cx);
        click(window, "batch-preview-button", cx);
        assert_eq!(batch_rows(&app, cx).len(), 3);
        run_palette(window, "Language: 中文", cx);
        assert!(batch_rows(&app, cx).is_empty(), "preview language is refreshed by generating a new preview");
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("value-input").value(), Some("6"));
            assert_eq!(window.find("manage-count").label(), Some("已选 3 项 · 显示 3 项"));
        })
        .unwrap();
        cx.update_window(other, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("batch-edit").label(), Some("批量编辑"));
        })
        .unwrap();
        click(window, "batch-preview-button", cx);
        assert!(batch_rows(&app, cx).iter().any(|row| row[2].starts_with("6 — 本文件")));
        run_palette(window, "语言 English", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("value-input").value(), Some("6"));
            assert_eq!(window.find("manage-count").label(), Some("3 selected · 3 shown"));
        })
        .unwrap();
        click(window, "batch-discard-input", cx);
        click(window, "manage-presets", cx);
        management_input(window, "manage-name", "Unsaved copy name", cx);
        run_palette(window, "Language: 中文", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("manage-name").value(), Some("Unsaved copy name"));
            assert_eq!(window.find("manage-preview-button").label(), Some("预览变更…"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn saved_language_localizes_startup_and_preserves_comparison_selection(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale_switching();
        let (dir, profiles) = localized_fixture();
        let path = dir.path().join("settings.json");
        crate::settings::Settings { language: crate::i18n::LanguagePreference::Chinese, ..Default::default() }
            .save(&path)
            .unwrap();
        let (window, app) = open_with_settings(cx, profiles, Some(path));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("batch-edit").label(), Some("批量编辑"));
        })
        .unwrap();
        run_palette(window, "Acme 0.2", cx);
        run_palette(window, "显示预设参数", cx);
        find_key(window, "wall_loops", cx);
        management_input(window, "value-input", "4", cx);
        click(window, "apply", cx);
        click(window, "compare-with", cx);
        choose_comparison(window, "fdm_process_common", cx);
        management_input(window, "compare-search", "wall_loops", cx);
        cx.update_window(window.into(), |_, window, cx| window.click(("row", 0usize), cx)).unwrap();
        cx.run_until_parked();
        run_palette(window, "语言 English", cx);
        let values = app.read_with(cx, |app, _| app.values.clone().unwrap());
        let comparison = values.read_with(cx, |v, _| v.comparison().clone());
        let rows = comparison.read_with(cx, |v, cx| v.table().read(cx).dump(cx).1);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][..4], ["wall_loops", "4", "2", "this file"]);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("compare-search").value(), Some("wall_loops"));
            assert_eq!(window.find("compare-detail").label(), Some("wall_loops"));
        })
        .unwrap();
        run_palette(window, "Language: 中文", cx);
        assert_eq!(comparison.read_with(cx, |v, cx| v.table().read(cx).dump(cx).1[0][3].clone()), "本文件");
    }

    #[gpui_kit::test]
    fn system_language_follows_the_operating_system_at_startup(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale_switching();
        *crate::i18n::TEST_SYSTEM_LOCALE.lock().unwrap() = "zh-Hans-CN".into();
        let dir = editable_fixture();
        // No settings file: the preference is System, applied by the same startup path as the app.
        let (window, _app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("batch-edit").label(), Some("批量编辑"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn chinese_localizes_value_and_issue_rows(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale_switching();
        let dir = editable_fixture();
        let file = dir.path().join("Acme/process/Acme 0.2.json");
        let mut doc = profile_core::Document::parse(std::fs::read_to_string(&file).unwrap()).unwrap();
        doc.set("compatible_printers", &profile_core::RawValue::List(vec!["Gone printer".into()]));
        std::fs::write(file, doc.text()).unwrap();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Acme 0.2", cx);
        run_palette(window, "show preset values", cx);
        find_key(window, "wall_loops", cx);
        run_palette(window, "show issues", cx);
        run_palette(window, "Language: 中文", cx);
        let values = app.read_with(cx, |app, _| app.values.clone().unwrap());
        assert_eq!(values.read_with(cx, |v, cx| v.table().read(cx).dump(cx).1[0][2].clone()), "本文件");
        // Without OrcaSlicer's translation files there is nothing to report missing.
        let shown = cx.update_window(window.into(), |_, window, cx| window.notifications(cx).len()).unwrap();
        assert_eq!(shown, 0);

        let issues = app.read_with(cx, |app, _| app.issues.clone().unwrap());
        let rows = issues.read_with(cx, |v, cx| v.table().read(cx).dump(cx).1);
        assert!(rows.iter().any(|row| row[4].contains("不是可选打印机")), "{rows:?}");
        assert!(rows.iter().all(|row| ["警告", "错误", "致命错误"].contains(&row[0].as_str())));
        run_palette(window, "语言 English", cx);
        let rows = issues.read_with(cx, |v, cx| v.table().read(cx).dump(cx).1);
        assert!(rows.iter().all(|row| ["warning", "error", "fatal"].contains(&row[0].as_str())));
    }

    fn batch_fixture() -> tempfile::TempDir {
        let dir = editable_fixture();
        let manifest = dir.path().join("Acme.json");
        let mut doc = profile_core::Document::parse(std::fs::read_to_string(&manifest).unwrap()).unwrap();
        doc.array_insert(
            "process_list",
            2,
            &serde_json::json!({"name":"Acme 0.3", "sub_path":"process/Acme 0.3.json"}),
        )
        .unwrap();
        std::fs::write(manifest, doc.text()).unwrap();
        std::fs::write(dir.path().join("Acme/process/Acme 0.3.json"), CHILD.replace("Acme 0.2", "Acme 0.3")).unwrap();
        for file in ["fdm_process_common.json", "Acme 0.2.json", "Acme 0.3.json"] {
            let path = dir.path().join("Acme/process").join(file);
            let mut doc = profile_core::Document::parse(std::fs::read_to_string(&path).unwrap()).unwrap();
            doc.set("wall_loops", &profile_core::RawValue::Str("2".into()));
            doc.set("compatible_printers", &profile_core::RawValue::List(vec![file.into()]));
            std::fs::write(path, doc.text()).unwrap();
        }
        dir
    }

    fn batch_select(window: WindowHandle<Root>, control: &'static str, query: &str, cx: &mut TestAppContext) {
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.within(control).click("input", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.press("secondary-a", cx);
            window.input(query, cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.press("down", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    fn batch_rows(app: &Entity<StudioApp>, cx: &mut TestAppContext) -> Vec<Vec<String>> {
        let view = app.read_with(cx, |app, _| app.batch.clone().unwrap());
        view.read_with(cx, |view, cx| view.table().read(cx).dump(cx).1)
    }

    #[gpui_kit::test]
    fn batch_edit_previews_three_files_and_undoes_from_values_after_save(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = batch_fixture();
        let files = ["fdm_process_common.json", "Acme 0.2.json", "Acme 0.3.json"]
            .map(|f| dir.path().join("Acme/process").join(f));
        let before = files.iter().map(|f| std::fs::read_to_string(f).unwrap()).collect::<Vec<_>>();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Acme 0.2", cx);
        run_palette(window, "batch edit", cx);
        click(window, "manage-select-shown", cx);
        management_input(window, "value-input", "5", cx);
        click(window, "batch-preview-button", cx);
        assert_eq!(batch_rows(&app, cx).len(), 3);
        assert!(batch_rows(&app, cx).iter().all(|r| r[2] == "5 — this file"));
        click(window, "batch-review-Acme/process/fdm_process_common.json", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("batch-row-detail").label(), Some("Acme/process/fdm_process_common.json"));
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        for (file, text) in files.iter().zip(&before) {
            assert_eq!(std::fs::read_to_string(file).unwrap(), *text);
        }
        click(window, "batch-apply", cx);
        assert_eq!(app.read_with(cx, |app, cx| app.edits.as_ref().unwrap().read(cx).dirty().len()), 3);
        click(window, "batch-undo", cx);
        assert!(app.read_with(cx, |app, cx| app.edits.as_ref().unwrap().read(cx).dirty().is_empty()));
        click(window, "batch-redo", cx);
        click(window, "batch-save", cx);
        cx.update_window(window.into(), |_, window, cx| window.press("escape", cx)).unwrap();
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&files[1]).unwrap(), before[1]);
        click(window, "batch-save", cx);
        cx.update_window(window.into(), |_, window, cx| window.press("enter", cx)).unwrap();
        cx.run_until_parked();
        for (file, text) in files.iter().zip(&before) {
            assert_eq!(
                std::fs::read_to_string(file).unwrap(),
                text.replace("\"wall_loops\": \"2\"", "\"wall_loops\": \"5\"")
            );
        }
        click(window, "batch-close", cx);
        run_palette(window, "show preset values", cx);
        find_key(window, "wall_loops", cx);
        click(window, "undo", cx);
        assert_eq!(app.read_with(cx, |app, cx| app.edits.as_ref().unwrap().read(cx).dirty().len()), 3);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("save-status").label(), Some("Undid a batch change in 3 presets."));
            window.press("secondary-shift-z", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(app.read_with(cx, |app, cx| app.edits.as_ref().unwrap().read(cx).dirty().is_empty()));
    }

    #[gpui_kit::test]
    fn batch_input_starts_from_current_values_and_choosing_a_key_is_not_unsaved(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = batch_fixture();
        let common = dir.path().join("Acme/process/fdm_process_common.json");
        std::fs::write(
            &common,
            std::fs::read_to_string(&common).unwrap().replace("\"wall_loops\": \"2\"", "\"wall_loops\": \"3\""),
        )
        .unwrap();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Acme 0.2", cx);
        run_palette(window, "batch edit", cx);
        click(window, "manage-select-shown", cx);
        batch_select(window, "batch-operation", "Remove key", cx);
        batch_select(window, "batch-operation", "Set value", cx);
        let unsaved = |cx: &mut TestAppContext| app.read_with(cx, |app, cx| app.unsaved_files(cx).len());
        assert_eq!(unsaved(cx), 0, "choosing a key or an operation is not unsaved input");
        // The untouched input holds the first selected preset's value (3), not another preset's value (2).
        click(window, "batch-preview-button", cx);
        let rows = batch_rows(&app, cx);
        assert_eq!(rows[0][3], "Unchanged (skipped)", "{rows:?}");
        assert_eq!(rows[1][3], "Will change", "{rows:?}");
        management_input(window, "value-input", "7", cx);
        assert_eq!(unsaved(cx), 3);
    }

    #[gpui_kit::test]
    fn batch_array_items_keep_each_list_and_filters_invalidate_previews(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = batch_fixture();
        let files = ["fdm_process_common.json", "Acme 0.2.json", "Acme 0.3.json"]
            .map(|f| dir.path().join("Acme/process").join(f));
        let before = files.iter().map(|f| std::fs::read_to_string(f).unwrap()).collect::<Vec<_>>();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        click(window, "batch-edit", cx);
        click(window, "manage-select-shown", cx);
        batch_select(window, "batch-key", "compatible_printers", cx);
        batch_select(window, "batch-operation", "Add item", cx);
        management_input(window, "value-input", "New printer", cx);
        click(window, "batch-preview-button", cx);
        assert_eq!(batch_rows(&app, cx).len(), 3);
        management_input(window, "manage-search", "Acme 0.2", cx);
        assert!(batch_rows(&app, cx).is_empty());
        click(window, "batch-preview-button", cx);
        assert_eq!(batch_rows(&app, cx).len(), 1);
        management_input(window, "manage-search", "", cx);
        click(window, "manage-select-shown", cx);
        click(window, "batch-preview-button", cx);
        click(window, "batch-apply", cx);
        click(window, "batch-save", cx);
        click(window, "ok", cx);
        for (file, text) in files.iter().zip(&before) {
            let mut expected = profile_core::Document::parse(text).unwrap();
            expected.array_insert("compatible_printers", 1, &serde_json::json!("New printer")).unwrap();
            assert_eq!(std::fs::read_to_string(file).unwrap(), expected.text());
        }
        click(window, "batch-preview-button", cx);
        assert!(batch_rows(&app, cx).iter().all(|r| r[3] == "Unchanged (skipped)"));
        batch_select(window, "batch-operation", "Remove item", cx);
        management_input(window, "value-input", "New printer", cx);
        click(window, "batch-preview-button", cx);
        click(window, "batch-apply", cx);
        click(window, "batch-save", cx);
        click(window, "ok", cx);
        for (file, text) in files.iter().zip(&before) {
            assert_eq!(std::fs::read_to_string(file).unwrap(), *text);
        }
        batch_select(window, "batch-operation", "Remove key", cx);
        click(window, "batch-preview-button", cx);
        assert!(batch_rows(&app, cx).iter().all(|r| r[2] == "(unset)"));
        click(window, "batch-apply", cx);
        click(window, "batch-undo", cx);
        assert!(app.read_with(cx, |app, cx| app.edits.as_ref().unwrap().read(cx).dirty().is_empty()));
    }

    #[gpui_kit::test]
    fn batch_preview_validation_input_protection_and_small_window_layout(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = batch_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        click(window, "batch-edit", cx);
        click(window, "manage-select-shown", cx);
        management_input(window, "value-input", "invalid", cx);
        click(window, "batch-preview-button", cx);
        // Values are not checked against OrcaSlicer's option types: any text is a valid edit.
        assert!(batch_rows(&app, cx).iter().all(|r| r[3] == "Will change" && r[4].is_empty()));
        management_input(window, "value-input", "4", cx);
        assert!(batch_rows(&app, cx).is_empty());
        cx.simulate_window_resize(window.into(), size(px(960.), px(720.)));
        cx.update(|cx| gpui_kit::component::Theme::update(cx, |t| t.font_size = px(20.)));
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            for id in ["batch-preview-button", "batch-apply", "batch-save"] {
                let bounds = window.find(id).bounds();
                assert!(bounds.bottom() <= px(720.), "{id}: {bounds:?}");
                assert!(bounds.right() <= px(960.), "{id}: {bounds:?}");
            }
            let table = window.find("batch-preview").bounds();
            assert!(table.size.height > px(0.));
            assert!(table.top() >= window.find("batch-apply").bounds().bottom());
            window.press("secondary-w", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
        click(window, "cancel", cx);
        assert!(!app.read_with(cx, |app, cx| app.unsaved_files(cx).is_empty()));
        click(window, "batch-discard-input", cx);
        assert!(app.read_with(cx, |app, cx| app.unsaved_files(cx).is_empty()));
        management_input(window, "batch-issue-filter", "no-such-code", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("manage-count").label(), Some("0 selected · 0 shown"));
            assert_eq!(window.find("batch-preview-button").disabled(), Some(true));
        })
        .unwrap();
    }

    fn promotion_fixture() -> tempfile::TempDir {
        let dir = batch_fixture();
        for (file, value) in [("fdm_process_common.json", "1"), ("Acme 0.2.json", "3"), ("Acme 0.3.json", "3")] {
            let path = dir.path().join("Acme/process").join(file);
            let mut doc = profile_core::Document::parse(std::fs::read_to_string(&path).unwrap()).unwrap();
            doc.set("wall_loops", &profile_core::RawValue::Str(value.into()));
            std::fs::write(path, doc.text()).unwrap();
        }
        let path = dir.path().join("Acme.json");
        let mut manifest = profile_core::Document::parse(std::fs::read_to_string(&path).unwrap()).unwrap();
        for (offset, name, parent) in [(3, "Unselected", "fdm_process_common"), (4, "Deep", "Unselected")] {
            manifest
                .array_insert(
                    "process_list",
                    offset,
                    &serde_json::json!({"name":name,"sub_path":format!("process/{name}.json")}),
                )
                .unwrap();
            std::fs::write(dir.path().join(format!("Acme/process/{name}.json")),serde_json::json!({"name":name,"type":"process","from":"system","instantiation":"false","inherits":parent}).to_string()).unwrap();
        }
        std::fs::write(path, manifest.text()).unwrap();
        dir
    }

    #[gpui_kit::test]
    fn promotes_siblings_with_explicit_unselected_impact_and_grouped_undo_after_saving(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = promotion_fixture();
        let untouched = ["Unselected", "Deep"].map(|name| {
            let path = dir.path().join(format!("Acme/process/{name}.json"));
            let text = std::fs::read_to_string(&path).unwrap();
            (path, text)
        });
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Acme 0.2", cx);
        click(window, "batch-edit", cx);
        management_input(window, "manage-search", "Acme 0.", cx);
        click(window, "manage-select-shown", cx);
        batch_select(window, "batch-operation", "Promote to parent", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("value-input").is_none());
        })
        .unwrap();
        click(window, "batch-preview-button", cx);
        let rows = batch_rows(&app, cx);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0][2], "3 — this file");
        assert!(rows[1..3].iter().all(|r| r[2] == "3 — fdm_process_common"));
        assert!(
            rows[3..]
                .iter()
                .all(|r| r[1].starts_with("1 —") && r[2].starts_with("3 —") && r[3].contains("file unchanged"))
        );
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("batch-status").label().unwrap().contains("2 unselected descendants"));
        })
        .unwrap();
        click(window, "batch-apply", cx);
        assert_eq!(app.read_with(cx, |app, cx| app.edits.as_ref().unwrap().read(cx).dirty().len()), 3);
        click(window, "batch-undo", cx);
        assert!(app.read_with(cx, |app, cx| app.edits.as_ref().unwrap().read(cx).dirty().is_empty()));
        click(window, "batch-redo", cx);
        click(window, "batch-save", cx);
        click(window, "ok", cx);
        let loaded = catalog(&app, cx);
        for name in ["Acme 0.2", "Acme 0.3", "Unselected", "Deep"] {
            let id = loaded.repo.find(PresetKind::Process, name)[0];
            assert_eq!(loaded.repo.lookup_scalar(id, "wall_loops"), Some("3"));
            assert!(!loaded.repo.preset(id).params.contains_key("wall_loops"));
        }
        for (path, before) in &untouched {
            assert_eq!(std::fs::read_to_string(path).unwrap(), *before);
        }
        click(window, "batch-close", cx);
        run_palette(window, "show preset values", cx);
        find_key(window, "wall_loops", cx);
        click(window, "undo", cx);
        assert_eq!(app.read_with(cx, |app, cx| app.edits.as_ref().unwrap().read(cx).dirty().len()), 3);
        let other = loaded.repo.find(PresetKind::Process, "Unselected")[0].index();
        assert_eq!(
            app.read_with(cx, |app, cx| app.edits.as_ref().unwrap().read(cx).effective(other, "wall_loops").unwrap().0),
            profile_core::RawValue::Str("1".into())
        );
        click(window, "redo", cx);
        assert!(app.read_with(cx, |app, cx| app.edits.as_ref().unwrap().read(cx).dirty().is_empty()));
    }

    #[gpui_kit::test]
    fn redundant_issue_filter_and_chinese_promotion_keep_selection_and_reject_invalid_targets(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale_switching();
        let dir = batch_fixture(); // All three own wall_loops values are 2.
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "show issues", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.within("issues-severity").click("input", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.press("up", cx);
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        batch_select(window, "issues-code", "redundant-override", cx);
        let issues = app.read_with(cx, |app, _| app.issues.clone().unwrap());
        let rows = issues.read_with(cx, |view, cx| view.table().read(cx).dump(cx).1);
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|r| r[0] == "info" && r[1] == "redundant-override"));
        run_palette(window, "Language: 中文", cx);
        assert!(
            issues
                .read_with(cx, |view, cx| view.table().read(cx).dump(cx).1)
                .iter()
                .all(|r| r[4].contains("与继承值完全相同"))
        );
        run_palette(window, "Acme 0.2", cx);
        click(window, "batch-edit", cx);
        batch_select(window, "batch-operation", "提到父项", cx);
        click(window, "batch-preview-button", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("batch-status").label().unwrap().contains("至少选择两个"));
            assert_eq!(window.find("batch-apply").disabled(), Some(true));
        })
        .unwrap();
        management_input(window, "manage-search", "Acme 0.", cx);
        click(window, "manage-select-shown", cx);
        click(window, "batch-preview-button", cx);
        let rows = batch_rows(&app, cx);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0][3], "父项：已设置此值");
        click(window, "batch-apply", cx);
        assert_eq!(
            app.read_with(cx, |app, cx| app.edits.as_ref().unwrap().read(cx).dirty().len()),
            2,
            "unchanged parent is not part of the undo group"
        );
        click(window, "batch-undo", cx);
        assert!(app.read_with(cx, |app, cx| app.edits.as_ref().unwrap().read(cx).dirty().is_empty()));
    }

    fn model_fixture() -> tempfile::TempDir {
        let dir = editable_fixture();
        let manifest = dir.path().join("Acme.json");
        let mut doc = profile_core::Document::parse(std::fs::read_to_string(&manifest).unwrap()).unwrap();
        doc.array_insert("machine_model_list", 0, &serde_json::json!({"name":"A", "sub_path":"machine/model.json"}))
            .unwrap();
        doc.array_insert("machine_list", 0, &serde_json::json!({"name":"A 0.4", "sub_path":"machine/a.json"})).unwrap();
        std::fs::write(manifest, doc.text()).unwrap();
        std::fs::create_dir_all(dir.path().join("Acme/machine")).unwrap();
        std::fs::write(dir.path().join("Acme/machine/model.json"), r#"{"type":"machine_model", "name":"A", "model_id":"A-ID", "nozzle_diameter":"0.4;0.6", "machine_tech":"FFF", "custom":"keep"}"#).unwrap();
        std::fs::write(dir.path().join("Acme/machine/a.json"), r#"{"type":"machine","name":"A 0.4","from":"system","instantiation":"true","printer_model":"A","printer_variant":"0.4","nozzle_diameter":["0.4"]}"#).unwrap();
        dir
    }

    #[gpui_kit::test]
    fn model_form_input_without_a_chosen_model_names_a_model_file(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = model_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        click(window, "manage-presets", cx);
        click(window, "manage-models", cx);
        batch_select(window, "models-operation", "Edit or rename model", cx);
        management_input(window, "models-family", "Family", cx);
        let unsaved = app.read_with(cx, |app, cx| app.unsaved_files(cx));
        assert_eq!(unsaved.len(), 1, "{unsaved:?}");
        assert!(unsaved[0].starts_with(dir.path().canonicalize().unwrap().join("Acme/machine")), "{unsaved:?}");
    }

    #[gpui_kit::test]
    fn manages_models_with_create_copy_edit_rename_delete_and_explicit_previews(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = model_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        click(window, "manage-presets", cx);
        click(window, "manage-models", cx);
        management_input(window, "models-name", "B", cx);
        management_input(window, "models-model_id", "B-ID", cx);
        click(window, "models-preview", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("manage-preview").label(), Some("2 file changes"));
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(!dir.path().join("Acme/machine/B.json").exists());
        click(window, "models-preview", cx);
        click(window, "ok", cx);
        assert_eq!(catalog(&app, cx).repo.models().len(), 2);
        assert!(app.read_with(cx, |app, cx| app.unsaved_files(cx).is_empty()));
        batch_select(window, "models-operation", "Copy model", cx);
        management_input(window, "models-name", "C", cx);
        management_input(window, "models-model_id", "C-ID", cx);
        click(window, "models-preview", cx);
        click(window, "ok", cx);
        assert_eq!(catalog(&app, cx).repo.models().len(), 3);
        let file = dir.path().join("Acme/machine/C.json");
        let before = std::fs::read_to_string(&file).unwrap();
        management_input(window, "models-family", "Family C", cx);
        click(window, "models-preview", cx);
        click(window, "ok", cx);
        let mut expected = profile_core::Document::parse(before).unwrap();
        expected.set("family", &profile_core::RawValue::Str("Family C".into()));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), expected.text());
        management_input(window, "models-name", "D", cx);
        management_input(window, "models-file_name", "D.json", cx);
        click(window, "models-preview", cx);
        click(window, "ok", cx);
        assert!(!file.exists());
        assert!(dir.path().join("Acme/machine/D.json").exists());
        batch_select(window, "models-operation", "Delete model", cx);
        click(window, "models-preview", cx);
        click(window, "ok", cx);
        assert!(!dir.path().join("Acme/machine/D.json").exists());
        let loaded = catalog(&app, cx);
        assert_eq!(loaded.repo.models().len(), 2);
        assert!(!loaded.repo.issues().iter().any(|i| i.severity() >= profile_core::Severity::Error));
    }

    #[gpui_kit::test]
    fn model_previews_block_references_removed_variants_and_stale_writes(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = model_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Machine models", cx);
        batch_select(window, "models-source", "model.json", cx);
        batch_select(window, "models-operation", "Delete model", cx);
        click(window, "models-preview", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("ok").label(), Some("Back"));
            window.press("enter", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(dir.path().join("Acme/machine/model.json").exists());
        batch_select(window, "models-operation", "Edit or rename", cx);
        management_input(window, "models-nozzle_diameter", "0.2", cx);
        click(window, "models-preview", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("ok").label(), Some("Back"));
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        click(window, "models-reset", cx);
        management_input(window, "models-name", "Renamed", cx);
        management_input(window, "models-file_name", "Renamed.json", cx);
        click(window, "models-preview", cx);
        let path = dir.path().join("Acme/machine/model.json");
        let before = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("{before}\n")).unwrap();
        click(window, "ok", cx);
        assert!(!dir.path().join("Acme/machine/Renamed.json").exists());
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("models-status").label().unwrap().contains("changed since the preview"));
        })
        .unwrap();
        std::fs::write(&path, &before).unwrap();
        click(window, "models-preview", cx);
        click(window, "ok", cx);
        let loaded = catalog(&app, cx);
        let printer = loaded.repo.find(PresetKind::Machine, "A 0.4")[0];
        assert_eq!(loaded.repo.lookup_scalar(printer, "printer_model"), Some("Renamed"));
    }

    #[gpui_kit::test]
    fn model_input_survives_language_reload_and_close_checks(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale_switching();
        let dir = model_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Machine models", cx);
        batch_select(window, "models-source", "model.json", cx);
        management_input(window, "models-name", "Draft name", cx);
        run_palette(window, "Language: 中文", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("models-name").value(), Some("Draft name"));
            assert_eq!(window.find("models-preview").label(), Some("预览变更…"));
            window.press("secondary-w", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
        click(window, "cancel", cx);
        let path = dir.path().join("Acme/machine/model.json");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("A-ID", "external-ID")).unwrap();
        click(window, "reload", cx);
        assert_eq!(catalog(&app, cx).repo.models()[0].model_id.as_deref(), Some("external-ID"));
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("models-name").value(), Some("Draft name"));
        })
        .unwrap();
        click(window, "models-preview", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("models-status").label().unwrap().contains("来源机型已变化"));
        })
        .unwrap();
        click(window, "models-reset", cx);
        assert!(app.read_with(cx, |app, cx| app.unsaved_files(cx).is_empty()));
        cx.simulate_window_resize(window.into(), size(px(960.), px(720.)));
        cx.update(|cx| gpui_kit::component::Theme::update(cx, |t| t.font_size = px(20.)));
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            for id in ["models-preview", "models-reset", "models-close"] {
                let bounds = window.find(id).bounds();
                assert!(bounds.right() <= px(960.) && bounds.bottom() <= px(720.), "{id}: {bounds:?}");
            }
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn model_writes_finish_before_quit_close_reload_and_folder_switch(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = model_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Machine models", cx);
        management_input(window, "models-name", "Protected", cx);
        management_input(window, "models-model_id", "P-ID", cx);
        click(window, "models-preview", cx);
        let generation = app.read_with(cx, |app, _| app.load_generation);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("ok", cx);
            assert!(app.read(cx).is_writing_files(cx));
            window.press("secondary-w", cx);
            window.click("open", cx);
            window.click("reload", cx);
            window.press("secondary-q", cx);
            assert_eq!(app.read(cx).load_generation, generation);
        })
        .unwrap();
        assert_eq!(cx.windows().len(), 1);
        assert!(!cx.did_prompt_for_paths());
        let mut visual = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        assert!(!visual.simulate_close());
        cx.run_until_parked();
        assert!(dir.path().join("Acme/machine/Protected.json").exists());
        assert!(!app.read_with(cx, |app, cx| app.is_writing_files(cx)));
    }

    fn management_fixture() -> tempfile::TempDir {
        let dir = editable_fixture();
        for file in ["Acme.json", "Acme/process/fdm_process_common.json", "Acme/process/Acme 0.2.json"] {
            let path = dir.path().join(file);
            let text = std::fs::read_to_string(&path).unwrap();
            // Keep source paths fixed, so choosing a row tests selection by file identity.
            let mut object: serde_json::Value = serde_json::from_str(&text).unwrap();
            if file == "Acme.json" {
                object["process_list"][0]["name"] = "Base 0.4 nozzle".into();
                object["process_list"][1]["name"] = "Process 0.4 nozzle".into();
            } else {
                object["name"] =
                    if file.ends_with("fdm_process_common.json") { "Base 0.4 nozzle" } else { "Process 0.4 nozzle" }
                        .into();
                if object.get("inherits").is_some() {
                    object["inherits"] = "Base 0.4 nozzle".into();
                }
            }
            std::fs::write(path, serde_json::to_string_pretty(&object).unwrap()).unwrap();
        }
        dir
    }

    fn management_operation(window: WindowHandle<Root>, label: &str, cx: &mut TestAppContext) {
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.within("manage-operation").click("input", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.press("secondary-a", cx);
            window.input(label, cx);
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

    fn management_input(window: WindowHandle<Root>, id: &'static str, text: &str, cx: &mut TestAppContext) {
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
            window.press("secondary-a", cx);
            if text.is_empty() {
                window.press("backspace", cx);
            } else {
                window.input(text, cx);
            }
        })
        .unwrap();
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn manages_new_copy_rename_and_delete_with_explicit_previews(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = management_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Process 0.4 nozzle", cx);
        click(window, "manage-presets", cx);
        management_input(window, "manage-name", "Process copy", cx);
        click(window, "manage-preview-button", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("manage-preview").label().unwrap().contains("2 file changes"));
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(!dir.path().join("Acme/process/Process copy.json").exists());
        click(window, "manage-preview-button", cx);
        click(window, "ok", cx);
        let loaded = catalog(&app, cx);
        assert_eq!(loaded.repo.find(PresetKind::Process, "Process copy").len(), 1);
        assert!(app.read_with(cx, |app, _| app.managing));

        management_operation(window, "New preset", cx);
        management_input(window, "manage-name", "Fresh process", cx);
        click(window, "manage-inherit-source", cx);
        click(window, "manage-preview-button", cx);
        click(window, "ok", cx);
        let loaded = catalog(&app, cx);
        let fresh = loaded.repo.find(PresetKind::Process, "Fresh process")[0];
        assert_eq!(loaded.repo.preset(fresh).meta.inherits(), Some("Process 0.4 nozzle"));

        management_operation(window, "Rename preset", cx);
        management_input(window, "manage-name", "Renamed process", cx);
        click(window, "manage-preview-button", cx);
        click(window, "ok", cx);
        let loaded = catalog(&app, cx);
        assert!(loaded.repo.find(PresetKind::Process, "Process 0.4 nozzle").is_empty());
        let renamed = loaded.repo.find(PresetKind::Process, "Renamed process")[0];
        assert!(loaded.repo.preset(renamed).meta.renamed_from.as_ref().unwrap().contains("Process 0.4 nozzle"));
        let fresh = loaded.repo.find(PresetKind::Process, "Fresh process")[0];
        assert_eq!(loaded.repo.preset(fresh).meta.inherits(), Some("Renamed process"));

        management_operation(window, "Delete presets", cx);
        click(window, "manage-select-shown", cx);
        click(window, "manage-preview-button", cx);
        click(window, "ok", cx);
        assert!(catalog(&app, cx).repo.presets().is_empty());
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("Acme.json")).unwrap()).unwrap();
        assert_eq!(manifest["process_list"], serde_json::json!([]));
    }

    #[gpui_kit::test]
    fn batch_copy_keeps_inheritance_and_applies_multiple_values(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = management_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Process 0.4 nozzle", cx);
        run_palette(window, "manage presets", cx);
        cx.simulate_window_resize(window.into(), size(px(1280.), px(1100.)));
        management_operation(window, "Copy group", cx);
        click(window, "manage-select-shown", cx);
        management_input(window, "manage-from", "0.4 nozzle", cx);
        management_input(window, "manage-to", "0.2 nozzle", cx);
        management_input(window, "manage-overrides", "wall_loops = 5\nenable_support = 1", cx);
        click(window, "manage-preview-button", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("manage-preview").label(), Some("3 file changes"));
        })
        .unwrap();
        click(window, "ok", cx);
        let loaded = catalog(&app, cx);
        let parent = loaded.repo.find(PresetKind::Process, "Base 0.2 nozzle")[0];
        let child = loaded.repo.find(PresetKind::Process, "Process 0.2 nozzle")[0];
        assert_eq!(loaded.repo.preset(child).parent, Some(parent));
        assert_eq!(loaded.repo.lookup_scalar(child, "wall_loops"), Some("5"));
        assert_eq!(loaded.repo.lookup_scalar(child, "enable_support"), Some("1"));
        assert!(parent.index() < child.index());
        assert!(!loaded.repo.issues().iter().any(|issue| issue.severity() >= Severity::Error));
    }

    #[gpui_kit::test]
    fn management_rejects_collisions_and_stale_previews_without_writing(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = management_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Process 0.4 nozzle", cx);
        click(window, "manage-presets", cx);
        management_input(window, "manage-name", "Process 0.4 nozzle", cx);
        click(window, "manage-preview-button", cx);
        click(window, "ok", cx); // A blocking preview's button only returns to the form.
        assert_eq!(catalog(&app, cx).repo.presets().len(), 2);
        management_input(window, "manage-name", "Stale copy", cx);
        click(window, "manage-preview-button", cx);
        let path = dir.path().join("Acme/process/Acme 0.2.json");
        let before = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, before.replace("\"wall_loops\": \"2\"", "\"wall_loops\": \"9\"")).unwrap();
        click(window, "ok", cx);
        assert!(!dir.path().join("Acme/process/Stale copy.json").exists());
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.find("manage-status").label().unwrap().contains("changed since the preview"));
        })
        .unwrap();
        assert!(std::fs::read_to_string(path).unwrap().contains("\"wall_loops\": \"9\""));
    }

    #[gpui_kit::test]
    fn management_preserves_existing_edits_and_filters_selection_by_model(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = management_fixture();
        let path = dir.path().join("Acme.json");
        let mut manifest: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        manifest["machine_model_list"] = serde_json::json!([{"name":"A","sub_path":"machine/model.json"}]);
        manifest["machine_list"] = serde_json::json!([{"name":"A printer","sub_path":"machine/a.json"}]);
        std::fs::write(path, manifest.to_string()).unwrap();
        std::fs::create_dir_all(dir.path().join("Acme/machine")).unwrap();
        std::fs::write(
            dir.path().join("Acme/machine/model.json"),
            r#"{"type":"machine_model","name":"A","nozzle_diameter":"0.4"}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("Acme/machine/a.json"),r#"{"type":"machine","name":"A printer","from":"system","instantiation":"true","printer_model":"A","printer_variant":"0.4","nozzle_diameter":["0.4"]}"#).unwrap();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Process 0.4 nozzle", cx);
        run_palette(window, "show preset values", cx);
        find_key(window, "wall_loops", cx);
        enter_value(window, "6", cx);
        click(window, "manage-presets", cx);
        assert!(!app.read_with(cx, |app, _| app.managing));
        assert_eq!(first_row(&app, cx).1, "6");
        // The "save first" notice can cover the toolbar buttons, so undo from the keyboard.
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("row", 0usize), cx);
            window.press("secondary-z", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(first_row(&app, cx).1, "2");
        click(window, "manage-presets", cx);
        management_input(window, "manage-model", "A", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("manage-count").label(), Some("1 selected · 1 shown"));
        })
        .unwrap();
        // A filter that hides the selected preset also removes it from the selection, so an
        // operation never includes rows the user cannot see.
        management_input(window, "manage-model", "no matching model", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("manage-count").label(), Some("0 selected · 0 shown"));
        })
        .unwrap();
        management_input(window, "manage-model", "", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("manage-count").label(), Some("0 selected · 2 shown"));
        })
        .unwrap();
        click(window, "manage-select-shown", cx);
        management_input(window, "manage-search", "Base", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("manage-count").label(), Some("1 selected · 1 shown"));
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn management_write_finishes_before_close_quit_or_folder_switch(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = management_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Process 0.4 nozzle", cx);
        click(window, "manage-presets", cx);
        management_input(window, "manage-name", "Protected copy", cx);
        click(window, "manage-preview-button", cx);
        let generation = app.read_with(cx, |app, _| app.load_generation);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("ok", cx);
            assert!(app.read(cx).is_writing_files(cx));
            window.press("secondary-w", cx);
            window.click("open", cx);
            window.click("reload", cx);
            window.press("secondary-q", cx);
            assert_eq!(app.read(cx).load_generation, generation);
        })
        .unwrap();
        assert_eq!(cx.windows().len(), 1);
        assert!(!cx.did_prompt_for_paths());
        let mut visual = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        assert!(!visual.simulate_close(), "native close also waits for the write to finish");
        cx.run_until_parked();
        assert_eq!(cx.windows().len(), 1);
        assert!(!app.read_with(cx, |app, cx| app.is_writing_files(cx)));
        assert_eq!(catalog(&app, cx).repo.find(PresetKind::Process, "Protected copy").len(), 1);
        assert!(dir.path().join("Acme/process/Protected copy.json").exists());
    }

    #[gpui_kit::test]
    fn long_management_preview_keeps_confirmation_visible_in_a_small_window(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = management_fixture();
        let (window, _) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Process 0.4 nozzle", cx);
        click(window, "manage-presets", cx);
        cx.simulate_window_resize(window.into(), size(px(1280.), px(1100.)));
        management_input(window, "manage-name", "Long preview copy", cx);
        let values = (0..24).map(|ix| format!("review_key_{ix} = example")).collect::<Vec<_>>().join("\n");
        management_input(window, "manage-overrides", &values, cx);
        cx.simulate_window_resize(window.into(), size(px(960.), px(720.)));
        cx.update(|cx| gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(20.)));
        click(window, "manage-preview-button", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let preview = window.find("manage-preview").bounds();
            for id in ["ok", "cancel"] {
                let button = window.find(id);
                assert!(button.visible());
                assert!(button.bounds().bottom() <= px(720.), "{id}: {:?}", button.bounds());
                assert!(preview.bottom() <= button.bounds().top());
            }
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(!dir.path().join("Acme/process/Long preview copy.json").exists());
    }

    #[gpui_kit::test]
    fn override_and_keyboard_save_follow_focus_and_confirmation(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = editable_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Acme 0.2", cx);
        run_palette(window, "show preset values", cx);
        find_key(window, "sparse_infill_density", cx);
        click(window, "override", cx);
        enter_value(window, "20%", cx);
        assert_eq!(first_row(&app, cx).1, "20%");
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(("row", 0usize), cx);
            window.press("secondary-z", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(first_row(&app, cx), ("sparse_infill_density".into(), "15%".into(), "this file (edited)".into()));
        cx.update_window(window.into(), |_, window, cx| window.press("secondary-shift-z", cx)).unwrap();
        cx.run_until_parked();
        assert_eq!(first_row(&app, cx).1, "20%");
        cx.update_window(window.into(), |_, window, cx| window.press("secondary-s", cx)).unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.has_active_dialog(cx));
            assert_eq!(window.find("save-preview").label(), Some("Changes to save"));
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(dir.path().join("Acme/process/Acme 0.2.json")).unwrap(), CHILD);
        cx.update_window(window.into(), |_, window, cx| window.press("secondary-s", cx)).unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.press("enter", cx)).unwrap();
        cx.run_until_parked();
        let loaded = catalog(&app, cx);
        let child = loaded.repo.find(PresetKind::Process, "Acme 0.2")[0];
        assert_eq!(loaded.repo.lookup_scalar(child, "sparse_infill_density"), Some("20%"));
        app.read_with(cx, |app, cx| assert!(app.edits.as_ref().unwrap().read(cx).dirty().is_empty()));
    }

    #[gpui_kit::test]
    fn opening_a_folder_requires_explicit_discard_and_cancellation_keeps_edits(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = editable_fixture();
        let other = editable_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Acme 0.2", cx);
        run_palette(window, "show preset values", cx);
        find_key(window, "wall_loops", cx);
        enter_value(window, "8", cx);
        click(window, "open", cx);
        assert!(cx.did_prompt_for_paths());
        cx.simulate_path_prompt_response(|_| None);
        cx.run_until_parked();
        assert_eq!(first_row(&app, cx).1, "8");
        click(window, "open", cx);
        cx.simulate_path_prompt_response(|_| Some(vec![other.path().to_path_buf()]));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert!(window.has_active_dialog(cx));
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(first_row(&app, cx).1, "8");
        click(window, "open", cx);
        cx.simulate_path_prompt_response(|_| Some(vec![other.path().to_path_buf()]));
        cx.run_until_parked();
        click(window, "ok", cx);
        assert_eq!(catalog(&app, cx).repo.root(), other.path().canonicalize().unwrap());
        assert_eq!(std::fs::read_to_string(dir.path().join("Acme/process/Acme 0.2.json")).unwrap(), CHILD);
    }

    #[gpui_kit::test]
    fn settings_save_errors_are_visible_deduplicated_and_retryable(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = editable_fixture();
        let storage = tempfile::tempdir().unwrap();
        let folder = storage.path().join("nested");
        let path = folder.join("settings.json");
        let (window, _) = open_with_settings(cx, dir.path().to_path_buf(), Some(path.clone()));
        cx.run_until_parked();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(&folder).unwrap();
        std::fs::write(&folder, "blocked").unwrap();
        run_palette(window, "theme: dark", cx);
        cx.update_window(window.into(), |_, window, cx| {
            assert!(cx.theme().is_dark(), "settings still apply in memory");
            assert_eq!(window.notifications(cx).len(), 1);
        })
        .unwrap();
        run_palette(window, "theme: dark", cx);
        cx.update_window(window.into(), |_, window, cx| assert_eq!(window.notifications(cx).len(), 1)).unwrap();
        std::fs::remove_file(&folder).unwrap();
        std::fs::create_dir(&folder).unwrap();
        run_palette(window, "theme: dark", cx);
        assert_eq!(crate::settings::Settings::load(&path).unwrap().theme, ThemePreference::Dark);
    }

    #[gpui_kit::test]
    fn invalid_settings_and_window_errors_have_user_visible_feedback(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = editable_fixture();
        let storage = tempfile::tempdir().unwrap();
        let path = storage.path().join("settings.json");
        std::fs::write(&path, "invalid json").unwrap();
        let (window, _) = open_with_settings(cx, dir.path().to_path_buf(), Some(path.clone()));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| assert_eq!(window.notifications(cx).len(), 1)).unwrap();
        cx.update(|cx| crate::feedback::error("open-window", "Could not open a window", "Platform error".into(), cx));
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| assert_eq!(window.notifications(cx).len(), 2)).unwrap();
        // The first saved change replaces the file; its unreadable content is kept aside.
        run_palette(window, "theme: dark", cx);
        assert_eq!(crate::settings::Settings::load(&path).unwrap().theme, ThemePreference::Dark);
        assert_eq!(std::fs::read_to_string(storage.path().join("settings.json.invalid")).unwrap(), "invalid json");
    }

    #[gpui_kit::test]
    fn window_errors_only_reach_their_window(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = editable_fixture();
        let (first, _) = open(cx, dir.path().to_path_buf());
        let (second, _) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        cx.update(|cx| {
            crate::feedback::window_error(first.into(), "open-folder", "Could not choose a folder", "No".into(), cx)
        });
        cx.run_until_parked();
        cx.update_window(first.into(), |_, window, cx| assert_eq!(window.notifications(cx).len(), 1)).unwrap();
        cx.update_window(second.into(), |_, window, cx| assert_eq!(window.notifications(cx).len(), 0)).unwrap();
    }

    #[gpui_kit::test]
    fn startup_error_can_be_reported_without_a_window(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        cx.update(super::init);
        cx.update(|cx| crate::feedback::error("open-window", "Could not open a window", "Platform error".into(), cx));
        cx.run_until_parked();
        let notifications = cx.shown_system_notifications();
        assert_eq!(notifications.len(), 1);
        assert_eq!(notifications[0].title.as_ref(), "Could not open a window");
    }

    #[gpui_kit::test]
    fn native_close_and_close_shortcut_protect_edits_and_unapplied_text(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = editable_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Acme 0.2", cx);
        run_palette(window, "show preset values", cx);
        find_key(window, "wall_loops", cx);
        enter_value(window, "4", cx);
        let mut visual = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        assert!(!visual.simulate_close(), "native close is held while the user decides");
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            assert!(window.has_active_dialog(cx));
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(first_row(&app, cx).1, "4");
        click(window, "undo", cx);
        // Text that has not been applied is also work the close operation must protect.
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("value-input", cx);
            window.press("secondary-a", cx);
            window.input("9", cx);
            window.press("secondary-w", cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| assert!(window.has_active_dialog(cx))).unwrap();
        click(window, "cancel", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("value-input").value(), Some("9"));
            window.press("secondary-w", cx);
        })
        .unwrap();
        cx.run_until_parked();
        click(window, "ok", cx);
        assert!(cx.windows().is_empty());
        assert_eq!(std::fs::read_to_string(dir.path().join("Acme/process/Acme 0.2.json")).unwrap(), CHILD);
    }

    #[gpui_kit::test]
    fn quit_checks_all_windows_and_rechecks_new_edits_before_discarding(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = editable_fixture();
        let (first, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(first, "Acme 0.2", cx);
        run_palette(first, "show preset values", cx);
        find_key(first, "wall_loops", cx);
        enter_value(first, "4", cx);
        run_palette(first, "new window", cx);
        let second =
            cx.windows().into_iter().find(|handle| *handle != first.into()).unwrap().downcast::<Root>().unwrap();
        cx.update_window(second.into(), |_, window, cx| {
            window.render_frame(cx);
            window.press("secondary-q", cx);
        })
        .unwrap();
        cx.run_until_parked();
        let dialog_window = [first, second]
            .into_iter()
            .find(|handle| cx.update_window((*handle).into(), |_, window, cx| window.has_active_dialog(cx)).unwrap())
            .expect("quitting a clean window still finds the dirty window");
        click(dialog_window, "cancel", cx);
        assert_eq!(cx.windows().len(), 2);
        assert_eq!(first_row(&app, cx).1, "4");
        cx.update_window(second.into(), |_, window, cx| window.press("secondary-q", cx)).unwrap();
        cx.run_until_parked();
        let base = catalog(&app, cx).repo.find(PresetKind::Process, "fdm_process_common")[0].index();
        app.read_with(cx, |app, _| app.edits.clone().unwrap()).update(cx, |edits, cx| {
            edits.session_mut(base).set("wall_loops", Some(profile_core::RawValue::Str("7".into())));
            cx.notify();
        });
        click(dialog_window, "ok", cx);
        assert_eq!(cx.windows().len(), 2, "new unsaved files require a new decision");
        cx.update_window(dialog_window.into(), |_, window, cx| assert!(window.has_active_dialog(cx))).unwrap();
        click(dialog_window, "ok", cx);
        assert!(cx.windows().is_empty());
        assert_eq!(std::fs::read_to_string(dir.path().join("Acme/process/Acme 0.2.json")).unwrap(), CHILD);
    }

    #[gpui_kit::test]
    fn small_window_and_larger_type_keep_controls_inside_their_regions(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = editable_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Acme 0.2", cx);
        run_palette(window, "show preset values", cx);
        find_key(window, "wall_loops", cx);
        cx.simulate_window_resize(window.into(), size(px(960.), px(720.)));
        cx.update(|cx| gpui_kit::component::Theme::update(cx, |theme| theme.font_size = px(20.)));
        click(window, "toggle-issues", cx);
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            for id in ["close-issues", "issues-search", "save", "reload"] {
                let element = window.find(id);
                assert!(element.visible(), "{id} should remain visible");
                assert!(element.bounds().right() <= px(960.), "{id}: {:?}", element.bounds());
                assert!(element.bounds().bottom() <= px(720.), "{id}: {:?}", element.bounds());
            }
            let table = window.find("values-table").bounds();
            let detail = window.find("value-detail").bounds();
            assert!(table.size.height > px(0.), "the table still has a viewport: {table:?}");
            assert!(table.bottom() <= detail.top(), "table and editor must not overlap");
            assert_eq!(window.find("reload").label(), Some("Reload profiles"));
            assert_eq!(window.find("undo").label(), Some("Undo edit"));
        })
        .unwrap();
        click(window, "close-issues", cx);
        assert!(!app.read_with(cx, |app, _| app.issues_open));
    }

    #[gpui_kit::test]
    fn saving_keeps_the_folders_the_user_expanded(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = editable_fixture();
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        let loaded = catalog(&app, cx);
        let child = loaded.repo.find(PresetKind::Process, "Acme 0.2")[0].index();
        let template = loaded.repo.find(PresetKind::Process, "fdm_process_common")[0].index();
        click_node(window, &app, Node::Vendor(0), cx);
        click_node(window, &app, Node::Group(0, nav::Group::Templates), cx);
        click_node(window, &app, Node::Group(0, nav::Group::Processes), cx);
        click_node(window, &app, Node::Preset(child), cx);
        let visible = |node: Node, cx: &mut TestAppContext| {
            app.read_with(cx, |app, cx| app.tree.read(cx).index_of(&node.id()).is_some())
        };
        assert!(visible(Node::Preset(template), cx));
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.within("preset-tabs").click(1usize, cx);
        })
        .unwrap();
        cx.run_until_parked();
        find_key(window, "wall_loops", cx);
        enter_value(window, "3", cx);
        click(window, "save", cx);
        click(window, "ok", cx);
        cx.run_until_parked();
        let refreshed = catalog(&app, cx);
        assert!(!Arc::ptr_eq(&loaded, &refreshed), "saving refreshed the catalog");
        let template = refreshed.remap_preset(&loaded, template).unwrap();
        assert!(visible(Node::Preset(template), cx), "the Templates folder is still expanded");
    }

    #[gpui_kit::test]
    fn a_saved_file_changed_elsewhere_does_not_pause_the_refresh(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
        let dir = editable_fixture();
        let file = dir.path().join("Acme/process/Acme 0.2.json");
        let (window, app) = open(cx, dir.path().to_path_buf());
        cx.run_until_parked();
        run_palette(window, "Acme 0.2", cx);
        run_palette(window, "show preset values", cx);
        find_key(window, "wall_loops", cx);
        enter_value(window, "3", cx);
        click(window, "save", cx);
        click(window, "ok", cx);
        cx.run_until_parked();
        // Another window or `git checkout` changes the saved file; nothing is unsaved here.
        std::fs::write(&file, std::fs::read_to_string(&file).unwrap().replace("\"3\"", "\"9\"")).unwrap();
        click(window, "reload", cx);
        cx.run_until_parked();
        let loaded = catalog(&app, cx);
        let id = loaded.repo.find(PresetKind::Process, "Acme 0.2")[0];
        assert_eq!(loaded.repo.lookup_scalar(id, "wall_loops"), Some("9"));
        let preset = id.index();
        app.read_with(cx, |app, cx| {
            assert_eq!(app.refresh_error, None);
            assert!(app.refresh_conflicts.is_empty());
            assert!(app.edits.as_ref().unwrap().read(cx).session(preset).is_none(), "the stale history is dropped");
        });
    }

    #[gpui_kit::test]
    fn reports_a_directory_that_cannot_be_loaded(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
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
        let _locale = crate::i18n::test_locale();
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
            window.press("secondary-a", cx);
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
        let wall = rows.iter().find(|row| row[0] == "wall_loops").expect("edited wall count differs");
        // The template does not set the key, so its side is missing rather than a default.
        assert_eq!(wall[1..], ["4", "(missing)", "this file", "—"]);
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
            assert!(window.try_find(("Right", 0usize)).is_none());
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
        assert_eq!(dump(cx)[0][4], "this file");
        click(window, "reload", cx);
        assert_eq!(dump(cx).len(), 1, "comparison filter and visibility survive refresh");
        assert_eq!(dump(cx)[0][1], "4");
        assert_eq!(dump(cx)[0][2], "4");
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("compare-detail").label(), Some("wall_loops"));
            assert_eq!(window.find("only-differences").checked(), Some(false));
        })
        .unwrap();
        click(window, "close-comparison", cx);
        assert_eq!(first_row(&app, cx).1, "4", "returning preserves the edit and filter");
        assert_eq!(std::fs::read_to_string(dir.path().join("Acme/process/Acme 0.2.json")).unwrap(), CHILD);
    }

    #[gpui_kit::test]
    fn compares_real_abs_nozzle_variants(cx: &mut TestAppContext) {
        let _locale = crate::i18n::test_locale();
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
        assert_eq!(rows[0][0], "compatible_printers");
        assert!(rows[0][1].contains("0.2 nozzle"));
        assert!(rows[0][2].contains("0.4 nozzle"));
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
            window.click(("icon-sort", 0usize), cx);
        })
        .unwrap();
        cx.run_until_parked();
        compare.read_with(cx, |view, cx| {
            let table = view.table().read(cx);
            let rows = table.dump(cx).1;
            assert!(rows.windows(2).all(|pair| pair[0][0].to_lowercase() >= pair[1][0].to_lowercase()));
            assert_eq!(rows[table.selected_row().unwrap()][0], "compatible_printers");
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find("compare-detail").label(), Some("compatible_printers"));
        })
        .unwrap();
    }
}
