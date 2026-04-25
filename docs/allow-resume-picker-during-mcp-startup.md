# Allow Resume Picker During MCP Startup

## Prompt request

theres a really annoying thing when launching codex where it loads the mcp's and if you do /resume to open the menu to find your session it errors with ■ '/resume' is disabled while a task is in progress. - can we either solve this by using existing message queue mechanisum or by just allowingt the resume menu to open during the mcp load - the thing is once you resume it loads again anyway.. explore the codebase and suggest the options and cleanest path for improvement.

## Summary

`/resume` is currently blocked by a generic "task running" guard, and MCP startup is treated as part of that same running state. On current upstream that guard now exists in both the composer and the chat widget, so the problem is still command gating, not missing resume plumbing.

The cleanest fix is to make slash-command availability context-aware so `/resume` can be allowed during MCP startup while remaining blocked during a real in-flight agent turn.

## Relevant code paths

- `codex-rs/tui/src/slash_command.rs`
  - `SlashCommand::available_during_task()` still marks `/resume` as unavailable during any running task.
- `codex-rs/tui/src/bottom_pane/chat_composer.rs`
  - `reject_slash_command_if_unavailable()` now blocks unavailable commands before `ChatWidget::dispatch_command()` runs.
  - This means bare `/resume` is rejected in the composer as soon as `is_task_running` is true.
- `codex-rs/tui/src/chatwidget.rs`
  - `update_task_running_state()` collapses `agent_turn_running || mcp_startup_status.is_some()` into one bottom-pane busy flag.
  - `dispatch_command()` and inline-arg dispatch still block commands solely from `bottom_pane.is_task_running()`, which is why MCP startup disables `/resume`.
  - `update_mcp_startup_status()` sets `mcp_startup_status` and turns the task-running state on.
  - `finish_mcp_startup()` clears startup state and re-enables normal command handling.
- `codex-rs/tui/src/app.rs`
  - `AppEvent::OpenResumePicker` already starts the picker app-server, opens the picker, rebuilds config, resumes the selected thread, shuts down the current thread, and swaps in a resumed `ChatWidget`.
  - This means the app-side resume flow is already present and does not need a new queueing system just to open the picker.
- `codex-rs/tui/src/chatwidget/interrupts.rs`
  - Existing queueing here is only for inbound interrupt-like render events such as exec/MCP/approval updates during streaming.
- `codex-rs/tui/src/chatwidget.rs`
  - `maybe_send_next_queued_input()` is only for queued user messages and explicitly waits for `is_task_running()` to become false.

## Why this happens

Today the TUI does not distinguish between:

- "the agent is actively in the middle of a turn"
- "startup/background MCP initialization is still happening"

Both states drive the same busy bit, and slash command availability only looks at that one bit. That is why `/resume` gets rejected even though opening the picker during startup is conceptually much safer than opening it during an active coding turn.

## Options

### 1. Add a targeted `/resume` special-case in both gates

Teach both:

- `ChatComposer::reject_slash_command_if_unavailable()`
- `ChatWidget::dispatch_command()`

to allow `/resume` when MCP startup is running but no agent turn is running.

Pros:

- Smallest code change.
- Directly solves the reported startup UX problem.
- Preserves the safety guard against abandoning a real in-progress turn.
- Reuses the existing `AppEvent::OpenResumePicker` and app-side resume flow.

Cons:

- Duplicates policy across two layers.
- Still leaves slash-command availability tied to an overly coarse bool.

### 2. Make slash-command availability context-aware

Replace the current `available_during_task()` bool with an availability check that understands at least:

- no busy state
- MCP startup busy
- agent-turn busy
- both

Then have both composer-side and chat-widget-side gating consult the same shared policy.

Pros:

- Architecturally cleaner.
- Fixes the current `/resume` bug without sprinkling one-off exceptions.
- Scales if other commands later need "allowed during MCP startup, blocked during active turn" behavior.

Cons:

- Slightly larger than a one-command special-case.
- Requires threading richer busy context into the composer, not just the chat widget.

### 3. Queue `/resume` until MCP startup completes

Accept `/resume` while startup is active, but defer opening the picker until MCP startup finishes.

Pros:

- Avoids acting while startup is still active.

Cons:

- It does not actually solve the interaction the user wants.
- Existing queueing mechanisms are not a good fit:
  - interrupt queueing is for inbound render events
  - queued user messages are for turn submissions, not app commands
- Adds complexity for weaker UX than option 1.

## Recommended path

Implement option 2 if we want the cleanest upstreamable shape. If we want the absolute smallest patch, option 1 is acceptable.

Reasoning:

- The current bug is specifically that MCP startup is overloading the same busy state as a real agent turn.
- On current upstream the policy is enforced in both the composer and the chat widget, which makes a single local exception less appealing.
- The app already has a complete in-process resume-picker flow.
- Allowing `/resume` during MCP startup is materially lower risk than allowing it during an active turn, because a successful resume will intentionally replace the current session anyway.
- A slash-command-specific availability context is enough for this issue without forcing a bigger UI-wide busy-state refactor.

## Proposed implementation shape

Keep the current bottom-pane `is_task_running` behavior for status, footer, and queued-input autosend.

Separately, introduce a slash-command availability check that can distinguish:

- active agent turn
- MCP startup only

One practical shape:

- add a small context enum or struct near `slash_command.rs`
- move slash-command gating policy there
- have both composer-side rejection and chat-widget dispatch use that shared helper
- mark `/resume` as allowed during MCP startup-only, but not during an active turn

This keeps the existing status/UI behavior intact while fixing the command policy at the right abstraction level.

## Test plan

Add or update TUI tests in both layers:

- keep the existing `/resume` picker-opening test
- add a composer-level test that bare `/resume` is not rejected during MCP startup-only
- add a chat-widget test that dispatching `/resume` during MCP startup emits `AppEvent::OpenResumePicker`
- add a chat-widget test that dispatching `/resume` during `TurnStarted` still emits the existing "disabled while a task is in progress" error

If the visible copy stays unchanged, snapshot churn should be minimal.

## Follow-up refactor worth considering later

If this pattern shows up for more commands outside slash handling, add a first-class UI busy-reason model and stop using one derived bool for both MCP startup and turn execution. That is broader than needed for this issue, but it would be the natural next step if more command or footer behavior needs the same distinction.
