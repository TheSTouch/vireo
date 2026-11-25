# Vireo backlog

Work we have agreed to do later, not yet given to a session. When a batch takes an item, it keeps
the item's id in its commit subjects; when the item lands, move it to **Done** with the commit hash.
Each item says what happens today, what should happen, where the code is, and the decisions taken
so far ("Open" lists what the owner may still change before a session starts it).

## Claude panel

### V-7. The panel's design, continued

- **Done in batch Panel (A-V7, `9736f9e066`):** the permission card (numbered options, up/down,
  Enter and number keys) and a sparser empty state (small logo, one line, the message box below).
- **Left** (also in `docs/src/vs-core/claude-parity.md`, "Design"):
  - before and after screenshots of the permission card and the empty state: screen capture is not
    allowed on the batch's machine, and the offscreen visual test runner needs a full debug build of
    the app (not enough disk space then);
  - the effort level in the model pill ("Opus 5.5 High"), and a microphone button;
  - a bookmarks button in the header;
  - syntax-highlighted diff lines;
  - look at the "@" list in the app (only tested so far).
- **Open:** the owner to say which differences matter most.

### V-13. Compare every small feature of the panel with VS Code's Claude Code extension

- **Today:** parity is tracked in `docs/src/vs-core/claude-parity.md`, built from the extension's
  documentation and from looking at it, item by item. The owner keeps finding small features that
  Vireo lacks or does differently.
- **Expected:** a full, feature-by-feature comparison, to use as the base for recreating the missing
  ones. Use the extension installed in VS Code (`/Applications/Visual Studio Code.app`). Go through
  every view, card, menu, button, keybinding and setting. For each one, write down what the extension
  does, what Vireo does, and the gap. Take screenshots of both where screen capture is allowed. The
  result updates `claude-parity.md`, and each gap becomes its own backlog item, grouped into batches.
- **Sources:** the running extension; its manifest (`package.json`: commands, settings, keybindings,
  views); its documentation at code.claude.com; its stylesheet, for sizes and colours, as V-3 did;
  and its bundled code, read in depth to understand what it does and how (the messages it sends and
  receives, its states, its edge cases).
- **Decided (owner, 2026-11-25):** "we check, we find, we understand, we implement. we dont steal,
  we don't copy." Study the extension as deeply as needed, including its code. Then write down what
  was understood, in our own words, and implement it ourselves in Rust from that description. No code,
  icons or other assets are copied or translated line by line. Vireo's panel stays our own
  implementation (README: "No code or assets come from the official extension").
- **Done on 2026-11-25 (manifest pass):** every command, setting, keybinding and menu entry of the
  extension's `package.json` (2.1.331) checked against `claude-parity.md`. All were already there
  except what 2.1.331 added: four settings and three commands, now rows of `claude-parity.md` and
  items V-16 to V-21.
- **Left:** the views, cards, menus and buttons of the running extension and its webview code, each
  compared with Vireo's; screenshots of both (screen capture is not allowed on this machine).

### V-16. Setting: Escape stops Claude (`escapeStopsClaude`)

- **Extension (2.1.331):** `claudeCode.escapeStopsClaude`, true by default. Escape in the Claude
  panel stops Claude while it works; when off, only the Stop button does. Escape still closes menus
  and dialogs and declines permission requests and questions.
- **Vireo today:** Escape always runs `claude_code::Interrupt` (keymaps, contexts `ClaudeCode` and
  `ClaudeCodeInput > Editor`).
- **Expected:** `claude_code.escape_stops_claude`, true by default; when false, Escape does not
  interrupt, and keeps its other uses.

### V-17. Setting: pin your message while reading the reply (`pinPrompt`)

- **Extension (2.1.331):** `claudeCode.pinPrompt`, true by default. Your message stays pinned at the
  top of the conversation while you scroll through Claude's reply to it; when off, it scrolls away.
- **Vireo today:** the message scrolls away with the conversation (`view.rs`, the message list).
- **Expected:** `claude_code.pin_prompt`, true by default: while a reply is on screen, the user
  message it answers sticks to the top of the list.

### V-18. The context usage indicator below the message box (`contextUsageThreshold`)

- **Extension (2.1.331):** `claudeCode.contextUsageThreshold`, a number from 0 to 100, 50 by
  default. The context usage indicator shows below the message box once more than this percentage
  of the context window is used; 0 shows it from the first reply.
- **Vireo today:** context tokens and cost appear only in a tooltip.
- **Expected:** an indicator below the message box (the share of the context window used), shown
  above the threshold, and the setting `claude_code.context_usage_threshold`. Look at the
  extension's indicator (what it shows, what clicking it does) before building it.

