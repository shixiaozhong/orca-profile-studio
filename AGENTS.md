# AGENTS.md

Guidance for Codex and other coding agents working in this repository. `CLAUDE.md` holds the same
guidance for Claude Code; when changing shared guidance, update both files.

## Overview

orca-profile-studio browses, validates and edits OrcaSlicer's system profiles
(`resources/profiles`). It is also a GPUI learning project. Rust workspace, edition 2024.

```
crates/
├── profile-core/    # loading, inheritance resolution, validation; no GUI dependencies
├── profile-tool/    # CLI: stats / check / resolve
└── profile-studio/  # gpui-kit desktop app
docs/roadmap.md      # roadmap: stages, design decisions, done criteria
docs/reference/      # profile format spec + Python reference stats script
scripts/             # bundle-macos.sh (.app bundle), icon.swift (draws the icon source PNG)
AGENTS.md            # this file (Codex); CLAUDE.md is the Claude Code counterpart
```

## Commands

```bash
export ORCA_PROFILES_DIR=../OrcaSlicer/resources/profiles
cargo test --workspace
cargo clippy --workspace --all-targets
cargo fmt
cargo run --release -p profile-tool -- stats
cargo run --release -p profile-tool -- check --vendor Snapmaker --min info
cargo run --release -p profile-tool -- resolve "Generic ABS @U1 0.2 nozzle"
cargo run --release -p profile-tool -- search "nozzle_temperature 270"
cargo run --release -p profile-tool -- compat "0.07mm DETAIL @CORE One 0.25"
cargo run -p profile-studio -- ../OrcaSlicer/resources/profiles
scripts/bundle-macos.sh [--install]   # target/release/bundle/Orca Profile Studio.app
```

The only expected build warning is a future-incompatibility note from the third-party `block` crate.
A relative `ORCA_PROFILES_DIR` is resolved against the workspace root in tests (Cargo runs them
from the package directory).

## Data source: the OrcaSlicer checkout

- Profiles come from `ORCA_PROFILES_DIR`, else `../OrcaSlicer/resources/profiles` next to this repo.
- `../OrcaSlicer` is a separate repository (Snapmaker's OrcaSlicer fork, the user works on branch
  `personal/notes`). Treat it as read-only unless the task is explicitly about changing it.
- `docs/reference/profile_format_spec.md` is the authoritative description of the format and of
  OrcaSlicer's loading semantics, with source references into the OrcaSlicer repo. Read it before
  changing loader behaviour.

## profile-core semantics (mirror OrcaSlicer, see the spec)

- Vendors load with OrcaFilamentLibrary first, then alphabetically (OrcaSlicer uses unspecified
  directory order). Each vendor loads sections in order process → filament → machine; each section
  has its own name table and is processed strictly in manifest order, so a parent must be listed
  before its child.
- A child replaces whole keys (arrays are not merged element-wise). The only cross-vendor parent
  source is OrcaFilamentLibrary's filaments.
- Lookups use the preset's own `name`, not the manifest entry name or file name.
- Manifest `sub_path`s go through `profile-core`'s `sub_path` module (M17): written with `/` on every
  platform, converted to native paths at load, matched by path components, case-folded in `/` form
  for collision checks. Never compare or write them via `to_string_lossy`/`to_str`.
- UTF-8 BOM is tolerated (nlohmann::json skips it; serde_json does not) but reported.
- No schema (M16): the project analyses profile files only. It has no option types, defaults,
  labels or value checks from OrcaSlicer's sources; a key no file in the chain sets has no value.
  Never add files exported or copied from OrcaSlicer (schema, `.po` translations) to this repository.
- Loading never aborts: every deviation is an `Issue` with a `Severity`:
  `Fatal` = OrcaSlicer would refuse to load the whole vendor, `Error` = OrcaSlicer logs and continues,
  `Warning` = loads but is probably unintended, `Info` = informational.
