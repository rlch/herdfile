## Context

herdr (0.9.x) is a terminal workspace manager for AI coding agents: workspace › tab › pane, with
agent status (`idle|working|blocked|done|unknown`) detected per pane. Agents create tabs and panes
through its CLI, but nothing tells them which tab, what to call it, whether to reuse one, or when to
close it. Measured on one machine (2026-10-07): 23 workspaces, 35 panes, 12 of 14 non-agent panes a
bare shell, 64 of 77 worktree directories with no pane, no pane labels in use.

Prior art surveyed: zellij layouts and tmuxinator/smug/tmuxp are start-once and never re-apply a
changed file. zellij `dump-layout` and tmuxp `freeze` snapshot live state back into a file. VS Code
tasks and process-compose name a command and its readiness, with placement as a separate setting.
herdr-workflows runs linear agent pipelines, with no reuse or cleanup.

herdr facts this design relies on (checked against 0.8.0 source and the 0.9.3 binary schema):

- `herdr api snapshot` returns the whole tree in one call, including labels, cwd, and agent status.
- Pane labels, tab and workspace names, and public ids (`wN:pK`) persist across a server restart.
  Tokens do not. `terminal_id` does not.
- `pane move` gives the moved pane a new id.
- The socket handles one request at a time, but nothing dedupes creates. Two clients doing
  list-then-create will both create.
- Closing a pane sends its processes SIGHUP, then SIGTERM, then SIGKILL, 250 ms apart. Closing a
  tab or workspace does the same for every pane in it.
- Events (`events.subscribe`) carry resource fields only. Nothing says which client caused them.
- Plugins have `[[startup]]` (one-shot, re-run after live handoff), `[[events]]`, and `[[actions]]`.
  They have no supervised long-lived process. agent-tags works around this by detaching a process
  behind a lock.
- herdr has no floating panes, only singleton popups outside the layout.

## Goals / Non-Goals

**Goals:**

- One file per workspace that says what is on screen, readable and editable by agents.
- herdr converges to that file. Leaving something out of the file closes it.
- The operator's hand changes are kept, not fought.
- Never take focus. Never close a working or blocked agent. Never create anything herdr does not
  know about.

**Non-Goals (this change):**

- Writing back hand moves and reorders. Closes and hand-opened shells only.
- Running services outside herdr (process-compose or similar).
- Port allocation between workspaces.

## Decisions

### The file says what should exist; herdr says what does

The tmux-family tools treat the multiplexer as the only truth and never diff a file against it. That
works for them because their files are start-once templates that nobody edits afterwards. Here the
file is edited all day by agents, so it has to be the target. Two rules stop the file and herdr
from fighting:

1. Anything only herdr can know is never written to the file: running or exited, agent status, pane
   ids, focus. Those are read live.
2. Hand changes are folded into the file (write-back) before the next apply. The file therefore
   never asks for something the operator just undid.

Alternative: commands only (`place dev`, `close dev`) with no file. Rejected. Agents get no single
place to read the layout, and closing becomes per-call bookkeeping again.

### Repo file and workspace file are separate

`.herdr/services.toml` is committed and rarely changes. It names commands. The workspace file
changes constantly and says only placement, by name. A service is defined once and placed in any
workspace.

```toml
# repo/.herdr/services.toml
[dev]
cmd = "pnpm dev"
ready = "ready in"

[test]
cmd = "pnpm test --watch"
```

```toml
# workspace file
[tab.main]
row = ["agent", "test"]   # left to right

[tab.services]
row = ["dev"]
```

Rejected: an indirection layer in the workspace file (`show = "background" | "beside"`). The file
must read as the screen.

### The watcher is a hidden background process started by a herdr plugin

herdfile ships as a herdr plugin (`herdr plugin install rlch/herdfile`). Its `[[startup]]` hook runs
`herdfile watch --detach`, which forks a background process holding `events.subscribe` and exits.
A lock file ensures one watcher per herdr server, because herdr re-runs startup hooks after a live
handoff. The watcher has no pane. It logs to `$XDG_STATE_HOME/herdfile/watch.log`.
`herdfile watch` without `--detach` runs in the foreground for debugging.

```toml
# herdr-plugin.toml
[[startup]]
command = ["herdfile", "watch", "--detach"]
```

Rejected: running the watcher in a herdr pane. It is one more thing on screen, and nothing restarts
it after a herdr restart.

### Rust, with comment-preserving TOML edits

One Rust binary, `herdfile`. Commands and write-back use `toml_edit` to change only the affected
entries, so the file stays stable and diffs stay small.
Releases ship prebuilt binaries (macOS arm64/x86_64, Linux x86_64/arm64). The plugin's `[[build]]`
downloads the matching release binary, falling back to `cargo install` from source.

