# Vireo

**A fast, native VS Code replacement, built on Zed, with a built-in Claude Code panel.**

Vireo is a fork of [Zed](https://github.com/zed-industries/zed). The goal is a full replacement for Visual Studio Code: the features and workflow people rely on in VS Code, built on Zed's native Rust core. No Electron, no Chromium, and a fraction of the memory.

> **Status: early.** For now the code is Zed plus the groundwork described below. Expect breaking changes.

Vireo is an independent project. It is not affiliated with or endorsed by Zed Industries, Microsoft or Anthropic.

## Why

VS Code runs every window as a Chromium renderer plus a Node extension host, plus its own copy of every language server. With a few windows open, that easily reaches several gigabytes. Zed already proves that a native, GPU-rendered editor can be fast and light. What it lacks, for many VS Code users, is the breadth: panels, explorers, tooling and the extension ecosystem. Vireo aims to close that gap.

## What we decided

- **A hard fork of Zed.** Vireo started from Zed and now goes its own way. It does not merge upstream Zed regularly. We look at what Zed does and borrow ideas or individual changes when they fit.
- **VS Code's layout and look.** Vireo arranges its window the way VS Code does:
  - an activity bar on the left, the sidebar next to it, and a secondary sidebar on the right
  - editor tabs with breadcrumbs, and a tabbed bottom panel
  - a title bar with a central search box, and VS Code's status bar layout
  - VS Code's default themes (Dark Modern, Light Modern), Codicons and VS Code's default keybindings

  The trademarked VS Code name and logo are not used.
- **Our own addon system. VS Code extensions will not run.** We are not building a compatibility layer for the VS Code extension API. Instead, Vireo extends Zed's WebAssembly extension system with a **UI API**, so addons can add:
  - commands
  - panels and tree views
  - status bar items
  - editor decorations
  - their own views

  Today Zed extensions can only provide language servers, debug adapters, slash commands and MCP servers.
- **VS Code features come back as addons.** Each missing feature becomes its own addon on top of the UI API. Planned addons include:
  - a test explorer
  - Jupyter notebooks
  - a .NET solution explorer
  - Docker and container panels
  - file and folder compare
  - i18n tooling
- **A first-class Claude Code integration.** Vireo includes a Claude Code panel that matches the feature set and layout of the official Claude Code extension for VS Code. It covers:
  - side panel and editor tabs
  - sessions
  - @-mentions
  - inline diff review with accept and reject per change
  - permission modes
  - the IDE tools Claude uses to read your selection, diagnostics and open files

  It is our own implementation. No code or assets come from the official extension.

## Roadmap

1. UI API for addons: the foundation everything else builds on.
2. Claude Code panel, at feature parity with the VS Code extension.
3. Feature addons, one VS Code capability at a time.
4. Vireo branding, builds and releases.

## Building

Vireo builds exactly like Zed:

- [Building for macOS](./docs/src/development/macos.md)
- [Building for Linux](./docs/src/development/linux.md)
- [Building for Windows](./docs/src/development/windows.md)

## Borrowing from Zed

Vireo does not track Zed. To look at a change made upstream and pick it up when it fits:

```sh
git remote add upstream https://github.com/zed-industries/zed.git
git fetch upstream
git cherry-pick <commit>
```

## Licensing

Vireo inherits Zed's licensing: GPL-3.0-or-later, with Apache-2.0 components where marked (including the GPUI framework). See the `LICENSE-*` files. Zed is developed by Zed Industries, Inc.

License information for third party dependencies is checked with [`cargo-about`](https://github.com/EmbarkStudios/cargo-about); see `script/licenses/zed-licenses.toml`.
