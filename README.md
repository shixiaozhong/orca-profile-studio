<div align="center">
  <img src="crates/profile-studio/resources/icon.png" alt="Orca Profile Studio" width="120" />
  <h1>Orca Profile Studio</h1>
  <p><em>A desktop tool to read, check and safely edit OrcaSlicer's system profiles</em></p>
  <p>English · <a href="README_CN.md">中文</a></p>
  <p>
    <a href="docs/user-guide.md">User guide</a> ·
    <a href="docs/reference/profile_format_spec.md">Profile format</a> ·
    <a href="docs/roadmap.md">Roadmap</a>
  </p>
  <a href="https://github.com/shixiaozhong/orca-profile-studio/actions/workflows/ci.yml"><img src="https://github.com/shixiaozhong/orca-profile-studio/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey?style=flat-square" alt="Platform">
  <img src="https://img.shields.io/badge/rust-1.95%2B-orange?style=flat-square" alt="Rust 1.95+">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="License: MIT"></a>
</div>

<!-- Screenshot: add it under docs/images/ and reference it here, e.g. the main window with the Values tab -->

> An unofficial tool maintained by an individual. It is not affiliated with or endorsed by the OrcaSlicer project, SoftFever or Snapmaker. The repository contains no files exported or copied from OrcaSlicer; the tool only reads the profile files in the folder you choose.

## Why

OrcaSlicer's system profiles (`resources/profiles`) are thousands of JSON files that inherit from each other: 57 vendors, about 7,100 presets, inheritance chains up to six levels deep, and more than 14,000 compatibility entries plus condition expressions. Finding a preset's actual value means following `inherits` from file to file. When a manifest is out of order or a parent is missing, OrcaSlicer logs a line and skips the preset, or refuses the whole vendor. Renaming one preset means updating manifests, parents and compatibility lists by hand.

Orca Profile Studio parses these files with OrcaSlicer's own loading rules and puts the effective values, the problems and the compatibility in front of you. Edits change only the values that changed, and every write is previewed and can be undone.

## Quick start

