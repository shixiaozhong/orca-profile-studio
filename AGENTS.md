# AGENTS.md

Guidance for Codex and other coding agents working in this repository. `CLAUDE.md` holds the same
guidance for Claude Code; when changing shared guidance, update both files.

## Overview

orca-profile-studio browses, validates and (later) edits OrcaSlicer's system profiles
(`resources/profiles`). It is also a GPUI learning project. Rust workspace, edition 2024.

```
crates/
├── profile-core/    # loading, inheritance resolution, validation; no GUI dependencies
│   └── schema/      # schema.json exported from OrcaSlicer (bundled via include_str!)
├── profile-tool/    # CLI: stats / check / resolve
└── profile-studio/  # gpui-kit desktop app
docs/roadmap.md      # roadmap: stages, design decisions, done criteria
docs/reference/      # profile format spec + Python reference stats script
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
cargo run -p profile-studio -- ../OrcaSlicer/resources/profiles
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
- UTF-8 BOM is tolerated (nlohmann::json skips it; serde_json does not) but reported.
- `Repository::load` works without a schema; `load_with_schema` (used by the CLI and the app, with
  `Schema::bundled()`) also checks every value (`check.rs`) and resolves unset keys to the kind's
  default preset (`Origin::Default`).
- Loading never aborts: every deviation is an `Issue` with a `Severity`:
  `Fatal` = OrcaSlicer would refuse to load the whole vendor, `Error` = OrcaSlicer logs and continues,
  `Warning` = loads but is probably unintended, `Info` = informational.
- `crates/profile-core/tests/orca_profiles.rs` asserts exact counts for OrcaSlicer commit
  `953509c23f`. When the profile data changes, update the numbers deliberately; the reference
  script `docs/reference/profile_format_stats.py` must agree (it does not cover the schema checks,
  whose counts only live in the `schema_*` tests).

## Schema (M0.5)

- Exporter: `src/dev-utils/Snapmaker_Orca_schema_export.cpp` in the OrcaSlicer repo, branch
  `personal/schema-export`, CMake target `Snapmaker_Orca_schema_export` (under `ORCA_TOOLS`, next
  to the profile validator). It writes every `ConfigOptionDef`, the three default presets
  (`PresetBundle` constructor) and the `Preset::*_options()` lists. Regenerate with:
  ```bash
  cmake --build ../OrcaSlicer/build/vscode-arm64 --target Snapmaker_Orca_schema_export
  ../OrcaSlicer/build/vscode-arm64/src/Snapmaker_Orca_schema_export.app/Contents/MacOS/Snapmaker_Orca_schema_export \
      -o crates/profile-core/schema/schema.json --commit $(git -C ../OrcaSlicer rev-parse --short=10 HEAD)
  ```
  A new target needs a CMake reconfigure (`cmake -S ../OrcaSlicer -B ../OrcaSlicer/build/vscode-arm64`).
- "Which keys belong to a kind" = keys of that kind's default preset (what `remove_invalid_keys`
  checks against), not `option_lists`.
- `legacy.rs` ports `PrintConfigDef::handle_legacy` by hand (rules are code, not data); keep it in
  sync when regenerating. Unknown keys are silently dropped by OrcaSlicer, so they are warnings.
- Severities follow what the C++ loader does with `EnableSystemSilent` (the GUI's rule): scalar
  numbers that do not parse → `Fatal` (vendor fails); bad enum/bool → `Error` (silently replaced by
  the default); unparsable vector items → `Error` (read as 0); trailing text, `nil` in a
  non-nullable vector, GUI min/max → `Warning`.

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
- Views that need a `Window` (`TableState`, `InputState`) are created lazily in `render`, e.g. the
  preset's `ValuesView` in `StudioApp::render_preset` and its value editor in `ValuesView::sync_editor`.
- With `test-support`, `.test_support()` wraps an element in `Observed<…>`: a helper that adds it
  cannot return `Stateful<Div>`; return `AnyElement` (an `impl IntoElement` return captures `cx`).
- Saving writes into the profiles directory, i.e. the OrcaSlicer checkout. UI tests that save use a
  temporary fixture (`editable_fixture` in `app.rs`), never the real profiles.
- `TreeItem` holds `Rc` and is not `Send`: do heavy work (repository load, `Catalog::new`) on the
  background executor, build tree items on the foreground.

## Conventions

- `rustfmt.toml`: `max_width = 120`, `use_small_heuristics = "Max"`. Run `cargo fmt` and keep
  clippy clean.
- Identifiers, comments, CLI and UI strings in English; README is in Chinese.
- Commit messages: conventional style (`feat: …`, `fix: …`, `docs: …`) with a short bullet body,
  ending with a blank line and the trailer `Co-Authored-By: Codex (<model>) <noreply@openai.com>`,
  where `<model>` is the model this session runs on (e.g. `Codex (gpt-5-codex)`); if you are not
  sure of the model name, write `Co-Authored-By: Codex <noreply@openai.com>`. Commit only when
  asked. No remote is configured yet.
- `Cargo.toml` deliberately has no `license` field; the user has not chosen one.
- `rust-version = "1.88"` makes Cargo pick older dependency versions; the local toolchain is 1.97.

## Roadmap

[docs/roadmap.md](docs/roadmap.md) (Chinese) is the roadmap: stages and status, key design decisions
of finished stages, scope / design / dependencies / done criteria of the next ones. Done: M0, M0.5,
M1, M2, M3, M4; next: M5 (global search and command palette). When a stage is finished, move it to "已完成" there with its design
decisions and commit, and update the status line in README.

Changing the exporter is work in another repository (`../OrcaSlicer`); confirm with the user first.

## Working with the user

- The user is a C++ developer on Snapmaker's OrcaSlicer fork and writes in Chinese; reply in Chinese.
- Do not fix, or offer to fix, OrcaSlicer's profile data (`../OrcaSlicer/resources/profiles`), even
  when the checks find real bugs there; report findings only.
- Any change in the OrcaSlicer repository (e.g. the schema exporter) needs the user's confirmation
  first. The exporter lives on its own branch, `personal/schema-export`.
