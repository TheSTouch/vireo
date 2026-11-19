# VS Core

VS Core is a fork of [Zed](https://github.com/zed-industries/zed). The goal is a full replacement for Visual Studio Code: the features and workflow people rely on in VS Code, built on Zed's native Rust core. No Electron, no Chromium, and a fraction of the memory.

> **Status: early.** For now the code is Zed plus the groundwork described below. Expect breaking changes.

VS Core is an independent project. It is not affiliated with or endorsed by Zed Industries, Microsoft or Anthropic.

## Why

VS Code runs every window as a Chromium renderer plus a Node extension host, plus its own copy of every language server. With a few windows open, that easily reaches several gigabytes. Zed already proves that a native, GPU-rendered editor can be fast and light. What it lacks, for many VS Code users, is the breadth: panels, explorers, tooling and the extension ecosystem. VS Core aims to close that gap.

## What we decided

- **Built on Zed, kept close to upstream.** VS Core regularly merges upstream Zed. Our work lives in new crates wherever possible, so merges stay cheap.
- **Our own addon system. VS Code extensions will not run.** We are not building a compatibility layer for the VS Code extension API. Instead, VS Core extends Zed's WebAssembly extension system with a **UI API**, so addons can add:
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
- **A first-class Claude Code integration.** VS Core includes a Claude Code panel that matches the feature set and layout of the official Claude Code extension for VS Code. It covers:
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
4. VS Core branding, builds and releases.

## Building

VS Core builds exactly like Zed:

- [Building for macOS](./docs/src/development/macos.md)
- [Building for Linux](./docs/src/development/linux.md)
- [Building for Windows](./docs/src/development/windows.md)

## Staying in sync with Zed

```sh
git remote add upstream https://github.com/zed-industries/zed.git
git fetch upstream
git merge upstream/main
```

## Licensing

VS Core inherits Zed's licensing: GPL-3.0-or-later, with Apache-2.0 components where marked (including the GPUI framework). See the `LICENSE-*` files. Zed is developed by Zed Industries, Inc.

License information for third party dependencies is checked with [`cargo-about`](https://github.com/EmbarkStudios/cargo-about); see `script/licenses/zed-licenses.toml`.
