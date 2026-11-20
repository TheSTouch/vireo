# Writing addons with Vireo's UI API

A Vireo addon is a Zed WebAssembly extension that also adds to the user
interface: commands, status bar items, panels, views, editor decorations, and
prompts. Addons are written in Rust against `zed_extension_api` and built
like any Zed extension. The UI API is extension API **v0.9.0**, which only
Vireo provides. Zed itself stops at v0.8.0.

The sample addon [`extensions/hello-ui`](../../../extensions/hello-ui) uses
every API on this page. The examples below come from it.

Two design rules shape the API:

- **No webviews.** The addon describes its views as data, and Vireo renders
  them with its own components, so addons always match the theme.
- **Calls are synchronous** for the addon, as Zed's APIs are. A quick pick or
  an input box blocks the addon until the user answers.

The UI API needs no capability in `extension.toml`.

## Setting up an addon

An addon is an ordinary extension directory:

```
hello-ui/
  extension.toml
  Cargo.toml
  src/hello_ui.rs
```

`Cargo.toml` builds a `cdylib` against the Vireo extension API. Vireo does
not publish its fork of `zed_extension_api`, so depend on it by path or by
git:

```toml
[lib]
path = "src/hello_ui.rs"
crate-type = ["cdylib"]

[dependencies]
zed_extension_api = { git = "https://github.com/TheSTouch/vireo" }
```

`src/hello_ui.rs` implements `zed_extension_api::Extension` and registers
the type:

```rust
use zed_extension_api::{self as zed, addon::{editor, ui, workspace}};

struct HelloUi { /* … */ }

impl zed::Extension for HelloUi {
    fn new() -> Self { /* … */ }
}

zed::register_extension!(HelloUi);
```

To try it, open the command palette and run **zed: install dev extension**,
then pick the directory. To package it, build it with the extension CLI:

```sh
cargo run -p extension_cli -- \
  --source-dir extensions/hello-ui \
  --output-dir /tmp/hello-ui-out \
  --scratch-dir /tmp/hello-ui-scratch
```

The CLI accepts an extension that only contributes UI: `[commands]` or
`[panels]` count as something it provides.

## Commands

Declare commands in `extension.toml`:

```toml
[commands.say-hello]
title = "Say Hello"
description = "Shows a notification and counts how often it ran."
```

Each command appears in the command palette as `<addon name>: <title>`, here
**Hello UI: Say Hello**. Vireo calls `run_command` with the command ID:

```rust
fn run_command(&mut self, command_id: &str, args: Vec<String>) -> zed::Result<()> {
    match command_id {
        "say-hello" => self.say_hello(),
        _ => Err(format!("unknown command `{command_id}`")),
    }
}
```

An error is shown to the user as a notification.

Users bind commands in their keymap with the `addon::RunCommand` action.
`args` is optional:

```json
{
  "bindings": {
    "cmd-alt-h": ["addon::RunCommand", { "extension": "hello-ui", "command": "say-hello" }]
  }
}
```

Commands run from a tree item, such as on click or from a context menu,
receive the item ID as their only argument.

## Status bar items

```rust
ui::set_status_item("hello", "Hello UI: 3", Some("Say hello"), Some("say-hello"));
ui::remove_status_item("hello");
```

Items show on the right of the status bar. The arguments are the item ID,
its text, a tooltip, and a command to run on click. Setting an item with an
existing ID updates it in place. Addons may set items from `new()`. Items go
away when the addon unloads.

## Panels and tree views

Declare a panel in `extension.toml`:

```toml
[panels.files]
title = "Hello Files"
icon = "folder"      # a Vireo icon name
dock = "left"        # left (default), right or bottom
view = "tree"        # tree (default) or custom

[[panels.files.context_menu]]
command = "open-item"
when = "file"        # only on items whose context value is "file"

[[panels.files.context_menu]]
command = "show-item-path"
```

The panel gets a dock button: in the activity bar for the left dock, or in
the status bar for the others. The command palette also lists
**Hello UI: Toggle Hello Files**, which is the `addon::TogglePanel` action
with `{ "extension": "hello-ui", "panel": "files" }`. A workspace shows up to
8 addon panels; a ninth is not shown and a warning is logged.

The panel ID is also the view ID. A tree view asks the addon for items with
`tree_children`, where `parent` is `None` for the root items:

```rust
fn tree_children(&mut self, view_id: &str, parent: Option<String>) -> zed::Result<Vec<ui::TreeItem>> {
    let Some(parent) = parent else {
        return Ok(workspace::workspace_folders()
            .into_iter()
            .map(|folder| tree_item(&folder, true))
            .collect());
    };
    // … the entries of `parent` …
}
```

A `TreeItem` has:

- `id`: unique within the view;
- `label`, plus an optional `description` shown dimmed after it;
- `icon`: a Vireo icon name;
- `collapsible`: whether it has children;
- `command`: run with the item ID when the item is clicked;
- `context_value`: matched against the `when` of the context menu entries.

`ui::refresh_view("files")` makes Vireo ask again. Expanded items stay
expanded.

## Custom views, in panels and editor tabs

A custom view is a tree of elements that the addon returns from
`render_view`. Build it with `ui::Element`:

```rust
fn render_view(&mut self, view_id: &str) -> zed::Result<ui::ViewTree> {
    Ok(Element::scroll("root", [
        Element::label("greeting", self.message()),
        Element::row("actions", [
            Element::button("hello", "Say hello"),
            Element::button("pick", "Pick greeting"),
        ]),
        Element::text_input("greeting-input", self.greeting.clone(), Some("Type a greeting".into())),
        Element::checkbox("shout", "Shout", self.shout),
        Element::divider("divider"),
        Element::list("folders", folders),
    ])
    .into_view_tree())
}
```

