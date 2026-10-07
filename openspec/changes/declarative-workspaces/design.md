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
- The file of workspaces: worktree workspaces, the parent tree, messaging by name, the "needs you"
  list. Planned as a second change.
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

### One writer at a time

A file lock per workspace serialises apply, write-back, and `adopt`. This also closes the
find-or-create race that herdr itself does not guard against.

### No focus, no rearranging the operator's view

Every create and split passes `--no-focus`. The watcher defers moves and reorders in the tab the
operator is looking at until they leave it. Opens and closes there still happen.

## Risks / Trade-offs

- [Event-matching misreads a hand change as the watcher's own, or the reverse] → match on
  operation, label, and tab within a short window. Run a full snapshot diff after every apply as a
  backstop.
- [An agent edits the file constantly and the screen reshuffles] → agent guidance limits edits to
  when the task changes or the operator asks. The watcher debounces edits.
- [A closed service loses unsaved work] → services are commands, restartable by definition. Agents
  and `mine` panes are never force-closed.
- [Plugin startup is one-shot, re-run on handoff] → a detached process behind a lock, the same
  pattern agent-tags uses.
- [The watcher dies and nobody notices, since it has no pane] → `herdfile status` reports whether it
  is running, and every herdfile command warns when it is not.
- [Two workspaces start the same dev port] → out of scope. The second one fails loudly in its pane.

## Migration Plan

`adopt` writes a file for each open workspace from what is on screen, marking existing panes
`unmanaged` unless their label matches a service. Nothing is closed on the first apply of an adopted
file. Uninstalling herdfile leaves herdr as it is.

## Open Questions

1. How often agents may change the file.
2. Whether the file of workspaces ships in this change or the next.
