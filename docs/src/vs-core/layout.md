# VS Code's layout and look

Vireo arranges its window and draws its chrome like VS Code's default
Dark Modern setup, so people coming from VS Code find things where they
expect them. Zed's features are all still there. Zed-only items are
hidden by default, and a setting brings each one back; this page lists
them.

Vireo does not use the VS Code name or logo anywhere in its interface.

## What matches VS Code

| Area | What you get |
| --- | --- |
| Defaults | VS Code keymap (`base_keymap: "VSCode"`), Menlo 12 px editor font at 1.5 line height, 13 px UI font, minimap always on, Explorer, Source Control, Run and Debug and Outline on the left, the agent on the right, terminal at the bottom |
| Themes | Ports of VS Code's default themes: **Dark Modern** (default dark), **Light Modern** (default light), Dark+ and Light+. They follow the system appearance |
| Icons | Codicons for the interface, and the **Seti** file icon theme (Seti Light in light mode) |
| Activity bar | A 48 px bar at the window's left edge: Explorer, Search, Source Control, Run and Debug, Extensions, then any other left-dock view, with badges. Clicking the active view collapses the side bar. Accounts and Manage menus at the bottom |
| Title bar | The command center in the middle (back, forward and a search box naming the folder, which opens Quick Open) and toggles for the primary side bar, the panel and the secondary side bar |
| Tabs | 35 px tabs with the theme's tab colors, a 1 px accent line on the active tab, and its close button always visible |
| Breadcrumbs | One crumb per folder (`src › util › mod.rs`), the file's icon before its name, then symbols |
| Bottom panel | A strip with Problems and Terminal (plus any other bottom view), the active one on a rounded fill, then Maximize Panel Size and Hide Panel. **Problems** is the project diagnostics view, docked; `cmd-shift-m` (`ctrl-shift-m`) toggles it |
| Status bar | 22 px tall. Left: remote indicator, branch with `*` for changes, sync (behind↓ ahead↑), errors and warnings. Right: `Ln 1, Col 1`, encoding, line endings, language |
| Explorer | An "Explorer" title row with a "..." menu, chevrons instead of folder icons, 8 px indent, 22 px rows |
| Welcome tab | Start (New File, Open Folder, Clone Git Repository, Connect to), Recent folders and Walkthroughs, branded Vireo |

## What still differs

- **Activity bar:** views can't be reordered by dragging or hidden from a right-click menu.
- **Title bar:** there is no "Customize Layout" button, and the command center has no dropdown of recent commands.
- **Tabs and breadcrumbs:** the breadcrumb row is Zed's 32 px toolbar, with its buttons on the right; VS Code's is 22 px. The tab bar keeps Zed's new file, split and zoom buttons where VS Code shows split and "...".
- **Bottom panel:** there is no Output view, Debug Console or Ports tab. The debugger's console lives in the Run and Debug view. The terminal keeps its own tab row under the strip. Problems shows Zed's diagnostic excerpts, not VS Code's tree of files.
- **Status bar:** there is no indentation item ("Spaces: 4") and no notification bell. There is no Publish Branch button when a branch has no upstream. The remote indicator shows only its icon.
- **Explorer:** the root folder is a plain row, not a collapsible section header. There are no Outline and Timeline sections; the outline is its own view in the activity bar. Indent guides always show; VS Code shows them on hover.
- **Welcome tab:** there is no "Show welcome page on startup" checkbox; use `restore_on_startup`. Walkthroughs open the matching page, not guided steps.
- **Where Vireo follows today's VS Code rather than older screenshots:** panel tabs and the Explorer title are in title case ("Problems", "Explorer"), not uppercase.

## Bringing back Zed's own interface

Each of these is a setting in `settings.json`:

| Zed item | Setting |
| --- | --- |
| Sign-in button in the title bar | `"title_bar": { "show_sign_in": true }` |
| User menu (avatar, plan chip) in the title bar | `"title_bar": { "show_user_menu": true }` |
| Project name and branch buttons in the title bar | `"title_bar": { "show_project_items": true, "show_branch_name": true }` |
| Onboarding banners | `"title_bar": { "show_onboarding_banner": true }` |
| Collaboration (people in the call, call controls) | `"title_bar": { "show_collaboration": true }` |
| No command center or layout toggles | `"title_bar": { "show_command_center": false, "show_layout_controls": false }` |
| Dock buttons in the status bar | `"status_bar": { "panel_buttons": true }` |
| Project search button in the status bar | `"search": { "button": true }` |
| Language servers button in the status bar | `"global_lsp_settings": { "button": true }` |
| No remote indicator or branch item | `"status_bar": { "remote_indicator": false, "branch_button": false }` |
| Folder icons in the project panel | `"project_panel": { "folder_indicator": "icon" }` (or `"both"`) |
| Zed's indentation in the project panel | `"project_panel": { "indent_size": 20 }` |
| No "Explorer" title row | `"project_panel": { "show_title": false }` |
| Zed's keymap | `"base_keymap": "Zed"` |
| Zed's fonts | `"buffer_font_family": ".ZedMono", "buffer_font_size": 15, "buffer_line_height": "comfortable", "ui_font_family": ".ZedSans", "ui_font_size": 16` |
| Zed's themes and icons | `"theme": { "mode": "system", "light": "One Light", "dark": "One Dark" }`, `"icon_theme": "Zed (Default)"` |
| Panels on Zed's sides | each panel's `dock` setting, for example `"git_panel": { "dock": "right" }` |

The VS Code settings importer also reads `window.commandCenter` and
`workbench.layoutControl.enabled`.

## Maintaining the ports

The themes and icons are generated, not hand-edited:

- `script/import-vscode-themes` turns VS Code's theme files into `assets/themes/vscode/vscode.json`.
- `script/import-codicons` copies Codicons over Zed's interface icons.
- `script/import-seti-icons` builds the Seti icon themes in `assets/icons/color/seti`.

Licenses are in `assets/themes/LICENSES` (MIT) and `assets/icons/LICENSES` (Codicons CC BY 4.0, Seti MIT).