### V-19. "Fix with Claude Code" on failed tests (`fixTestFailure`, `showFixActions`)

- **Extension (2.1.331):** the command `claude-vscode.fixTestFailure` ("Fix with Claude Code") on a
  failed test's message, in the Testing view's context menu and inline on the message, shown while
  `claudeCode.showFixActions` (true by default) is on.
- **Vireo today:** nothing. Vireo shows test results through tasks, not a Testing view, so the place
  for the action has to be chosen.
- **Open:** where it goes (a failed task's output, diagnostics' code actions); the setting's text,
  "where the editor reports something to fix", may also cover diagnostics.

### V-20. Resolve merge conflicts with Claude (`resolveMergeConflicts`)

- **Extension (2.1.331):** `claude-vscode.resolveMergeConflicts` ("Claude Code: Resolve Merge
  Conflicts"), an editor title button on a file with conflicts, and in the menu of Git's "Merge
  Changes" group.
- **Vireo today:** nothing.
- **Expected:** the action in Vireo's conflict UI (the editor's conflict toolbar, the git panel's
  conflicted files), opening Claude with a prompt to resolve that file's conflicts. Look at what
  prompt the extension sends and where the conversation opens.

### V-21. Add a notebook cell to the chat (`addCellToChat`)

- **Extension (2.1.331):** `claude-vscode.addCellToChat` ("Claude Code: Add Cell to Chat") in the
  notebook cell title menu.
- **Vireo today:** nothing for cells; Add to Chat works on files and selections.
- **Expected:** the same entry on a cell in Vireo's notebook view, adding the cell to the message
  box.


### V-22. Pasting an image or a file in the message box inserts its name

- **Seen on 2026-11-25:** the owner: "I can't paste images or files in the Claude prompt input, it
  just puts the name and not the file."
- **Cause:** `ClaudeCodeView::paste` (`view.rs` ~1322) listens for `editor::Paste` on the view's
  root, so the message box's editor runs Paste first (`editor/src/clipboard.rs` ~252) and never
  passes it on: images and files copied in Finder never reach Vireo's handler. Also, macOS's
  clipboard reader (`gpui_macos/src/pasteboard.rs` ~59, `read`) returns the text alone when an image
  comes with a text flavour (a browser's "Copy Image"). Dropping files on the panel is not handled
  either, though `claude-parity.md` says "pasting or dropping an image" works.
- **Expected:** a pasted or dropped image becomes an image chip, a pasted or dropped file or folder
  an `@` chip; text pastes as before.

## Done

- V-1, Tab accepts the suggested prompt (the message box's key contexts were never matched, which
  also disabled the "/" list's keys and `use_ctrl_enter_to_send`): `5fa2a27fd9`.
- V-2, new and past conversations open in a new tab (settings `new_conversation_opens_in`,
  `past_conversation_opens_in`): `c264eb8aad`.
- V-3, icons as large as VS Code's: the Claude panel `3fa939cad5`; the status bar and the title bar
  `26bffef2e6`.
- V-4, a project opens with its Claude conversation ready (setting `open_on_project_open`):
  `a4a8f41e6c`.
- V-5, opening a conversation shows its latest messages: `7a7eebc39c`.
- V-6, a "↓" button to jump back to the latest messages: `5bed49a46c`.
- V-8, a restarted conversation picks up its interrupted step: `aeb6a407a4`. Left: Claude tabs are
  not restored after Vireo itself restarts, so a turn interrupted in a tab by a Vireo restart is not
  continued.
- V-9, the panel's log also goes to `claude-code.log` in the logs folder: `6270ea1f80`.
- V-10, your messages and tool blocks can be selected and copied: `2339863f61`. Left: the diff lines
  of Edit and Write calls cannot be selected.
- V-11, AskUserQuestion shows its questions and options: `1d6b54173e`.
- V-12, the AskUserQuestion picker checked against a live request from `claude` 2.1.329: the CLI
  accepts V-11's answers, and the live request is a test fixture: `3ee28b3458`. Left: seeing the
  picker drawn in the app.
- V-14, a project opens with a normal Claude tab, not the side panel (the side bar left open by V-4
  is closed once per folder): `e4875fee3d`.
- V-15, Tab on the suggestion: not a bug (the owner found it works). A test now runs it the way the
  app does, and with suggestions off Tab no longer pastes the hidden one: `b8e7c6377b`.
- `@` opens a file and folder list in the message box: `ac2410e811`.
- The extension uninstall test, and the race it revealed in the extension store: `ff07243366`.