You need [Rust](https://rustup.rs) 1.95+ and a set of OrcaSlicer profiles: a source checkout, an installed OrcaSlicer, or a plain `resources/profiles` folder all work.

```bash
git clone https://github.com/shixiaozhong/orca-profile-studio.git
cd orca-profile-studio
cargo run --release -p profile-studio                    # then "Open folder…" (⌘O)
cargo run --release -p profile-studio -- ../OrcaSlicer   # or pass the folder directly
```

<details>
<summary>Build a macOS app</summary>

```bash
scripts/bundle-macos.sh             # builds target/release/bundle/Orca Profile Studio.app
scripts/bundle-macos.sh --install   # and copies it to ~/Applications/
```

The bundle is signed ad hoc for local use and is not notarized. macOS system notifications only work when the app is started from the `.app`.

</details>

<details>
<summary>Build a portable Windows zip</summary>

On Windows with the MSVC toolchain (cross-compiling from a Mac is not supported):

```powershell
powershell -ExecutionPolicy Bypass -File scripts\package-windows.ps1
```

This builds `target\release\package\orca-profile-studio-<version>-windows-<x64|arm64>.zip`. Unzip it and run `Orca Profile Studio.exe`; nothing is installed. The executable is not signed, so SmartScreen may ask you to confirm with "More info" → "Run anyway".

</details>

<details>
<summary>Command-line tool</summary>

```bash
cargo run --release -p profile-tool -- --profiles ../OrcaSlicer check
```

`--profiles` can also come from the `ORCA_PROFILES_DIR` environment variable.

</details>

## Features

- **Effective values and their source**: see which level of the inheritance chain sets each key, what every level wrote, and what was overridden
- **Structural checks**: 20+ kinds of problems in manifests, inheritance, duplicates, printer models and nozzle variants, compatibility conditions and file encoding, ranked by how OrcaSlicer actually behaves when loading; loading never stops at the first error
- **Compatibility**: which printers each filament and process can be selected on, and why, computed with OrcaSlicer's rules and matched against OrcaSlicer's own verdicts on about 4 million combinations with zero differences
- **Global search**: ⌘K searches preset names, option keys and the values files set, e.g. `nozzle_temperature 270`
- **Used by**: what inherits from a preset and which compatibility lists, default presets and printer models name it, own or inherited, one click away
- **Side-by-side comparison**: compare the effective values of up to eight presets of the same kind, such as one filament's nozzle variants, across vendors and including unsaved edits
- **Format-preserving edits**: saving replaces only the changed values and keeps indentation, array layout, BOM and line endings; undo, redo and a preview before every save
- **Batch edits**: set or remove a key, add or remove array items across many presets with a per-preset preview and group undo; promote a value shared by siblings to their parent in one step
- **Preset and printer model management**: create, copy (including whole groups), rename and delete, with manifests, parents and references updated for you; writes are transactional and roll back on failure
- **Any version**: open any folder, including an OrcaSlicer checkout, an installation or a `.app`; use several windows to look at versions side by side
- **Command line**: `check`, `resolve`, `search`, `compat` and more; `check` fits straight into CI
- **English and Chinese**: follows the system language or a manual choice; light, dark and system themes

## Platforms

| Platform | Desktop app | CLI | Package |
|---|---|---|---|
| macOS | ✅ | ✅ | `.app` (`scripts/bundle-macos.sh`) |
| Windows | ✅ | ✅ | portable zip (`scripts/package-windows.ps1`) |
| Linux | — | ✅ | — |

CI runs the tests and the real-data regression on all three; the desktop UI tests run on macOS and Windows.

## Usage

| Action | macOS | Windows |
|---|---|---|
| Open folder | `⌘O` | `Ctrl+O` |
| Search and commands | `⌘K` | `Ctrl+K` |
| Save (with preview) | `⌘S` | `Ctrl+S` |
| Undo / Redo | `⌘Z` / `⇧⌘Z` | `Ctrl+Z` / `Ctrl+Shift+Z` |
| New window | `⌘N` | `Ctrl+N` |
| Close window / Quit | `⌘W` / `⌘Q` | `Ctrl+W` / `Ctrl+Q` |

| Command | What it does |
|---|---|
| `stats` | Counts of vendors, models and presets, inheritance depth, problems by code |
| `check` | Lists problems, filtered by vendor and severity; exits with 1 when there are errors |
| `resolve <preset>` | The inheritance chain and every key's effective value |
| `search <query>` | Searches presets, option keys and the values files set |
| `compat <preset>` | Compatible printers and why, or the processes and filaments a printer accepts |
| `refs <name>` | What inherits from or names a preset, or the printers of a printer model |
| `compat-check <table>` | Compares compatibility with a table exported by OrcaSlicer |

The [user guide](docs/user-guide.md) covers every feature of the app and every command.

## Design principles

- **Mirror OrcaSlicer's loading rules**: vendor, section and manifest order, whole-key overrides, the single cross-vendor parent source and BOM handling all follow OrcaSlicer; the rules and their source references are in the [profile format notes](docs/reference/profile_format_spec.md) (in Chinese)
- **Analyze the profile files only**: no built-in copy of OrcaSlicer's option definitions (types, defaults, ranges), so the tool is not tied to one OrcaSlicer version
- **Preview before writing**: every change goes into an undoable session and is written transactionally only after you confirm
- **Report, never repair**: problems are listed, never fixed automatically

## FAQ

- **Will it change my OrcaSlicer checkout?** Only when you confirm a save or a file operation, and only inside the folder you opened. Use it on a Git checkout so `git diff` shows every change and Git can undo it.
- **Why do some keys have no value?** A key that no file in the inheritance chain sets takes its default from OrcaSlicer's C++ sources. This tool only reads profile files, so such keys are not shown.
- **Does it report unknown keys, wrong types or out-of-range values?** No. That needs OrcaSlicer's option definitions, which the tool deliberately does without; it checks structure and compatibility.
- **Does it handle user presets or SLA printers?** No, only the system profiles under `resources/profiles`.
- **Is there a Linux desktop app?** Not yet; on Linux only the command-line tool is available.
- **How do I compare the profiles of two OrcaSlicer versions?** Press ⌘N for a new window and open a different folder in each.

## Docs

- [User guide](docs/user-guide.md) ([中文](docs/user-guide_CN.md)): every feature of the app, settings, Windows notes, the command line and the problem types
- [Profile format](docs/reference/profile_format_spec.md) (in Chinese): the format and loading rules of OrcaSlicer's profiles, with source references
- [Roadmap](docs/roadmap.md) (in Chinese): the scope, key design decisions and done criteria of each stage
- [AGENTS.md](AGENTS.md): development conventions and implementation notes; `CLAUDE.md` has the same content

## Development

```bash
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
python3 -I docs/reference/profile_format_stats.py ../OrcaSlicer/resources/profiles --check
```

```
crates/
├── profile-core/    # loading, inheritance, checks, compatibility, search, format-preserving writes, transactions
├── profile-tool/    # command-line tool
└── profile-studio/  # desktop app (GPUI + gpui-kit)
scripts/             # macOS / Windows packaging and icons
docs/                # user guide, roadmap, profile format notes and an independent Python stats script
```

The real-data regression tests read `ORCA_PROFILES_DIR` (by default `../OrcaSlicer/resources/profiles` next to this repository) and expect the numbers of OrcaSlicer `953509c23f`; the Python script checks the same statistics with an independent implementation. [CI](.github/workflows/ci.yml) runs formatting, clippy, all tests and the real-data regression on Linux, macOS and Windows, with the profiles sparse-checked out from a public OrcaSlicer fork at a pinned commit; missing data fails the run instead of skipping it.

## Background

OrcaSlicer has a lot of profile files, layered through inheritance. Whether I wanted to look up one value or change a group of presets, I ended up jumping between piles of JSON files: tiring to read and easy to get wrong when editing. So I set out to build a tool for exactly that: reading, checking and editing.

I had also wanted to try [GPUI](https://gpui.rs) for a while, so this became my practice project: it started as a core library and a command-line tool without any UI, and grew step by step into a desktop app with editing, batch operations, file management, two languages and packages for two platforms. The trade-offs of each stage are recorded in the [roadmap](docs/roadmap.md).

## Acknowledgements

- [OrcaSlicer](https://github.com/SoftFever/OrcaSlicer): the profile format and loading rules this tool analyzes come from it
- [GPUI](https://gpui.rs): Zed's UI framework, the foundation of the desktop app
- [GPUI Kit](https://gpui-kit.com): the UI components, and its Coding Guides and Design Guides

## License

MIT License. Feel free to use it and to contribute.
