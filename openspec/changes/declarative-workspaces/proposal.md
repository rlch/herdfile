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
- New per-workspace file (not in git): the workspace's tabs, and in each tab a tree of rows and
  columns of panes with optional sizes, named by service (or `agent` for the workspace's own agent).
  Agents change it through `herdfile` commands that return once herdr matches.
- New watcher: when the workspace file changes, it opens, closes, and moves panes so that herdr
  matches the file. It finds panes by label, never steals focus, never closes a working or blocked
  agent, and creates everything through herdr's own commands.
- Write-back: when the operator closes a pane by hand, the watcher removes it from the file instead
  of reopening it. When a pane is opened outside herdfile (by hand or by an agent calling herdr
  directly), the watcher adds it to the file marked `unmanaged`, so it is never closed automatically.
  A workspace with no file is the same case: the watcher writes its file from what is on screen, so
  there is no separate adopt step. A `[watch] workspaces` setting limits which workspaces it manages.
- New file of workspaces, one level up: each entry is a workspace with a folder, an optional branch
  (which becomes a herdr worktree), a purpose, a parent, and an optional agent to start in it.
  Removing an entry removes the workspace once its agent is idle and its branch is merged.
- `parent` gives a tree. An orchestrator reads it to know its children; status is read live.
- Messaging by name: `herdfile tell <name> "..."` delivers a prompt to that workspace's agent.
- One "needs you" list collects everything waiting on the operator, instead of many panes.
- Out of this change (planned as follow-ups): writing back hand moves and reorders, and rewriting
  existing launcher skills on top of herdfile.

## Capabilities

### New Capabilities

- `services-file`: the repo-level `.herdr/services.toml` format: service names, commands, cwd,
  env, readiness text, and validation.
- `workspace-file`: the per-workspace file format, where it lives, how panes are named and ordered,
  and ownership marks (managed, operator's own, unmanaged).
- `apply`: the watcher that makes herdr match the workspace file: open, close, move, the idle rule
  for agents, no focus, finding panes by label, and ignoring its own changes.
- `write-back`: recording what is live into the file (hand closes, panes it does not know, a whole
  workspace on first sight, dragged sizes), and the rule when a hand change and a command collide.
- `workspaces-file`: the file of workspaces: entries, worktree creation, seeded agents, the parent
  tree, and removal rules.
- `messaging`: `tell` by name, including `tell parent`.
- `needs-you`: the single list of things waiting on the operator.

### Modified Capabilities

None. This is a new repo.

## Impact

- New repo `herdfile` (public, MIT). One Rust binary, installed as a herdr plugin whose startup hook
  runs a hidden watcher.
- Depends on herdr's CLI and socket API: `api snapshot`, `events.subscribe`, pane labels,
  `pane close`, `tab create`, `pane split`, `pane move`, `worktree create|remove`, `agent start`,
  `agent prompt`. Targets herdr 0.9.x.
- Repos that opt in add `.herdr/services.toml`. Nothing changes for repos that do not.
- Agent instructions (for example a global CLAUDE.md) will later point agents at the workspace
  file instead of telling them to open tabs directly. That edit is not part of this change.