- `crates/profile-core/tests/orca_profiles.rs` asserts exact counts for OrcaSlicer commit
  `953509c23f`. When the profile data changes, update the numbers deliberately; the reference
  script `docs/reference/profile_format_stats.py` must agree. Its `--check` flag asserts the
  independent baseline and exits nonzero on drift; CI runs it.

## Compatibility cross-check (M6)

- Condition variables are typed from their text (M16): an integer is an int, a number (a trailing
  `%` allowed) a double, anything else a string; an unset variable is an empty string, and an
  unindexed list whose items are all equal reads as that item.
- `Snapmaker_Orca_compat_export` in the OrcaSlicer repo (branch `personal/schema-export`, CMake target
  under `ORCA_TOOLS`; a new target needs `cmake -S ../OrcaSlicer -B ../OrcaSlicer/build/vscode-arm64`) loads the system
  profiles the way the GUI does (temporary data_dir with a `system` symlink, nothing is written into
  the checkout) and exports OrcaSlicer's own `is_compatible_with_printer` verdicts. Compare with:
  ```bash
  cmake --build ../OrcaSlicer/build/vscode-arm64 --target Snapmaker_Orca_compat_export
  ../OrcaSlicer/build/vscode-arm64/src/Snapmaker_Orca_compat_export.app/Contents/MacOS/Snapmaker_Orca_compat_export \
      --profiles ../OrcaSlicer/resources/profiles -o /tmp/compat.json --commit $(git -C ../OrcaSlicer rev-parse --short=10 HEAD)
  cargo run --release -p profile-tool -- compat-check /tmp/compat.json
  ```
  After M16 there are 0 differences (OrcaSlicer `ba538f3bc1`, whose profiles equal `953509c23f`).
  The table (~700 KB) is not committed.

## GPUI / gpui-kit notes (learned the hard way)

- `gpui-kit = "=0.8.0"` (bundles `gpui-pre =0.3.9`, pre-1.0, breaking changes between versions).
  Its sources are in `~/.cargo/registry/src/*/gpui-kit-0.8.0`, `gpui-component-0.8.0`,
  `gpui-base-0.8.0`, `gpui-pre-0.3.9`; read them for API details rather than guessing.
- Icons: use `gpui_kit::assets::IconName` (full Lucide catalog). `gpui_kit::component::IconName`
  is only the default subset (no Printer, Spool, Layers…). The app registers `AllAssets`.
- `font_semibold` and similar need `gpui_kit::component::StyledExt` in scope.
- In `#[cfg(test)]` modules never glob-import `gpui_kit::*` (directly or via `use super::*` from a
  module that globs it): with `test-support` the glob brings GPUI's `test` macro, which shadows
  `#[test]` and makes `#[gpui_kit::test]` recurse. Import test items explicitly.
- `gpui-kit`'s `test-support` dev feature needs `cocoa = 0.26.0` in `Cargo.lock`; a fresh
  `cargo generate-lockfile` resolves it, pinning a single crate does not.
- Screenshots of app windows fail in this environment (no screen-recording permission). Verify UI
  with headless tests: `#[gpui_kit::test]`, `gpui_kit::test::TestWindowExt` (`find`, `click`,
  `press`), `cx.run_until_parked()` for background work. Give elements `.id(..).test_support()
  .aria_label(..)` so tests can query them; tree rows are found by their row index, `DataTable`
  rows by `("row", ix)`, tabs by index inside their bar (`window.within("preset-tabs").click(1usize)`).
  `TableState::dump(cx)` (via `TableDelegate::cell_text`) reads table contents.
- After a click whose effect goes through a subscription (e.g. `TableEvent::SelectRow`), call
  `cx.run_until_parked()` and `render_frame` before asserting on the new frame.
- Read the gpui-kit Coding Guides (skill `gpui-kit`, `references/coding-guides.md`) before UI work,
  and the Design Guides (skill `gpui-kit-design-guides`) before designing or changing a screen.
  Layout sizes use rem helpers (`w_48`, `h_72`, `gap_2`…) or `rems(..)` when no helper fits, so
  they follow the theme's base font. `px(..)` only where the API takes `Pixels`: table column
  widths, resizable panel sizes, `Dialog::w` / `margin_top`, window bounds.
  `render` only reads state: views that need a `Window` (`TableState`, `InputState`) are created in
  event handlers. `StudioApp::sync_values` creates the `ValuesView` from the tree observer
  (`observe_in`), the tab handler and the palette; `ValuesView::sync_editor` rebuilds the value
  editor from the row-selection subscription and the `Edits` observer.
