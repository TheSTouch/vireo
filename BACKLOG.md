# Vireo backlog

Work we have agreed to do later, not yet given to a session. When a batch takes an item, it keeps
the item's id in its commit subjects; when the item lands, move it to **Done** with the commit hash.
Each item says what happens today, what should happen, where the code is, and the decisions taken
so far ("Open" lists what the owner may still change before a session starts it).

## Claude panel

### V-1. Tab accepts the suggested prompt

- **Today:** when the message box is empty, it shows Claude's suggested next prompt in grey. Pressing
  Tab inserts a tab character and the suggestion disappears.
- **Expected:** Tab fills the message box with the suggestion, cursor at the end, as in VS Code's
  extension. Typing anything else first dismisses the suggestion as today.
- **Code:** `crates/claude_code_ui/src/view.rs`: the suggestion is shown by
  `refresh_placeholder_later` (~483) from `conversation.prompt_suggestion`; the key context adds
  `has_suggestion` only when the input is empty (~1615); the keymaps bind `tab` to
  `claude_code::AcceptSuggestion` under `ClaudeCodeInput > Editor && has_suggestion`. Find out why the
  binding loses to the editor's own Tab (precedence, or the context not being set when the grey text
  shows), and add a test that Tab accepts.

### V-2. New conversation and past conversations open in a new tab

- **Today:** the panel's **+** (new conversation) and picking a conversation from **past
  conversations** (the clock) both replace the conversation in the current view.
- **Expected:** both open a new Claude tab by default, so the current conversation stays open.
- **Settings:** `claude_code.new_conversation_opens_in` and `claude_code.past_conversation_opens_in`,
  each `"new_tab"` (default) or `"current"`, for people who prefer replacing.
- **Code:** `new_conversation_requested` (`view.rs` ~837), `show_sessions`
  (`claude_code_ui.rs` ~275), `panel::open` with `Location::NewTab`.
- **Open:** when the + is pressed in the side bar panel (not a tab), the default opens the new
  conversation in an editor tab and leaves the side bar as it is.

### V-3. Icons as large as VS Code's

- **Today:** the status bar icons and the Claude panel's icons look smaller than VS Code's.
- **Expected:** the same sizes as VS Code at the same zoom. Measure both apps from screenshots
  (VS Code: 16 px Codicons in a 22 px status bar; check the Claude extension's header and message box
  buttons) and match.
- **Scope decided:** the status bar; the Claude panel header (past conversations, new conversation,
  focus view); the Claude message box toolbar (+, /, model, mode, send).
- **Open:** the title bar layout toggles at the top right too, if they also look small.

### V-4. Every project opens with a Claude session ready

- **Today:** a new project window opens with no AI session.
- **Expected:** opening a project shows the Claude panel in the right side bar with that project's
  most recent conversation resumed, or a new conversation when the project has none, so you can
  start typing at once.
- **Setting:** `claude_code.open_on_project_open`, `true` by default.
- **Open:** "always a new conversation" instead of the last one; and later, the same for other AI
  agents (the owner said "in my case Claude"), probably as a choice of agent.

### V-5. Opening a conversation shows its latest messages

- **Today:** resuming a conversation (past conversations, reopening a tab, restoring after a
  restart) shows its first message, scrolled to the top.
- **Expected:** it opens scrolled to the bottom, on the latest messages, and keeps following new ones
  (`follow_tail`).
- **Code:** `view.rs`: `follow_tail` (~95), `scroll_handle.scroll_to_bottom()` (~432, ~541, ~605).
  The scroll probably runs before the restored transcript is laid out; scroll once it is.

### V-6. A button to jump back to the latest messages

- **Today:** after scrolling up in a long conversation, the only way back is to scroll down.
- **Expected:** while the conversation is not at the bottom, a round "↓" button floats above the
  message box (as in the extension); clicking it scrolls to the bottom and resumes following.
- **Code:** `view.rs` `is_scrolled_to_bottom`, `follow_tail`.

### V-7. The panel's design, continued

- **Today:** batch Polish (A-2a to A-2c) brought the panel closer to the extension; the owner finds it
  better but still not the same.
- **Left from A-2** (also in `docs/src/vs-core/claude-parity.md`, "Design"):
  - compare the permission card (numbered options, up/down selection);
  - a sparser empty state: small logo, one line, the message box at the bottom;
  - the effort level in the model pill ("Opus 5.5 High"), and a microphone button;
  - a bookmarks button in the header;
  - syntax-highlighted diff lines;
  - look at the "@" list in the app (only tested so far).
- **Open:** the owner to say which differences matter most; until then, start with the permission
  card and the empty state.

### V-8. A restarted conversation picks up its interrupted step

- **Today:** seen on 2026-11-21. When Vireo restarted a conversation's `claude` process (`--resume`)
  while a tool call was running, the call was lost and the conversation went idle with no message. The
  person had to type "continue". `continue_after_reload` is meant to cover this and did not.
- **Expected:** after any restart of the process, a turn that was interrupted continues on its own
  (or the panel says it was interrupted, with a Continue button).
- **Code:** `crates/claude_code_ui/src/session.rs` (start, `--resume`), the `continue_after_reload`
  handling. Reproduce by killing the `claude` process during a long Bash call.

### V-9. The panel's logs go to a file

- **Today:** "Show logs" shows the `claude` process output kept in memory; nothing is on disk, so a
  problem like V-8 cannot be looked at afterwards, or from another session.
- **Expected:** also write them, rotated, to `~/Library/Logs/Zed/claude-code.log` (the logs folder
  of the platform), with the session id on each line.
- **Code:** `crates/claude_code_ui/src/extras.rs` (`ClaudeLogs`, `show_logs`).

### V-10. Text in your own messages can be selected and copied

- **Today:** the messages you send are drawn as a plain label (`render_user` in
  `crates/claude_code_ui/src/view.rs`, `Label::new(text)`), so you cannot select part of them to copy.
  Claude's replies use the Markdown element and can be selected.
- **Expected:** select any part of your messages with the mouse and copy it with Cmd+C, like Claude's
  replies; and a copy button on hover that copies the whole message, as in the extension. Check that
  tool call input and output blocks can be selected too.
- **Code:** render the user text with the same selectable element as the replies (plain text, not
  Markdown, so what you typed is shown as typed).

### V-11. AskUserQuestion shows its options

- **Today:** when Claude asks a multiple-choice question (the AskUserQuestion tool), the panel shows a
  generic permission card instead of the options, so the question cannot really be answered (seen on
  2026-11-21: the owner never saw the choices). `docs/src/vs-core/claude-parity.md` lists it as Todo.
- **Expected:** a picker as in the extension: the question, its options with their descriptions
  (single or multiple choice), an "Other" free text field, keyboard selection, and Submit, which
  sends the answers back as the tool's result.
- **Code:** `crates/claude_code_ui/src/view.rs` `render_permission`, the `can_use_tool` request
  for `AskUserQuestion` in `session.rs` and `protocol.rs`.

## Done

- `@` opens a file and folder list in the message box: `ac2410e811`.
- The extension uninstall test, and the race it revealed in the extension store: `ff07243366`.
