# workspaces-file Specification

## Purpose
TBD - created by archiving change declarative-workspaces. Update Purpose after archive.
## Requirements
### Requirement: One file lists the workspaces
herdfile SHALL keep one file of workspaces per herdr server at
`$XDG_STATE_HOME/herdfile/workspaces.toml`. Each key SHALL be a workspace name, used as the herdr
workspace label. Each entry SHALL have `dir` and MAY have `branch`, `purpose`, `parent`, and
`agent = { brief, model }`. The file SHALL be written only by herdfile commands.

#### Scenario: List the tree
- **WHEN** an agent runs `herdfile tree`
- **THEN** it prints each workspace with its purpose, its parent, and its agent's live status

### Requirement: Adding an entry opens the workspace
`herdfile ws add` SHALL create the workspace through herdr with `--no-focus`. With `branch`, it
SHALL use `herdr worktree create`, or `herdr worktree open` when the branch already exists (fetched
ahead, or its workspace was closed). A parent that is open but not yet in the file SHALL be recorded
on the spot. It SHALL write the workspace's own workspace file with
`[tab.main] row = ["agent"]` and, if `agent` is given, start the agent and then send the brief as
its first prompt once herdr reports it ready.

#### Scenario: Default start
- **WHEN** no `command` is configured and `kind = "claude"`, `args = ["--model", "{model}"]`
- **THEN** herdfile runs `herdr agent start review-pr-312 --kind claude --pane <new pane> -- --model opus`,
  then `herdr agent prompt review-pr-312 "Read briefs/review-312.md and follow it."`

#### Scenario: Wrapper command
- **WHEN** `command = "cl --{model}"` is configured
- **THEN** herdfile types `cl --opus` at the new pane's shell prompt, waits for herdr to detect the
  agent, names it `review-pr-312`, and sends the brief as its first prompt

#### Scenario: Worktree workspace with an agent
- **WHEN** an agent runs `herdfile ws add review-pr-312 --dir ~/dev/schools-ts --branch review-pr-312 --brief briefs/review-312.md --model opus`
- **THEN** herdr shows a worktree workspace `review-pr-312` with an agent working on the brief, and
  the operator's focus has not moved

#### Scenario: Name already used
- **WHEN** `ws add` is given a name already in the file
- **THEN** it fails and nothing is created

### Requirement: Removal waits for idle and merged
`herdfile ws remove <name>` SHALL remove the entry and close the workspace only when its agent is
idle or done. A workspace that herdr reports as a linked worktree, whoever opened it, SHALL be
removed with `herdr worktree remove` only if its branch (or, when detached, its commit) is merged
into its base, squash merges included. With unmerged commits, nothing SHALL be removed and an item SHALL be added to the
"needs you" list. The folder of a non-worktree workspace MUST NOT be deleted.

#### Scenario: Primary workspace with linked worktrees
- **WHEN** an entry without `branch` is removed while herdr links worktree workspaces to it
- **THEN** herdfile does not pass `--group`, the workspace stays open, and "needs you" says why

#### Scenario: Worktree opened outside herdfile
- **WHEN** a worktree workspace opened with plain `herdr worktree create` is recorded on first sight
  and later removed with `ws remove` after its branch merged
- **THEN** its entry carries the branch, and the worktree goes through `herdr worktree remove`

#### Scenario: Merged branch
- **WHEN** `review-pr-312` is removed, its agent is idle, and its branch is merged
- **THEN** the worktree is removed and the workspace closes

#### Scenario: Unmerged commits
- **WHEN** `review-pr-312` is removed and its branch has unmerged commits
- **THEN** the workspace stays, and "needs you" says it has unmerged commits

#### Scenario: Working agent
- **WHEN** `review-pr-312` is removed while its agent is working
- **THEN** nothing closes until the agent is idle, then the merged/unmerged rule applies

### Requirement: Parents form a tree
`parent` SHALL name another entry or `operator`. Removing a parent SHALL re-parent its children to
the removed entry's parent. A cycle MUST be rejected.

#### Scenario: Parent removed
- **WHEN** `land-prs` is removed and `review-pr-312` has `parent = "land-prs"`
- **THEN** `review-pr-312` gets `land-prs`'s parent

### Requirement: Hand changes to workspaces are written back
A workspace the operator closes by hand SHALL be removed from the file; one renamed by hand SHALL
keep its entry under the new name, with its children following. A workspace created outside
herdfile SHALL be added as `unmanaged` and never removed automatically. The first time the watcher
sees an open workspace in scope that the file does not list, it SHALL add it as `unmanaged` without
closing anything.

#### Scenario: Operator closes a workspace
- **WHEN** the operator closes workspace `review-pr-312` by hand
- **THEN** its entry is removed and nothing reopens it