Rejected: Python. Its built-in `tomllib` only reads. Writing back without losing comments needs
`tomlkit`, which is one more thing to install.

### Only herdfile writes the workspace file

Agents change the file through commands. Each takes the workspace lock, so it always edits the
current file and never overwrites a write-back:

```
herdfile place dev --tab services     # add, or move if already placed
herdfile place logs --after agent
herdfile remove test
herdfile show                         # print the file
```

Agents read the file freely. Nobody edits it by hand, the operator included. The watcher still
applies a direct edit if one happens, but direct edits get no protection against lost updates.

Rejected: agents editing the text. An agent that reads the file, then saves after a write-back,
silently reopens what the operator just closed.

### All herdr calls go through one backend module

herdfile talks to the multiplexer only through a small backend interface: read the tree, subscribe
to events, create a tab, split with a size, move, rename, set a ratio, close. herdr's socket API is
the only implementation in this change.

tuios (v0.8.5, checked 2026-10-07) was considered as a replacement. It answers 77 of herdr's 102
socket methods, but rejects `layout.*` and split ratios, focuses the target pane on `pane split`
while a client is attached (ignoring `--no-focus`), and does not keep processes across a daemon
restart. A hidden watcher on it would move the operator's focus. It is re-checked on request. Switch
criteria: a split that does not take focus, settable split ratios, and processes that survive a
daemon restart. Meeting them means writing a second backend, not changing the file format.

### Workspace files live in a state folder

One file per workspace at `$XDG_STATE_HOME/herdfile/<workspace-id>.toml` (default
`~/.local/state/herdfile/`). herdr's workspace id survives a server restart. An agent finds its own
file with `herdfile path`, which reads `$HERDR_WORKSPACE_ID`.

Rejected: a git-ignored file in the workspace's folder. Two workspaces can share a folder (both on
`~/dev`), and they would fight over one file.

### A tab is a tree of rows and columns, with optional sizes

Any layout herdr can show can be written. A tab has one top-level `row` (left to right) or
`column` (top to bottom). Each entry is a pane name, or an inline table that is a pane with a size
or a nested row or column:

```toml
[tab.main]
row = ["agent", { column = ["test", { pane = "dev", size = 30 }], size = 40 }]
```
```
┌──────────────┬──────────┐
│              │ test     │
│ agent (60%)  │          │
│              ├──────────┤
│              │ dev 30%  │
└──────────────┴──────────┘
```

`size` is a percentage of the parent. Entries without one share what is left equally. herdr splits
are binary, so a row of n entries becomes a chain of n-1 splits with ratios that produce the
requested sizes. Apply uses `pane split`, `pane move`, and `layout.set_split_ratio`, never
`layout.apply`, which would kill the tab's processes.

When the operator drags a divider, the new size is written back, rounded to 5% so small drags do
not rewrite the file. A drag that rounds to the current value changes nothing. herdr reports
resizes as `layout_updated`; write-back reads the new ratios from the snapshot.

Rejected: a flat left-to-right list. It cannot express the common agent-left, two-stacked-right
layout, and the operator wants full control over shape and size.

### Commands return when herdr matches

Agents care about correctness and latency. A command (`place`, `remove`, `set`) takes the lock,
edits the file, and asks the watcher over a local socket to apply now. It returns when the snapshot
matches, printing what changed, or fails with the reason. There is no file-watch delay, and the
caller never acts on a layout that has not happened yet. `--no-wait` returns after the edit.

For a whole tab at once, `herdfile set <tab> '<row or column>'` replaces one tab's tree in one call,
so a re-layout is one round trip, not one command per pane.

### Identity is the pane label

Every pane herdfile manages is labelled with its service name (`dev`, `test`) or `agent`. Labels
survive restarts. Ids do not survive `pane move`. A pane's label is its identity in the file. Two
panes with the same label in one workspace is an error, reported rather than guessed at.

### Ownership is a mark in the file

Each pane entry is one of three kinds:

- managed (the default): opened and closed by the watcher.
- `mine`: the operator opened it by hand. Never closed by the watcher. Removed from the file when
  the operator closes it.
- `unmanaged`: something opened it without the file (an agent calling herdr directly). Recorded so
  agents can see it, never closed automatically.

### Agents are only closed when idle

Removing `agent` (or any pane whose agent status is `working` or `blocked`) from the file does not
close it. The pane is marked for removal and closed on its next transition to `idle` or `done`.

### The watcher ignores its own changes