- With `test-support`, `.test_support()` wraps an element in `Observed<…>`: a helper that adds it
  cannot return `Stateful<Div>`; return `AnyElement` (an `impl IntoElement` return captures `cx`).
- Saving emits `EditsEvent::Saved` (including partial success), which triggers a background full
  refresh. Reload uses the same path and retains edits. `Edits::rebase` maps sessions by file path;
  `EditSession::rebase` preserves undo/redo and rejects external value changes. Only sessions with
  unsaved changes conflict (a saved one whose file changed drops its history). A conflict pauses
  the whole snapshot replacement; the window retains its old data until retry or explicit discard
  of the conflicting sessions. Keep the `load_generation` check when changing this flow (every save
  starts a newer load, so a snapshot read during a save never lands), and migrate the latest state
  at completion so edits made during loading survive. Tree expansion is carried over by node path
  (`nav::expanded_nodes` / `nav::expand`); `Catalog::remap_*` maps indexes between loads.
- Save previews and destructive confirmations use `Root`'s `AlertDialog`: Enter commits and
  Escape cancels. App-wide errors use `feedback::error` (every window), one window's errors
  `feedback::window_error`; both are deferred until window updates finish,
  with stable notification IDs and a visible Dismiss action. Settings writes retry unchanged
  values after failures; an unreadable settings file is copied to `settings.json.invalid` before
  it is overwritten, or saving is disabled for the run when the copy fails. Startup without a window falls back to a system notification and stderr.
- `lifecycle` handles application-menu/Command-Q quit across all windows; native close and
  Command-W check one window. Both include unapplied editor text. Quit rechecks the window/file
  scope before discarding because another window may gain edits while confirmation is open.
  GPUI's non-cancellable system shutdown callback cannot protect direct OS termination.
- Preset management (M10): `profile_core::ManagementPlan` (`manage.rs`) plans create/copy/rename/delete
  as file snapshots plus blocking or review notices and never writes while planning. Staleness is
  checked against `Repository`'s content fingerprints of every file it read (any byte change means
  Reload). `validate` loads only the candidate (unchanged files hard-linked into a temp dir) and diffs
  its issues against the loaded repository, mapping renamed paths. `apply` takes an in-process
  per-folder lock (never a lock file), stages in `.orca-profile-studio-transaction-*`, applies
  removals, then preset files, then manifests, and rolls back on failure. `Document::array_*` edits
  one array element and keeps all other bytes. The studio's `ManagementView` (`manage.rs`) and the
  reusable `PresetPicker` (`preset_picker.rs`, selection by file path, pruned to visible rows) run
  previews and writes on the background executor; writing blocks close, quit, reload and open.
- Batch editing (M11): `BatchView` previews a `BatchPlan` on the background executor from a cloned
  `Edits` snapshot. It resolves every edited ancestor, stages one key per preset, and applies only
  after the baseline still matches. Invalidate previews on selection/input/session/reload changes.
  String-array item operations materialize inherited values only when changed; no-op rows are skipped.
  Targets are planned in index (load) order, so a child without its own value that already receives
  the target through a selected, edited ancestor is skipped instead of getting a redundant override.
  Set input starts from the first selected preset's current value; only typed input counts as unapplied.
  `EditSession` history entries carry group IDs; `Edits` owns group membership by file path. Route
  UI undo/redo through `Edits::history` / `history_group`, never directly through a grouped session:
  all member stack tops must agree before any mutation. Lost member history blocks the whole group.
  `ValueEditor` is shared by Values and Batch; `save::confirm` is the common save preview/commit.
  Batch Apply only changes sessions; Save keeps M3's per-file/partial-success semantics. Include
  unapplied batch input in window/open/quit protection, even while the batch view is hidden.
