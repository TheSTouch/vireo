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
- `@` opens a file and folder list in the message box: `ac2410e811`.
- The extension uninstall test, and the race it revealed in the extension store: `ff07243366`.
