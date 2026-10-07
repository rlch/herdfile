# herdfile

Declarative workspaces for [herdr](https://herdr.dev).

A repo says what services it has. A per-workspace file says what is on screen:
which tabs, which panes in each. A hidden watcher makes herdr match the file,
and writes hand changes back into it. One level up, a file of workspaces says
which workspaces exist, what each is for, and who its parent is. Built so AI
agents can read and change the layout of your terminal workspaces without
guessing, and leftovers get closed.

Targets herdr 0.9.x. The design and its decisions are in
`openspec/specs/` (the change that built it: `openspec/changes/archive/2026-10-07-declarative-workspaces/`).

## Install

```sh
herdr plugin install rlch/herdfile
```

The plugin's build step downloads the release binary for your machine into the
plugin folder (or builds it with `cargo install` when there is none). Its
startup hook runs `herdfile watch --detach`: a background watcher with no pane,
one per herdr server, behind a lock. `herdfile status` says whether it is
running; every command warns when it is not. It logs to
`$XDG_STATE_HOME/herdfile/watch.log` (default `~/.local/state/herdfile/`).

Which workspaces the watcher manages is a setting (default: all). To try it on
one workspace first, set this before installing, then widen it:

```toml
# ~/.config/herdfile/config.toml
[watch]
workspaces = ["hf-dogfood"]       # labels; a trailing * matches a prefix; "*" is all
```

Before widening the scope, `herdfile plan` prints what the watcher would do to
each workspace in scope (labels, recorded panes, and any open, close, move or
resize) without changing anything.

To put `herdfile` itself on your PATH: `cargo install --git https://github.com/rlch/herdfile`.

## Services: `.herdr/services.toml`

Committed in the repo. Named commands, nothing about placement.

```toml
[dev]
cmd = "pnpm dev"
ready = "ready in"        # optional: output that means it is up

[test]
cmd = "pnpm test --watch"
cwd = "web"               # optional, relative to the repo root
env = { CI = "1" }        # optional
```

`agent` is reserved for the workspace's own agent pane. Unknown keys are
rejected.

## The workspace file

One per herdr workspace, at `$XDG_STATE_HOME/herdfile/<workspace-id>.toml`.
`herdfile path` prints it for `$HERDR_WORKSPACE_ID`. Not in git.

```toml
dir = "/Users/me/dev/app"   # where .herdr/services.toml is looked up

[tab.main]
row = ["agent", { column = ["test", { pane = "dev", size = 30 }], size = 40 }]

[tab.scratch]
row = [{ pane = "shell-1", mark = "unmanaged", cwd = "/Users/me/dev/app" }]
```

Each tab has one `row` (left to right) or `column` (top to bottom). An entry is
a pane name, `{ pane = ..., size = ... }`, or a nested `{ row = [...] }` /
`{ column = [...] }` with an optional `size`. `size` is a percent of the parent;
entries without one share the rest. A name is a service, `agent`, or a pane
marked `mine` (the operator's own) or `unmanaged` (opened outside herdfile).
Marked panes are never closed by the watcher.

Agents change the file only through commands. Each takes the workspace lock,
edits the current file, asks the watcher to apply, and returns once herdr
matches (`--no-wait` returns after the edit):

```sh
herdfile place dev --tab services     # add, or move if already placed
herdfile place logs --after agent     # also --before, --right-of, --left-of, --above, --below
herdfile place test --size 30
herdfile remove test                  # closes it; a busy agent closes once idle
herdfile set main 'row = ["agent", { column = ["test", "dev"] }]'
herdfile mark shell-1 mine
herdfile show
herdfile apply                        # apply now
```

What the watcher does:

- records what the file does not know: a pane opened by hand or by an agent
  calling herdr directly, and on first sight a whole workspace, exactly as it
  is on screen, sizes included. Services and `agent` are recorded as managed,
  anything else as `unmanaged`. Nothing is closed, moved or resized by this;
- finds panes by label (its identity), never by id;
- opens services in their `cwd` with `env`, and closes managed panes the file
  drops, except agents that are `working` or `blocked`: those close on their
  next `idle`;
- reshapes tabs with herdr's split, swap, and move (never `layout.apply`), and
  sets split ratios; nothing is restarted;
- never takes focus, and leaves moves in the tab you are looking at until you
  leave it (opens and closes still happen);
- writes your hand changes back: a pane you close leaves the file, a pane you
  move or swap is moved in the file, a tab you rename is renamed, and a
  divider you drag is written as a size rounded to 5%. It never moves a pane
  back. If your hand change and an agent's
  command hit the same pane, yours wins and the command says its change was
  dropped.

## The file of workspaces

One per herdr server, at `$XDG_STATE_HOME/herdfile/workspaces.toml`. Each key is
a workspace name: its herdr label and its agent's herdr name, so it must match
`[a-z][a-z0-9_-]{0,31}`.

```toml
[land-prs]
dir = "~/dev/app"
purpose = "land the open PRs"
parent = "operator"

[review-pr-312]
dir = "~/dev/app"
branch = "review-pr-312"          # a herdr worktree
purpose = "review PR 312"
parent = "land-prs"
agent = { brief = "briefs/review-312.md", model = "opus" }
```

```sh
herdfile ws add review-pr-312 --dir ~/dev/app --branch review-pr-312 \
  --parent land-prs --purpose "review PR 312" --brief briefs/review-312.md --model opus
herdfile ws remove review-pr-312
herdfile tree                     # names, purposes, parents, live agent status
```

`ws add` creates the workspace with `herdr worktree create --no-focus` (with
`--branch`; `worktree open` when the branch already exists, e.g. fetched ahead or
left by a closed workspace) or `workspace create`, writes its workspace file with the agent in
tab `main`, starts the agent named after the workspace, waits for it, and sends
`Read <brief> and follow it.` `--parent` defaults to the calling workspace,
recorded on the spot if the watcher has not seen it yet.

`ws remove` waits for the agent to be idle. A worktree, including one opened
outside herdfile, goes (`herdr worktree remove`) only once its branch (or its
commit, when detached) is merged into its base, squash merges included;
with unmerged commits nothing is removed and it goes on the "needs you" list.
A workspace without a branch is closed, never with `--group`, and its folder is
kept. Children of a removed workspace move up to its parent. A workspace you
close by hand leaves the file; one opened outside herdfile, or open before
herdfile was, is recorded as `unmanaged`.

### Agent command

`~/.config/herdfile/config.toml` (or `$XDG_CONFIG_HOME/herdfile/config.toml`, or the
path in `$HERDFILE_CONFIG`):

```toml
[agent]
kind = "claude"                 # herdr agent start --kind
args = ["--model", "{model}"]   # {model} from --model; dropped when none

# Optional: start through a wrapper typed at the pane's shell prompt, so shell
# functions work. herdfile waits for herdr to detect the agent, then names it.
command = "cl --{model}"
```

The default runs `herdr agent start <name> --kind claude --pane <pane> --
--model <model>`. Setting `command = "cl --{model}"` starts agents through the
`cl` wrapper instead (it picks the account and maps `--opus` to the 1M model).

## Messages and "needs you"

```sh
herdfile tell review-pr-312 "is it approved?" --wait --timeout 120000
herdfile tell parent "approved, ready to merge"
herdfile ask "ship the migration tonight?"
herdfile needs                    # everything waiting on the operator, oldest first
herdfile needs done <id>
```

`tell` is `herdr agent prompt` with `From <your workspace>: ` in front and
`parent` resolved through the tree. It reaches the workspace's agent by its pane,
so agents herdfile did not start are reachable too. `--wait`, `--until`, and
`--timeout` pass through; with `--wait` only the agent's reply is printed. herdr's
errors pass through unchanged: a blocked agent refuses with `agent_blocked`.

### `needs.jsonl`

`$XDG_STATE_HOME/herdfile/needs.jsonl`, one JSON object per line. Other tools
may append lines.

```json
{"id":"n1b2c3","workspace":"review-pr-312","reason":"branch `review-pr-312` has unmerged commits; not removed","source":"held-removal","time":"2026-10-07T06:00:00Z"}
```

| field | meaning |
| --- | --- |
| `id` | unique; `herdfile needs done <id>` clears it |
| `workspace` | the workspace's name |
| `reason` | what is waiting, in a sentence |
| `source` | `blocked`, `held-removal`, `ask`, or the adding tool's name |
| `time` | RFC 3339, UTC |
| `pane` | optional; for `blocked`, the pane it is about |

The watcher adds a `blocked` entry when an agent turns blocked and clears it
when it leaves blocked. A held removal clears when the branch merges.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Unit tests sit beside the code and need nothing running; several read a real
herdr snapshot from `tests/fixtures/snapshot.json`. Integration tests are one
crate, `tests/it/`, and each starts its own herdr server: a named session under
a fresh `XDG_CONFIG_HOME` in `/tmp`, never your running one (the harness refuses
the default socket). They are built not to disturb you:

- at most 3 servers run at once (`HERDFILE_TEST_SERVERS=n` to change), so a
  plain `cargo test` is light;
- if the test process dies (Ctrl-C), a watchdog stops its servers and removes
  their folders;
- servers run with sounds and pop-ups off, panes run a plain shell, and git
  ignores your global config (no hooks, no signing prompts);
- agents are a fake `claude` built from `tests/fixtures/fake_claude.rs`, and the
  harness checks a pane resolves `claude` to it before any test starts one.

`HERDFILE_KEEP_TEST_DIR=1` keeps each test's folder for inspection.