- Machine-model management (M13): `ModelDraft` / `ModelPlan` in core `manage/models.rs` own model
  fields and references, then reuse `ManagementPlan` for validation and file transactions. Model
  names are vendor-scoped manifest identities, distinct from `model_id`; rename only rewrites own
  `printer_model` values in that vendor and leaves inheriting children untouched. Deletion and
  removed variants with machine references block writing. Resource existence is a preview notice,
  not a new repository Issue; see format spec 4.1 for OrcaSlicer's fallback paths. Never copy assets.
  The form edits `default_materials_text` (file order), not the sorted `default_materials`; a cleared
  field removes its key; new or copied models need a `model_id`; a rename notes a cover image that
  still carries the old name.
  `ModelsView` retains form input across language changes and refreshes, and requires Reset input
  if a reloaded source changed. `is_writing_files` and `unsaved_files` include the model view even
  when hidden; the shared `manage::preview_plan` handles both preset and model confirmations.
- Redundant overrides / promotion (M14): `redundant-override` is an info-only exact RawValue
  comparison against the parent's effective value. Roots and incomplete chains are excluded. The
  Values view and the batch preview's new-issue column compare
  against `Edits::effective` of the parent, so unsaved ancestor edits count; promotion into an
  OrcaFilamentLibrary parent is refused (`PromotionError::SharedLibrary`). Count at `953509c23f`: 56,926;
  `profile_format_stats.py ROOT` independently agrees. Never auto-remove them.
  `PromotionPlan::with_values` reads current own values for all presets; it requires distinct
  siblings with the same own value, a complete chain, and a same-vendor parent. It returns the
  actual parent/child changes separately from unselected descendant impacts. Impact rows must not
  become edit sessions or writes. `BatchPlan` preserves M11 staleness checks and groups the parent
  (only if changed) with the children for atomic undo/redo; retain the visible impact count.
- Localization (M12): `i18n::set_language` switches the process-wide locale and defers an in-place
  refresh of all windows. Never reload profiles or rebase Edits just to translate a view. Preserve
  retained input entities, enum values, selection and undo history; update labels/placeholders and
  table headers in handlers, not render. Batch previews invalidate without clearing typed input.
  Only the app's own UI text is translated; option names are always the raw keys (M16 removed the
  `.po` reading). `issue_text` exhaustively translates IssueKind for UI only; core/CLI stay
  English. The locale is process-wide: UI tests that only read English text hold
  `i18n::test_locale()` (shared, runs in parallel); tests that switch language hold
  `i18n::test_locale_switching()` (exclusive, restores English). The System preference uses the same
  startup path in tests; `i18n::TEST_SYSTEM_LOCALE` pins the detected OS locale (English unless a
  test sets it). Translation completeness and placeholder parity are checked against literal `t!` keys in the app sources.
- Saving writes into the profiles directory, i.e. the OrcaSlicer checkout. UI tests that save use a
  temporary fixture (`editable_fixture` in `app.rs`), never the real profiles.
- App preferences live in the `SettingsStore` global (`settings.rs`), saved on every change to
  `orca-profile-studio/settings.json` under the platform's config directory (`settings_path`: macOS
  Application Support, Windows `%APPDATA%`, else XDG) or to `ORCA_STUDIO_SETTINGS`. UI tests
  open windows through `open_with_settings(cx, dir, None)`, which keeps settings in memory, so they
  never touch the user's file. Resizable splits keep an `Entity<ResizableState>`; its `Resized`
  event (also emitted by `resize_panel`) is where sizes are saved.
- Key bindings and test key presses use `secondary-` (⌘ on macOS, Ctrl elsewhere), never `cmd-`;
  UI text shows a shortcut through `i18n::shortcut(..)`, never a literal ⌘. Quit lives in the app
  menu on macOS and at the end of File elsewhere. System notifications only work from the `.app`
  (GPUI needs a bundle identifier); `cargo run` falls back to stderr.
