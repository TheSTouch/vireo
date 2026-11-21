# Develop Vireo

Vireo can change itself. The **Develop Vireo** page, opened with the tools icon at the bottom of
the activity bar (above Accounts) or with **Develop Vireo: Open** in the command palette, gets
Vireo's sources onto your machine and starts Claude Code sessions that work on them.

## Sources

The page downloads the sources from GitHub (`https://github.com/TheSTouch/vireo`) into `~/Vireo`.
**Use Another Folder…** points it at a checkout you already have; Vireo remembers that choice.
The page shows the branch, the commit and how many files you have changed. **Open Sources** opens
them in a window of their own.

The download is a partial clone (`git clone --filter=blob:none`): file contents are fetched when
needed, which keeps the first download small.

## Build tools

The page checks for git, Rust (`cargo`) and, on macOS, Xcode's command line tools, and links to
[Building for macOS](../development/macos.md) for anything missing.

`script/install-vireo-mac` builds Vireo from the sources in release mode and installs it into
`/Applications`. The first build takes 30 to 60 minutes. Run nothing else with cargo meanwhile:
Vireo's builds use a lot of memory. The app keeps the dev release channel, so it never offers
updates from Zed's servers, and it is signed locally: the first time, open it with right-click >
Open if macOS refuses.

## Prepared sessions

Each prepared session opens the sources and a new Claude Code conversation with a prompt in the
message box. Nothing is sent until you press Enter, so you can read and edit the prompt first.

| Session | What it does |
| --- | --- |
| Change something | You describe a change; Claude makes it, tests it and commits it. |
| Fix a bug | You describe the bug; Claude finds the cause, adds a failing test and fixes it. |
| Build and install Vireo | Builds the sources with `script/install-vireo-mac` and installs the app. |
| Update to the latest Vireo | Pulls the latest Vireo, keeps your commits on top, rebuilds and installs. |

Sessions never push unless you ask.

### Adding a session

Prepared sessions are Markdown files in `assets/vireo/sessions/`, listed in file name order. The
page reads them from your checkout, so a new or edited file shows up without rebuilding Vireo
(it falls back to the copies built into Vireo when the checkout has none):

```markdown
---
title: Write an addon
description: Describe an addon; Claude creates it with Vireo's addon UI API.
input: What should the addon do?
---
You are working in Vireo's own sources. Create an addon that does this:

{{input}}

Read docs/src/vs-core/addon-ui-api.md first.
```

- `title` is required; `description` shows under it.
- `input` adds a text box, with its value as the placeholder; `{{input}}` in the prompt is
  replaced by what you type. Leave it out for a session that needs no input.
