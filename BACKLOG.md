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

### V-12. Check the AskUserQuestion picker against a real question

- **Seen on 2026-11-25:** when Claude asked a multiple-choice question, the card showed the tool's
  JSON payload with the answers inside it, and only "Yes" or "Tell Claude what to do": no way to pick
  an answer. That was the app installed from `a084f43`, built before V-11 (`1d6b54173e`), which
  replaces that card with a picker.
- **Expected:** in a build that includes V-11, a real AskUserQuestion from the `claude` CLI shows
  the picker: each question, its options with descriptions, single or multiple choice, "Other",
  keyboard selection, Submit. Claude receives the answers and carries on with them.
- **To do:** reproduce it with a live `claude` (ask it to "ask me a multiple-choice question with
  AskUserQuestion"). V-11 was built from the Agent SDK docs and never compared with a live
  `can_use_tool` request (its commit says so). Check the request and the answer in `claude-code.log`
  (V-9). Fix any mismatch and add the live request as a test fixture.
- **Code:** `crates/claude_code_ui/src/view.rs` (the picker in `render_permission`),
  `session.rs` `respond_permission`, `protocol.rs` `PermissionDecision`.

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

### V-14. A project opens with a normal Claude tab, not the side panel

- **Seen on 2026-11-25:** V-4 (`a4a8f41e6c`) shows the Claude panel in the right side bar every
  time a project opens, even if it was closed last time. The owner: "that is not what I asked, I want
  a normal Claude Code tab pre-opened, that is all. Now I have a part of the screen locked on a
  Claude session I cannot close."
- **Expected:** opening a project folder opens one ordinary Claude tab in the editor area, like any
  other tab: it can be closed, moved and split. It resumes the project's newest conversation, or
  starts a new one, with the message box focused. The side bar is left alone.
- **Code:** `claude_code_ui.rs` `open_on_project_open_from`, called from `zed.rs`
  `initialize_panels`; `panel.rs` `open` with `Location::PrimaryEditor`.
- **Decided:** keep the setting `claude_code.open_on_project_open` (true by default). No tab is
  added when the window already has a Claude tab. The windows whose saved layout still has the
  Claude panel open in the right side bar (from V-4) get it closed once, the first time the new
  behaviour runs, so nobody stays stuck with it; after that, the side bar is never touched.

### V-15. Tab still does not accept the suggested prompt

- **Seen on 2026-11-25:** in the app built from `a9b2ba6c1d`, which includes V-1 (`5fa2a27fd9`),
  Tab on Claude's grey suggested prompt still does not fill the message box. V-1's test passes, so
  the test does not reproduce what happens in the app.
- **Expected:** Tab fills the message box with the suggestion, as V-1 describes.
- **To do:** find what differs between the test and the app (the side panel instead of a tab, the
  VS Code base keymap, the real `prompt_suggestion` message, where the focus is) and test that.

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
