## ADDED Requirements

### Requirement: herdr converges to the workspace file
When a workspace file changes, the watcher SHALL open, close, and move panes so that the
workspace's labelled panes match the file. Panes are matched by label.

#### Scenario: Replace a pane
- **WHEN** `[tab.main] panes = ["agent", "test"]` is changed to `["agent", "logs"]`
- **THEN** the pane labelled `test` is closed and a pane labelled `logs` running the `logs` service
  is opened to the right of `agent`

#### Scenario: Move a pane between tabs
- **WHEN** `dev` is removed from `[tab.services]` and added to `[tab.main]`
- **THEN** the existing `dev` pane is moved into `main` without restarting its process

#### Scenario: Missing pane is opened
- **WHEN** the file lists `dev` and no pane labelled `dev` exists
- **THEN** a pane labelled `dev` is created in the service's cwd and its `cmd` is run

### Requirement: Not in the file means closed
A managed pane whose label is not in the file SHALL be closed.

#### Scenario: Remove a service
- **WHEN** `test` is deleted from the file
- **THEN** the pane labelled `test` is closed

### Requirement: Busy agents are never closed
A pane whose agent status is `working` or `blocked` MUST NOT be closed by apply. It SHALL be closed
on its next transition to `idle` or `done` if it is still absent from the file.

#### Scenario: Remove a working agent
- **WHEN** `agent` is removed from the file while its status is `working`
- **THEN** the pane stays open, and is closed when its status becomes `idle`

#### Scenario: Re-added before idle
- **WHEN** a pane pending removal is added back to the file before it goes idle
- **THEN** the pending removal is cancelled

### Requirement: Marked panes are never closed
Apply MUST NOT close panes marked `mine` or `unmanaged`.

#### Scenario: Unmanaged pane
- **WHEN** an `unmanaged` pane is removed from the file
- **THEN** its entry is dropped and the pane stays open

### Requirement: Never take focus
Every create, split, and move SHALL pass `--no-focus` or its socket equivalent. Apply MUST NOT call
any focus command. Moves and reorders in the tab the operator is viewing SHALL wait until the
operator leaves that tab.

#### Scenario: Open a pane in a background tab
- **WHEN** apply opens `dev` in tab `services` while the operator is in tab `main`
- **THEN** the operator stays in `main`

### Requirement: Only herdr's own commands
Apply SHALL create and change panes, tabs, and worktrees only through herdr's CLI or socket API.

#### Scenario: Pane creation
- **WHEN** apply opens a pane
- **THEN** the pane appears in `herdr api snapshot` with its label set

### Requirement: One writer at a time
Apply, write-back, and adopt SHALL hold a lock per workspace while they read herdr and change it or
the file.

#### Scenario: Two edits at once
- **WHEN** two agents edit the file within the same second
- **THEN** apply runs once per settled file, and no pane is created twice

### Requirement: Invalid files change nothing
If the workspace file or services file fails to load, apply MUST NOT change herdr and SHALL report
the error where the editing agent can read it.

#### Scenario: Syntax error
- **WHEN** an agent saves a file with a TOML syntax error
- **THEN** no pane is opened or closed, and the error is reported with the file and line

### Requirement: The watcher runs hidden in the background
The watcher SHALL be started by the herdr plugin's startup hook as a detached background process
with no pane. At most one watcher SHALL run per herdr server. `herdfile status` SHALL report whether
it is running, and other herdfile commands SHALL warn when it is not.

#### Scenario: herdr starts
- **WHEN** herdr starts with the plugin installed
- **THEN** one watcher is running and no pane or tab was created for it

#### Scenario: Startup hook runs twice
- **WHEN** herdr re-runs the startup hook after a live handoff
- **THEN** the second `herdfile watch --detach` sees the lock and exits, leaving one watcher

#### Scenario: Watcher is down
- **WHEN** an agent runs `herdfile path` and no watcher is running
- **THEN** the path is printed with a warning that edits will not be applied
