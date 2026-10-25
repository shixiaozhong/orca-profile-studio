# CLAUDE.md

Guidance for Claude Code when working in this repository.

## Overview

orca-profile-studio browses, validates and (later) edits OrcaSlicer's system profiles
(`resources/profiles`). It is also a GPUI learning project. Rust workspace, edition 2024.

```
crates/
├── profile-core/    # loading, inheritance resolution, validation; no GUI dependencies
├── profile-tool/    # CLI: stats / check / resolve
└── profile-studio/  # gpui-kit desktop app
docs/reference/      # profile format spec + Python reference stats script
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
- Loading never aborts: every deviation is an `Issue` with a `Severity`:
  `Fatal` = OrcaSlicer would refuse to load the whole vendor, `Error` = OrcaSlicer logs and continues,
  `Warning` = loads but is probably unintended, `Info` = informational.
- `crates/profile-core/tests/orca_profiles.rs` asserts exact counts for OrcaSlicer commit
  `953509c23f`. When the profile data changes, update the numbers deliberately; the reference
  script `docs/reference/profile_format_stats.py` must agree.

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
  .aria_label(..)` so tests can query them; tree rows are found by their row index.
- `TreeItem` holds `Rc` and is not `Send`: do heavy work (repository load, `Catalog::new`) on the
  background executor, build tree items on the foreground.

## Conventions

- `rustfmt.toml`: `max_width = 120`, `use_small_heuristics = "Max"`. Run `cargo fmt` and keep
  clippy clean.
- Identifiers, comments, CLI and UI strings in English; README is in Chinese.
- Commit messages: conventional style (`feat: …`), ending with the Co-Authored-By line from the
  session's attribution reminder. Commit only when asked. No remote is configured yet.
- `Cargo.toml` deliberately has no `license` field; the user has not chosen one.
- `rust-version = "1.88"` makes Cargo pick older dependency versions; the local toolchain is 1.97.

## Roadmap

| Stage | Content |
|---|---|
| M0 ✅ | core: manifest parsing, inheritance resolution, validation; CLI |
| M1 ✅ | gpui-kit app: vendor → models / filaments / processes tree, background loading, details pane |
| M0.5 | export `schema.json` from OrcaSlicer: per option type, default, min/max, enum values, label, tooltip, category, mode, nullable, plus the per-type key sets `Preset::print_options()`, `filament_options()`, `printer_options()` |
| M2 | inheritance view: every key's effective value and the layer it comes from |
| M3 | edit form with dirty marks, undo/redo, format-preserving save |
| M4 | side-by-side preset diff |
| M5 | global search and command palette |
| M6 | issues panel; evaluate `compatible_*_condition` expressions |
| M7 | dock layout, themes, multiple windows |
| Stretch | 2D G-code layer viewer |

M0.5 needs a small C++ program linked against OrcaSlicer's libslic3r that walks `print_config_def`.
It naturally lives in the OrcaSlicer repo (e.g. next to
`src/dev-utils/Snapmaker_Orca_profile_validator.cpp`); the generated `schema.json` is copied here.
That is work in another repository, so confirm with the user before changing OrcaSlicer.
