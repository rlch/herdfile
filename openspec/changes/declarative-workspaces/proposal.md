## Why

AI agents running in herdr have no shared picture of what a workspace should look like. Each one
opens tabs and panes its own way, nothing closes them, and the result is dozens of idle shells and
finished agents that the operator has to clean up by hand. A file that says what each workspace
contains gives agents one place to read the layout and one place to change it. A watcher then makes
herdr match that file, and anything left out of the file gets closed.

## What Changes

- New repo file `.herdr/services.toml`: named commands a repo can run (`[dev]`, `[test]`), each
  with a command, an optional working directory, and optional readiness text. It says nothing about
  tabs or panes.
- New per-workspace file (not in git): the workspace's tabs, and the panes in each tab, left to
  right, named by service (or `agent` for the workspace's own agent).
- New watcher: when the workspace file changes, it opens, closes, and moves panes so that herdr
  matches the file. It finds panes by label, never steals focus, never closes a working or blocked
  agent, and creates everything through herdr's own commands.
- New `adopt` command: writes a workspace file for an already-open workspace from what is on screen.
- Write-back: when the operator closes a pane by hand, the watcher removes it from the file instead
  of reopening it. When the operator opens a shell by hand, the watcher adds it to the file and
  marks it as theirs, so it is never closed automatically.
- Out of this change (planned as follow-ups): writing back hand moves and reorders, the file of
  workspaces above this one (worktree workspaces, a parent tree, messaging by name, a "needs you"
  list), and rewriting existing launcher skills on top of it.

## Capabilities

### New Capabilities

- `services-file`: the repo-level `.herdr/services.toml` format: service names, commands, cwd,
  env, readiness text, and validation.
- `workspace-file`: the per-workspace file format, where it lives, how panes are named and ordered,
  and ownership marks (managed, operator's own, unmanaged).
- `apply`: the watcher that makes herdr match the workspace file: open, close, move, the idle rule
  for agents, no focus, finding panes by label, and ignoring its own changes.
- `adopt`: creating a workspace file from a live workspace.
- `write-back`: recording the operator's hand closes and hand-opened shells into the file, and the
  rule when a hand change and an agent edit collide.

### Modified Capabilities

None. This is a new repo.

## Impact

- New repo `herdfile` (public, MIT). New binary and herdr integration (plugin or daemon, decided in
  design).
- Depends on herdr's CLI and socket API: `api snapshot`, `events.subscribe`, pane labels,
  `pane close`, `tab create`, `pane split`, `pane move`. Targets herdr 0.9.x.
- Repos that opt in add `.herdr/services.toml`. Nothing changes for repos that do not.
- Agent instructions (for example a global CLAUDE.md) will later point agents at the workspace
  file instead of telling them to open tabs directly. That edit is not part of this change.
