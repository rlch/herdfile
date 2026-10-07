## ADDED Requirements

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
SHALL use `herdr worktree create`. It SHALL write the workspace's own workspace file with
`[tab.main] row = ["agent"]` and, if `agent` is given, start the agent with the brief using the
configured launch command.

#### Scenario: Worktree workspace with an agent
- **WHEN** an agent runs `herdfile ws add review-pr-312 --dir ~/dev/schools-ts --branch review-pr-312 --brief briefs/review-312.md --model opus`
- **THEN** herdr shows a worktree workspace `review-pr-312` with an agent working on the brief, and
  the operator's focus has not moved

#### Scenario: Name already used
- **WHEN** `ws add` is given a name already in the file
- **THEN** it fails and nothing is created

### Requirement: Removal waits for idle and merged
`herdfile ws remove <name>` SHALL remove the entry and close the workspace only when its agent is
idle or done. A worktree SHALL be removed with `herdr worktree remove` only if its branch is merged
into its base. With unmerged commits, nothing SHALL be removed and an item SHALL be added to the
"needs you" list. The folder of a non-worktree workspace MUST NOT be deleted.

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
A workspace the operator closes by hand SHALL be removed from the file. A workspace created outside
herdfile SHALL be added as `unmanaged` and never removed automatically. `herdfile adopt --all` SHALL
add every open workspace as `unmanaged` without closing anything.

#### Scenario: Operator closes a workspace
- **WHEN** the operator closes workspace `review-pr-312` by hand
- **THEN** its entry is removed and nothing reopens it
