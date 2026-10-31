# User guide

English · [中文](user-guide_CN.md)

This guide covers every feature of the Orca Profile Studio desktop app and the command-line tool. For the project overview, installation and design principles, see the [README](../README.md).

Shortcuts are written for macOS; on Windows use Ctrl instead of ⌘. Toolbar tooltips show the shortcut for the current platform.

## Contents

- [Opening a profiles folder](#opening-a-profiles-folder)
- [Browsing](#browsing)
- [Editing and saving](#editing-and-saving)
- [Comparing presets](#comparing-presets)
- [Issues panel](#issues-panel)
- [Compatibility](#compatibility)
- [Used by](#used-by)
- [Search and command palette](#search-and-command-palette)
- [Batch edits](#batch-edits)
- [Promoting to the parent](#promoting-to-the-parent)
- [Managing presets](#managing-presets)
- [Managing printer models](#managing-printer-models)
- [Language, theme and windows](#language-theme-and-windows)
- [Settings and startup](#settings-and-startup)
- [Windows notes](#windows-notes)
- [Command-line tool](#command-line-tool)
- [Problem types](#problem-types)

## Opening a profiles folder

- "Open folder…" next to the title in the toolbar (⌘O, also in the File menu) switches to any folder at any time: another OrcaSlicer checkout, another branch, or the profiles that ship with an installed version. The current path is shown next to it; hover to see all of it.
- If the chosen folder is not a profiles folder itself, the app looks inside it for `resources/profiles` (an OrcaSlicer source checkout, or a Windows/Linux installation), `profiles` (the `resources` folder) and `Contents/Resources/profiles` (a macOS `.app`). If none of them exists, it says the folder has no vendor profiles instead of loading an empty tree. A profiles folder has at least one `<Vendor>.json` beside a `<Vendor>` folder.
- By default the app starts on a start page with "Open folder…", "Open last folder" and the "Open the last folder at startup" checkbox.
- To look at two versions side by side, press ⌘N for a new window (empty by default) and open the other folder there.
- If there are unsaved edits, opening another folder asks whether to discard them first; cancelling the folder choice or the confirmation keeps everything as it is.

## Browsing

- **Navigation tree** (left): vendors → printer models (→ their printer presets), filaments, processes and templates. The tree is virtualized and renders only the visible rows. Presets with warnings or worse show a warning icon.
- **Details** (right): for a vendor, model or preset, the file information, the inheritance chain, the effective values of key fields and its problems.
- **Values tab**: a table of every key that any file in the inheritance chain sets, with its effective value and its source (this file or the n-th ancestor). Keys that no file sets are not shown: their defaults live in OrcaSlicer's source code, which this tool does not read.
  - Filter by key, or show only the keys this file sets; click a column header to sort.
  - Select a row to see every level that sets the key and its value there, with overridden values struck through, along with related problems (such as an override equal to the inherited value).
- **Background loading**: loading runs on a background thread. During a refresh the previous view stays usable and is replaced in one step when the new data is ready. A newer load supersedes an older one; if a refresh fails, the previous view and your edits are kept and you can retry.

## Editing and saving

- Select a row in the Values tab to edit the preset's own value. For an inherited value, click "Override" first; "Reset to inherited" removes this file's value so the inherited one applies again.
- The editor follows the shape of the value: a single-line input for a string, a multi-line box with one item per line for an array, and verbatim editing for text with line breaks (such as G-code). Values are not checked against OrcaSlicer's option types.
- Edited keys and presets show a pencil icon. ⌘Z / ⇧⌘Z undo and redo. ⌘S or "Save…" opens a save preview: Enter writes the files, Escape cancels. Only applied edits are saved.
- Saving replaces only the changed values and keeps the file's indentation, array layout, BOM, line endings (CRLF/LF) and final newline. A file changed by someone else since loading is not overwritten.
- After saving, the data refreshes in the background: presets that inherit from the saved one (including other vendors' presets that inherit from the shared filament library), search, problems and compatibility all update. Reload keeps unsaved edits, undo/redo history, the selection, the tab and the Values filter in the same way.
- If a file with edits or history is changed, deleted or becomes unreadable outside the app, the refresh pauses: the previous view and all edits are kept and the window says so. "Review files…" lists the files. Restore them and Reload again, or explicitly discard the edits and history of those files and take the version on disk; edits in other files are kept.

## Comparing presets

- Compare up to eight presets of the same kind, from any vendor and including templates. Start from the Values tab with "Compare with…", or tick presets in batch edit or preset management and click "Compare selected".
- The comparison fills the window: one row per key, one column per preset with its effective value. The first preset is the base; values that differ from it are highlighted. A key a preset does not set shows as "(missing)".
- Add presets with "Add a preset of the same kind…" and remove one with its × button; removing the base makes the next preset the base.
- Only keys whose values are not all equal are shown by default; turn off "Only differences" to see every key. Filter by key or sort by any column; select a row to see every preset's inheritance levels for that key below.
- The comparison includes unsaved edits, noted next to each preset. Values are compared as written: a string versus an array, a different array order or a different spelling of the same number all count as differences.
- The comparison is read-only. "Back" returns to where you opened it (Values, batch edit or preset management) with your edits, filters and selection unchanged; opening it again from the same preset's Values tab keeps the presets you had added.

## Issues panel

- Click "Issues" in the toolbar, or run "Show issues" from ⌘K, to open a resizable list of problems at the bottom of the window.
- It shows warnings and above by default; filter by minimum severity, problem code, vendor or text.
- Selecting a row (by click or ↑↓) jumps to the preset. If the problem is about a key, the Values tab opens on that key while focus stays in the list.

## Compatibility

The preset's Overview has a "Compatibility" section:

- for filaments and processes, the condition expression (if any) and the compatible printers, with the reason for each (condition, list or no restriction);
- for printers, how many processes and filaments they accept.

The rules match OrcaSlicer: condition expressions, `compatible_printers` lists and the shared filament library's exclusions, as described in section 6 of the [profile format notes](reference/profile_format_spec.md) (in Chinese).

## Used by

The Overview of a preset has a "Used by" section listing everything that inherits from it or names it:

- the number of descendants, and the children that inherit from it directly (including other vendors' filaments that inherit from the shared filament library);
- the presets whose `compatible_printers` (printers), `compatible_prints` or `default_print_profile` (processes), or `default_filament_profile` (filaments) names it, and the printer models whose `default_materials` lists a filament.

References are found in effective values: a filament that inherits its `compatible_printers` names the same printers as the ancestor that sets it, and is marked "inherited from …". Renaming only rewrites the files that set the value themselves. Templates are never named by these keys, so only their children are listed; when another selectable preset of the same kind has the same name, a warning says the references may mean either.

Click a reference to open it: a child opens on its Overview, a value reference on the Values tab at that key. Each group shows twelve rows, then "Show all". A printer model's Overview lists the printers whose `printer_model` names it, including the templates that set it. The list comes from the loaded files: unsaved edits are not included until they are saved.

## Search and command palette

- Press ⌘K or click "Search…" in the toolbar.
- Search presets (name, alias, `setting_id`, `filament_id`), option keys and the values files set. Every word must match: `nozzle_temperature 270` finds the files that set it to 270.
- Enter jumps to the selected result: a preset is revealed and selected in the tree; a value or key opens the Values tab filtered to that key.
- Commands run from here too: reload, open a folder, new window, switch between Overview and Values, language, theme, startup behavior and more.
- Only loaded data is searched. The index updates after saving; unsaved edits are not searched.

## Batch edits

- "Batch edit" in the toolbar or ⌘K opens a separate view.
- Filter by kind, vendor, text, printer model and problem code, tick presets of one kind, then choose a key and an operation: Set value, Remove key (inherit), Add item or Remove item. The keys offered are the ones presets of that kind actually set.
- Item operations work on array values (such as `compatible_printers`): other items in each preset's own list are kept and an existing item is not added twice. An inherited list that needs a change becomes a local override, clearly marked in the preview. A child that already inherits the target value from a selected parent is skipped, so no redundant override is written.
- Preview shows the old and new values, their sources, newly redundant overrides and how many descendants are affected; click a preset name for the full details. "Apply batch" changes only the edit sessions; nothing is written until you confirm "Save…". Changing the input, the selection or the edits, or reloading, requires a new preview.
- "Undo batch" / "Redo batch" or ⌘Z / ⇧⌘Z undo and redo the whole group; undoing one of its steps in the Values tab also undoes the whole group.

## Promoting to the parent

- In batch edit, select at least two presets of the same kind with the same parent, then choose a key and "Promote to parent". Every selected preset must set exactly the same value.
- The preview lists the parent, the selected children and every unselected descendant whose effective value would change. Unselected descendants are not rewritten, and the summary states how many are affected. Promoting into a parent of another vendor (including the shared filament library) is refused.
- Applying writes the value to the parent and removes the children's overrides; the whole change undoes and redoes in one step.
- An override equal to the inherited value (`redundant-override`) is reported at info level: choose "All severities" in the issues panel and filter by that code. It is only reported, never removed automatically.

## Managing presets

- Click "Presets" in the toolbar, or run "Manage presets" from ⌘K.
- Create, copy, copy in groups, rename and delete presets. The table at the top filters by kind, vendor, text and printer model, and you tick the presets to work on.
- A group copy derives new names and file names from one replacement rule (for example `0.4 nozzle` → `0.2 nozzle`); when a parent in the group is copied too, the new children inherit from the new parent. You can also set a few keys on every copy (one `key = value` per line).
- Renaming also updates the manifest, `inherits`, every vendor's `compatible_printers` / `compatible_prints`, default processes and filaments, and printer models' `default_materials`; optionally the old name is recorded in `renamed_from`.
- Every operation first previews all file changes and notes, then writes them as one batch that rolls back on failure. It refuses to write if it would add an error or fatal problem, delete a parent that still has children, or empty a compatibility list. Preset management is unavailable while there are unsaved edits.

## Managing printer models

- Click "Machine models" at the top of preset management, or open it from ⌘K.
- Create a model from scratch or copy an existing one, and edit its name, model ID, semicolon-separated nozzle variants and default materials, and the bed, texture and hotend resource paths.
- Changes and renames update the vendor manifest and the vendor's `printer_model` references. A model still referenced by printers cannot be deleted, and removing a nozzle variant that is in use is refused. Resource files are only checked and noted, never copied or deleted.
- Preview first, then Enter writes everything as one batch that rolls back on failure. After creating a model, use a group copy to create its printer presets and a batch edit to extend compatibility lists.

## Language, theme and windows

- **Language**: follows the system by default. Search "Language" (or "语言") in ⌘K to choose English, Chinese or the system language; all windows switch together. The app's text and problem descriptions are translated; option keys always show their raw names and values stay as written. Switching keeps unsaved edits, unapplied input, filters and selections. The translations live in [`locales/app.yml`](../crates/profile-studio/locales/app.yml).
- **Theme**: run "Theme: System / Light / Dark" from ⌘K; the system appearance is the default.
- **Windows**: ⌘N opens an independent window, and each window can open a different folder.
- **Closing and quitting**: the close button or ⌘W checks the current window, ⌘Q checks all windows; unsaved edits or unapplied input ask for confirmation first. If another window gains unsaved files while the confirmation is open, you are asked again. A forced quit or system termination cannot be intercepted.
- **Errors**: failures to read or write settings, choose a folder or open a window show a notification in the window with a Dismiss button; a failed settings write does not affect the current session. When no window can show an error (for example if the first window fails to open), the app falls back to a system notification and stderr. On macOS, system notifications only work when the app is started from the `.app`.

## Settings and startup

- Remembered settings: theme, language, the last folder, whether to open it at startup, the sidebar width, whether the issues panel is open and its height, and the window size.
- Settings file: `~/Library/Application Support/orca-profile-studio/settings.json` on macOS, `%APPDATA%\orca-profile-studio\settings.json` on Windows, and the same name under `$XDG_CONFIG_HOME` (default `~/.config`) elsewhere. The `ORCA_STUDIO_SETTINGS` environment variable chooses another path (it has no effect when the app starts from Finder or the Dock, which have no terminal environment). A damaged settings file is backed up as `settings.json.invalid` before it is overwritten.
- A folder opens at startup when:
  1. the first command-line argument names one, e.g. `cargo run --release -p profile-studio -- ../OrcaSlicer`;
  2. "Open the last folder at startup" is on (the start page checkbox, or "Startup: open the last folder / open no folder" in ⌘K): the last folder opens, or, before any folder has been opened, `../OrcaSlicer/resources/profiles` next to this repository at build time. That path is fixed when the app is built, so a `.app` copied to another machine will not find it.
- The same setting decides whether a ⌘N window starts empty or opens the last folder.
- The desktop app does not read `ORCA_PROFILES_DIR`; only the command-line tool and the tests do.

## Windows notes

- There is no menu bar on Windows: use Ctrl+N / Ctrl+W / Ctrl+Q for new window, close and quit; everything else is in the toolbar and Ctrl+K. Closing the last window quits.
- The executable is not signed, so SmartScreen may block the first run: choose "More info" → "Run anyway". Release builds open no console window.
- Paths are shown in the usual `D:\…` form. Manifest `sub_path`s are always written with `/`.
- Saving keeps each file's line endings and BOM, so Git's `core.autocrlf` setting produces no extra differences.
- Preset management installs files atomically with hard links where possible; on drives without hard links (FAT/exFAT USB drives, some network shares) it creates a new file and writes it instead, which still never overwrites an existing file.

## Command-line tool

`--profiles <folder>` selects the profiles folder; the `ORCA_PROFILES_DIR` environment variable works too. As in the app, an OrcaSlicer checkout, installation or `.app` is accepted.

```bash
export ORCA_PROFILES_DIR=../OrcaSlicer/resources/profiles
```

| Command | What it does |
|---|---|
| `stats` | Counts of vendors, models and presets of each kind, inheritance depth, value shapes, problems by code |
| `check` | Problems grouped by code; `--vendor` for one vendor, `--min` for the lowest severity, `--limit` per code |
| `resolve <preset>` | The inheritance chain and the effective value of every key a file in it sets; `--key` for one key |
| `search <query>` | The same search as ⌘K: presets, option keys and the values files set |
| `compat <preset>` | For a filament or process, the compatible printers and why; for a printer, the processes and filaments it accepts |
| `refs <name>` | What inherits from a preset or names it, as in "Used by"; for a printer model, the printers that use it |
| `compat-check <table>` | Compares compatibility with a table exported by OrcaSlicer |

```bash
cargo run --release -p profile-tool -- stats
cargo run --release -p profile-tool -- check --vendor Snapmaker --min info
cargo run --release -p profile-tool -- resolve "Generic ABS @U1 0.2 nozzle" --key temperature
cargo run --release -p profile-tool -- search "nozzle_temperature 270"
cargo run --release -p profile-tool -- compat "0.07mm DETAIL @CORE One 0.25"
cargo run --release -p profile-tool -- refs "Snapmaker U1 (0.4 nozzle)"
```

`check` exits with status 1 when a problem of `error` severity or worse is shown (adjust with `--fail-on`), so it can guard profile changes in CI before they are merged.

The table for `compat-check` is exported by `Snapmaker_Orca_compat_export` in the OrcaSlicer repository (see [`AGENTS.md`](../AGENTS.md) for the commands). On the profiles of OrcaSlicer `953509c23f`, all of about 4 million (preset, printer) pairs agree.

## Problem types

Loading never stops at the first problem. Every deviation becomes a problem with one of four severities, based on how OrcaSlicer actually behaves:

| Severity | Meaning |
|---|---|
| `fatal` | OrcaSlicer refuses to load the whole vendor |
| `error` | OrcaSlicer logs an error and carries on |
| `warning` | Loads, but is probably not what the author intended |
| `info` | For information only |

What is checked, by category:

| Category | Problems |
|---|---|
| Files | Unreadable or invalid JSON, a UTF-8 BOM, a missing `name`, metadata or values of the wrong JSON type, a `type` that does not match the section the file is listed in |
| Manifests | Malformed entries, listed files that do not exist, a manifest name that differs from the preset's name, orphan files not listed anywhere, a malformed vendor version |
| Inheritance | A parent that cannot be found, a parent listed after its child, duplicate names within a vendor or across vendors, an invalid `instantiation`, a `from` other than `system` |
| Printer models | A missing or unknown `printer_model`, a `printer_variant` that is not one of the model's nozzle variants, a model without nozzle variants, a filament without `filament_id` |
| Compatibility | `compatible_printers` naming a printer that does not exist, a condition expression that cannot be parsed or evaluated |
| Redundancy | An override equal to the inherited value (`redundant-override`, info level) |

Every problem code and its severity are listed in [`crates/profile-core/src/issue.rs`](../crates/profile-core/src/issue.rs); the loading rules are in the [profile format notes](reference/profile_format_spec.md) (in Chinese).

This tool does not read OrcaSlicer's option definitions (types, defaults, ranges), so it does not report unknown keys, wrong types or out-of-range values.
