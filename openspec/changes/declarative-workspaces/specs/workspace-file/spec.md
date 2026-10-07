## ADDED Requirements

### Requirement: One file per workspace lists its tabs and panes
Each managed workspace SHALL have one workspace file, not committed to git. Each `[tab.<label>]`
table is one tab, in file order. Its `panes` array lists pane names left to right. A name is a
service from the repo's services file, `agent`, or a pane marked `mine` or `unmanaged`.

#### Scenario: Two tabs
- **WHEN** the file has `[tab.main] panes = ["agent", "test"]` and `[tab.services] panes = ["dev"]`
- **THEN** the desired workspace is a tab `main` with `agent` on the left and `test` on the right,
  then a tab `services` with `dev`

#### Scenario: Unknown name
- **WHEN** a pane name is neither a service, `agent`, nor a marked pane
- **THEN** the file is rejected with an error naming the pane, and herdr is not changed

### Requirement: Workspace files live in the state folder
Each workspace file SHALL be stored at `$XDG_STATE_HOME/herdfile/<workspace-id>.toml`, defaulting to
`~/.local/state/herdfile/<workspace-id>.toml`. `herdfile path` SHALL print the file for
`$HERDR_WORKSPACE_ID`, or for a workspace given as an argument.

#### Scenario: Agent finds its file
- **WHEN** an agent in workspace `w3` runs `herdfile path`
- **THEN** it prints `~/.local/state/herdfile/w3.toml` (expanded)

#### Scenario: Two workspaces on one folder
- **WHEN** workspaces `w3` and `w7` both have cwd `~/dev`
- **THEN** each has its own file

### Requirement: Names are unique within a workspace
A pane name MUST appear at most once in a workspace file.

#### Scenario: Duplicate name
- **WHEN** `dev` appears in two tabs
- **THEN** the file is rejected with an error naming both tabs

### Requirement: Ownership marks
A pane entry SHALL be managed by default. A pane MAY be marked `mine` (opened by the operator) or
`unmanaged` (opened outside the file), with its cwd recorded so it can be identified.

#### Scenario: Operator's shell recorded
- **WHEN** the file contains a `mine` pane named `shell-1`
- **THEN** the watcher never closes it

### Requirement: Live state is never stored
The workspace file MUST NOT contain pane ids, terminal ids, agent status, running or exited state,
sizes, or focus.

#### Scenario: Write-back after a status change
- **WHEN** an agent's status changes from `working` to `idle`
- **THEN** the workspace file is not rewritten