Events carry no originator. The watcher keeps a short list of the operations it just issued
(create, close, move, rename, keyed by label and tab). Any event that matches one is its own. Any
event that does not is a hand change (or an outside agent) and goes to write-back. If a hand change
and a command hit the same pane before apply runs, the hand change wins and the command reports
that its change was dropped.

### First sight is write-back; no adopt command

An earlier draft had `herdfile adopt` to write a file for an open workspace, and `adopt --all` for
the file of workspaces. Both are write-back with an empty file, so the watcher does it instead: any
pane or workspace the file does not know is recorded, and on first sight that is everything. When a
pane or tab is recorded, its sizes come from the screen (whole percents), so recording never moves a
divider. Which workspaces the watcher manages is a setting, not a command:

```toml
# ~/.config/herdfile/config.toml
[watch]
workspaces = ["*"]          # default; or labels, with a trailing * for a prefix
```

Rejected: keeping `adopt` as an opt-in step. It is a one-off command doing what write-back already
does, and every workspace opened later would need it again.

### One writer at a time

A file lock per workspace serialises apply and write-back. This also closes the
find-or-create race that herdr itself does not guard against.

### No focus, no rearranging the operator's view

Every create and split passes `--no-focus`. The watcher defers moves and reorders in the tab the
operator is looking at until they leave it. Opens and closes there still happen.

### The file of workspaces

`dir` is a path (`~` allowed), as herdr's own `--cwd` takes; no repo-name lookup table.

One file per herdr server, `$XDG_STATE_HOME/herdfile/workspaces.toml`, written only by herdfile
commands, like the workspace files. Each key is a workspace name, which is also its herdr workspace
label.

```toml
[land-prs]
dir = "~/dev/schools-ts"
purpose = "land the engineers' open PRs"
parent = "orchestrator"

[review-pr-312]
dir = "~/dev/schools-ts"
branch = "review-pr-312"          # becomes a herdr worktree
purpose = "review PR 312"
parent = "land-prs"
agent = { brief = "briefs/review-312.md", model = "opus" }
```

```
herdfile ws add review-pr-312 --dir ~/dev/schools-ts --branch review-pr-312 \
  --parent land-prs --purpose "review PR 312" --brief briefs/review-312.md --model opus
herdfile ws remove review-pr-312
herdfile tree                       # names, purposes, parents, live agent status
```

Adding an entry with `branch` runs `herdr worktree create --no-focus` (never `git worktree add`),
writes the new workspace's own workspace file with `[tab.main] row = ["agent"]`, and starts the
agent with its brief. Without `branch`, the workspace opens on `dir`. The agent is started with a
configurable launch command, so the tool works with any agent CLI.

Removing an entry:

- agent working or blocked: nothing happens yet; removal waits until it is idle.
- agent idle and branch merged into its base: `herdr worktree remove`, workspace closed.
- agent idle and branch has unmerged commits: nothing removed; an item goes on the "needs you" list.
- no branch: the workspace is closed once idle. The folder is never deleted.

`parent` names another entry or `operator`. Removing a parent does not remove its children; they are
re-parented to the removed entry's parent. Status always comes live from herdr; the file stores no
status.

The same hand-change rule applies one level up. A workspace the operator closes by hand is removed
from the file. A workspace created outside herdfile is added as `unmanaged` and never removed
automatically. The first time the watcher sees an open workspace, it is added as `unmanaged`.

### Starting an agent: kind, args, then the brief as a prompt

This follows herdr and tuios. herdr's `agent start <name> --kind claude --pane <id> -- <args>` runs
a known agent CLI in an existing pane and returns when it is ready. tuios's `start-agent claude
--name x --prompt '...'` does the same and types the first prompt after. Neither puts the task on
the command line. So herdfile:

1. creates the pane (`--no-focus`),
2. starts the agent: by default `herdr agent start <workspace> --kind <kind> -- <args>`,
3. when it is ready, sends the brief with `herdr agent prompt <workspace> "Read <brief> and follow it."`.

```toml
# ~/.config/herdfile/config.toml
[agent]
kind = "claude"                 # herdr's --kind
args = ["--model", "{model}"]   # {model} from --model, per argument, no shell