- Shortcuts only dispatch when something is focused: `StudioApp` focuses its root `FocusHandle` at
  creation; `cx.on_focus_lost` restores the nearest surviving focus ancestor, falling back to the root. Never
  request focus from `render`.
- Modal surfaces go through `Root`: the palette is `window.open_dialog(...)`, which traps focus,
  handles Escape and restores focus on close. The dialog builder may rerun; it only reads state.
- `InputState::set_value` does not emit `InputEvent::Change`; update dependent state yourself.
- The palette is a `Command` with `filterable(false)`: the app builds the groups from `on_query`
  and maps `on_confirm`'s `IndexPath { section, row }` to its own targets (`palette.rs`).
- `TreeItem` holds `Rc` and is not `Send`: do heavy work (repository load, `Catalog::new`) on the
  background executor, build tree items on the foreground.

## Conventions

- `rustfmt.toml`: `max_width = 120`, `use_small_heuristics = "Max"`. Run `cargo fmt` and keep
  clippy clean.
- Identifiers, comments and CLI strings in English; README is in Chinese. App UI strings live in
  `crates/profile-studio/locales/app.yml` (en / zh-CN), accessed through `t!`; keep element IDs in English.
- Commit messages: conventional style (`feat: …`, `fix: …`, `docs: …`) with a short bullet body,
  ending with a blank line and the trailer `Co-Authored-By: Codex (<model>) <noreply@openai.com>`,
  where `<model>` is the model this session runs on (e.g. `Codex (gpt-5-codex)`); if you are not
  sure of the model name, write `Co-Authored-By: Codex <noreply@openai.com>`. Commit only when
  asked. The remote `origin` is the public GitHub repository
  `shixiaozhong/orca-profile-studio`; push only when asked.
- One task, one commit: code, tests and docs (roadmap, README, AGENTS.md/CLAUDE.md) go together.
  A commit cannot contain its own hash (amending changes it), so never make a follow-up commit
  just to record a hash. Put the stage in the subject instead (`feat: … (M10)`), leave the
  roadmap's 提交 cell as `待补`, and fill it with the short hash (`git log --grep '(M10)'`) in
  the next commit that edits the roadmap anyway.
- `Cargo.toml` deliberately has no `license` field; the user has not chosen one.
- `rust-version = "1.95"` matches GPUI's use of `std::hint::cold_path`; CI checks the locked workspace
  on Linux and macOS. serde-saphyr and smol_str already require 1.89. The local toolchain is 1.97.

## Roadmap

[docs/roadmap.md](docs/roadmap.md) (Chinese) is the roadmap: stages and status, key design decisions
of finished stages, scope / design / dependencies / done criteria of the next ones. Done: M0, M0.5,
M1–M17 (M15: CI on GitHub Actions, Linux and macOS, real-data regression from a cached sparse
checkout of the public OrcaSlicer fork at the pinned commit; M16: schema removed, profiles only;
M17: `.app` bundle script, secondary- key bindings, per-platform menus and settings path, `/` in
manifest sub_paths). Next: M18 Windows build and portable zip (CI Windows job plus the user's own machine). Git diff is pending (待定) and the OrcaSlicer upgrade check is
deferred (暂缓); pending and deferred items get no M number until the user decides to do them.
The G-code viewer and OrcaSlicer sync automation are deferred.
When a stage is finished, move it to "已完成" there with its design decisions, mark it ✅, and update
the status line in README, all in the stage's own commit (see the commit rule above for the 提交 column).
The user committing a stage is its acceptance: there is no separate "awaiting validation" status.

Changing the compat exporter is work in another repository (`../OrcaSlicer`); confirm with the user first.

## Working with the user

- The user is a C++ developer on Snapmaker's OrcaSlicer fork and writes in Chinese; reply in Chinese.
- Do not fix, or offer to fix, OrcaSlicer's profile data (`../OrcaSlicer/resources/profiles`), even
  when the checks find real bugs there; report findings only.
- Any change in the OrcaSlicer repository (e.g. the compat exporter) needs the user's confirmation
  first. The exporter lives on its own branch, `personal/schema-export`.
