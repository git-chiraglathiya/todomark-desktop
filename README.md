# TodoMark Desktop (Tauri)

TodoMark is a desktop Markdown viewer + todo board built with Tauri.

It keeps feature parity with the VS Code extension UI:
- Todo list mode (`all`, `pending`, `completed`)
- Full markdown mode (tables, emoji, KaTeX, Mermaid)
- Section list mode (headings containing todos)
- Sticky header toggle
- Radial theme/color picker
- Click-to-toggle markdown checkboxes with immediate autosave
- Auto-refresh when the markdown file changes externally

## Project Location

`/Users/chirag/WORK/todomark/todomark-desktop`

## Tooling (mise)

This project pins local tools in `mise.toml`:
- `rust = 1.94.0`
- `cargo:tauri-cli = 2.10.1`

Use `mise exec -- ...` for Tauri/Rust commands.

## Run In Dev

```bash
cd /Users/chirag/WORK/todomark/todomark-desktop
npm install
mise exec -- npm run tauri:dev
```

## Build macOS Bundles

```bash
cd /Users/chirag/WORK/todomark/todomark-desktop
mise exec -- npm run tauri:build
```

Build outputs:
- `.app`: `src-tauri/target/release/bundle/macos/TodoMark.app`
- `.dmg`: `src-tauri/target/release/bundle/dmg/TodoMark_0.1.0_aarch64.dmg`

## Markdown File Association

The app bundle registers `.md` association in macOS metadata (`CFBundleDocumentTypes`) with role `Editor`.

To set TodoMark as default for `.md` files:
1. In Finder, right-click any `.md` file.
2. Choose `Get Info`.
3. Under `Open with`, select `TodoMark`.
4. Click `Change All...`.

## Runtime Behavior

- Launch with no file: opens native markdown file picker.
- Launch with one or more `.md` paths: opens one TodoMark window per file.
- App already running and new `.md` opened: routes to same process and opens/focuses the corresponding window.