Elements:

- layout: `column`, `row`, `scroll` (a column that scrolls) and `list`;
- content: `label`, `icon`, `button`, `text_input`, `checkbox` and `divider`.

Element IDs must be unique within the view. Nesting stops at 64 levels.

Events come back to `handle_view_event` with the element's ID:

- `Click`, from buttons;
- `Change(text)` as the user types in a text input, and `Submit(text)` on
  Enter;
- `Toggle(checked)`, from checkboxes.

Vireo renders the view again after each event and after
`ui::refresh_view(view_id)`. A text input keeps what the user is typing while
it has focus.

```rust
fn handle_view_event(&mut self, view_id: &str, element_id: &str, event: ui::ViewEvent) -> zed::Result<()> {
    match (element_id, event) {
        ("hello", ui::ViewEvent::Click) => self.say_hello()?,
        ("shout", ui::ViewEvent::Toggle(checked)) => self.shout = checked,
        _ => {}
    }
    Ok(())
}
```

A custom view shows in a panel declared with `view = "custom"`, or in an
editor tab:

```rust
ui::open_view_tab("dashboard", "Hello Dashboard");
```

Opening the same view again activates its tab.

## Editor decorations

```rust
editor::set_decorations(&path, "hello", &[
    Decoration::Highlight((whole_line, Some("#ffd70040".into()))),
    Decoration::Underline((selection, Severity::Warning, "Selected text".into())),
    Decoration::InlayHint((Position { line, column: 0 }, "hi ".into())),
    Decoration::Hover((whole_line, "**Hello** from Hello UI".into())),
    Decoration::GutterIcon((line, Severity::Info, "Hello UI was here".into())),
]);
editor::clear_decorations("hello");
```

Decorations belong to a **collection**, named by the addon (here `hello`):

- `set_decorations(path, collection, decorations)` replaces the collection's
  decorations on one file, given by its absolute path. An empty list removes
  them.
- `clear_decorations(collection)` removes the collection from every file.

Collections are per addon, so addons never touch each other's decorations.
Decorations go away when the addon unloads, and they follow the file into
every editor that opens it.

| Decoration | Shows |
| --- | --- |
| `Highlight((range, color))` | A background highlight. `color` is `#rrggbb` or `#rrggbbaa`. A collection's highlights share the color of its first colored one; use several collections for several colors. |
| `Underline((range, severity, message))` | A wavy underline in the theme's color for the severity, with the message on hover. |
| `InlayHint((position, text))` | Inline text, styled like inlay hints. |
| `Hover((range, markdown))` | Markdown shown when hovering the range. |
| `GutterIcon((line, severity, tooltip))` | A gutter marker in the severity's color, with the tooltip on hover over the line. |

Positions are zero-based lines and zero-based **byte** columns. Positions
past the end of a line or file are clipped.

## Workspace services

| Function | Does |
| --- | --- |
| `editor::get_active_editor()` | The active editor's absolute `path` (if any), newest `selection` and `selected_text`. |
| `editor::get_active_editor_text()` | The active editor's full text. |
| `editor::open_file(path, position)` | Opens a file and moves the cursor to `position`. |
| `workspace::show_notification(level, message)` | A notification; `level` is `Info`, `Warning` or `Error`. |
| `workspace::quick_pick(items, placeholder)` | A fuzzy-filtered list of `QuickPickItem { id, label, description }`. Returns the picked item's ID, or `None` when dismissed. |
| `workspace::input_box(prompt, value)` | A line of text, starting from `value`, or `None` when dismissed. |
| `workspace::workspace_folders()` | The absolute paths of the workspace's folders. |
| `workspace::read_file(path)` | A file's UTF-8 text. |
| `workspace::read_dir(path)` | A directory's entries; subdirectory names end with `/`. |
| `workspace::watch_path(path)` | Watches a file or directory; changes reach `files_changed(watch_id, paths)`. |
| `workspace::unwatch_path(watch_id)` | Stops a watch. |
| `workspace::get_addon_settings()` | The addon's settings as JSON, or `None`. |
| `workspace::storage_get(key)` / `storage_set(key, value)` | Values that persist across restarts; `storage_set(key, None)` removes one. Each addon has its own storage. |

All these functions work on the active workspace. Paths are absolute.
`read_file`, `read_dir` and `watch_path` only accept paths inside the
workspace's folders: the path is resolved first, so `..` and symlinks
cannot reach outside them. A watch ends when the addon unloads.

Users configure addons under `addons` in their settings, keyed by
extension ID. For example:

```json
{
  "addons": {
    "hello-ui": { "greeting": "Bonjour" }
  }
}
```

With this, `get_addon_settings()` returns `{"greeting":"Bonjour"}` to
hello-ui.

## Reference

- WIT definitions: `crates/extension_api/wit/since_v0.9.0/`.
  - `ui.wit`: status items, tree items, custom views, view tabs.
  - `editor.wit`: decorations, the active editor, opening files.
  - `workspace.wit`: notifications, prompts, files, settings, storage.
  - `extension.wit`: the exports `run-command`, `tree-children`,
    `render-view`, `handle-view-event` and `files-changed`.
- Rust API: the `zed_extension_api::addon` module, and the `run_command`,
  `tree_children`, `render_view`, `handle_view_event` and `files_changed`
  methods of `zed_extension_api::Extension`.
- Implementation: the `addon_ui` crate (UI) and
  `extension_host/src/wasm_host/wit/since_v0_9_0.rs` (host functions).