# Optional: start through a wrapper instead. It is typed at the pane's shell
# prompt, so shell functions work; herdfile then waits for herdr to detect the
# agent and names it with `herdr agent rename`.
command = "cl --{model}"
```

The operator's dotfiles set `command = "cl --{model}"`, because `cl` picks the account and maps
`--opus` to the 1M model. `agent start --kind claude` would run plain `claude` and skip that.

Rejected: a full argv with the brief baked into the first argument. Neither herdr nor tuios starts
agents that way, and it skips the readiness wait.

### Messaging by name

herdr already has agent names: unique among live agents, `[a-z][a-z0-9_-]{0,31}`, cleared when the
agent exits. herdfile reuses them instead of adding its own. A workspace's name must match that
pattern, and its agent is given the same herdr agent name whenever herdfile starts it. So
`herdr agent prompt review-pr-312 "..."` works with no herdfile involved.

`herdfile tell` is a thin wrapper over `herdr agent prompt` that adds only what herdr lacks:

- `parent` resolves through the tree.
- The text is prefixed with the sender's name (`From review-pr-312: ...`).
- `--wait`, `--until` and `--timeout` pass straight through. With `--wait`, the reply is printed
  afterwards with `herdr agent read --source recent-unwrapped`, since herdr does not return it.
- herdr's errors pass through unchanged. A blocked target fails with `agent_blocked`, as herdr
  intends (a human answers approvals); herdfile does not queue around it. The blocked agent is
  already on the "needs you" list.

Rejected: queueing messages for blocked agents. It is a second delivery path that works around
herdr's deliberate refusal.

### The "needs you" list

One list of things waiting on the operator, stored as `$XDG_STATE_HOME/herdfile/needs.jsonl` and
shown with `herdfile needs`. Entries come from: an agent turning `blocked`, a removal held by
unmerged commits, and `herdfile ask "<question>"` from any agent. An entry is cleared when its cause
clears (the agent leaves `blocked`, the branch merges) or with `herdfile needs done <id>`. The file
format is documented so other tools (a PR landing tool) can add entries.

### Fit with herdr

Checked against herdr 0.9.3 (`herdr --skill`, CLI help) so herdfile adds to herdr and never works
around it:

- Names: workspace labels, pane labels, and herdr agent names are herdfile's identities. It mints no
  ids of its own.
- Commands: every change is a herdr CLI or socket call. `layout.apply` is never used on live tabs
  (it kills processes). `worktree create`, never `git worktree add`.
- Closing: herdr's guidance is "do not close what you did not create". herdfile closes only managed
  panes and workspaces, never `unmanaged` ones. It never passes `workspace close --group`; a primary
  workspace with linked worktrees stays open and goes on "needs you".
- Blocked agents: never typed into, matching herdr's `agent_blocked` refusal.
- Messaging: `tell` is `herdr agent prompt` plus a sender prefix and parent lookup.
- Placement guidance: herdr's bundled agent skill tells agents to open a sibling pane directly. With
  herdfile installed, agents should use `herdfile place` instead. Panes opened directly still work
  and are recorded as `unmanaged`, so nothing breaks if an agent follows herdr's skill.
- The watcher: herdr's `[[startup]]` hooks are one-shot, not supervised. Detaching a long-lived
  process from one is a workaround, the same one agent-tags uses. If herdr adds supervised plugin
  processes, herdfile moves to them.

## Risks / Trade-offs

- [Event-matching misreads a hand change as the watcher's own, or the reverse] → match on
  operation, label, and tab within a short window. Run a full snapshot diff after every apply as a
  backstop.
- [An agent changes the layout constantly and background tabs reshuffle] → accepted. The operator
  does not mind how often; there is no rate limit. Moves in the viewed tab still wait until the
  operator leaves it.
- [A closed service loses unsaved work] → services are commands, restartable by definition. Agents
  and `mine` panes are never force-closed.
- [Plugin startup is one-shot, re-run on handoff] → a detached process behind a lock, the same
  pattern agent-tags uses.
- [The watcher dies and nobody notices, since it has no pane] → `herdfile status` reports whether it
  is running, and every herdfile command warns when it is not.
- [Two workspaces start the same dev port] → out of scope. The second one fails loudly in its pane.
- [A wrong `ws remove` destroys work] → a worktree is only removed when its agent is idle and its
  branch is merged. Unmerged commits always stop removal and go to the operator.
- [Messages arrive while an agent is mid-task] → agent CLIs queue typed input while working. Blocked
  agents refuse messages, as herdr does.

## Migration Plan

There is no migration step. Write-back records whatever the file does not know, and a workspace
with no file is one where it knows nothing: on first sight the watcher writes the file from what is
on screen, sizes included, marking panes `unmanaged` unless their label is a service or `agent`.
Nothing is closed, moved, or resized. Rollout is by scope: `[watch] workspaces = ["hf-dogfood"]`
first, then `["*"]`. Uninstalling herdfile leaves herdr as it is.

## Open Questions

1. `tell`: fire and forget, or wait for the reply.
2. How `dir` is written: a path, or a short repo name resolved from a configured list.
