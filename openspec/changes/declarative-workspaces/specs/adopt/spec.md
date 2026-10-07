## ADDED Requirements

### Requirement: Adopt writes a file from a live workspace
`herdfile adopt [<workspace>]` SHALL write a workspace file describing the workspace's current tabs
and panes. Panes whose label matches a service or that host the workspace's agent are written as
managed. All other panes are written as `unmanaged`.

#### Scenario: Adopt a workspace
- **WHEN** a workspace has tab `1` with an agent pane and a bare shell
- **THEN** the file has `[tab.1]` with `agent` and an `unmanaged` entry for the shell

#### Scenario: Unlabelled pane
- **WHEN** a pane has no label
- **THEN** adopt labels it with a generated unique name and records that name

### Requirement: Adopting closes nothing
The first apply after adopt MUST NOT close or move any pane.

#### Scenario: Apply after adopt
- **WHEN** adopt has just written a file and apply runs
- **THEN** herdr is unchanged

### Requirement: Adopt does not overwrite
Adopt MUST NOT overwrite an existing workspace file unless `--force` is given.

#### Scenario: File exists
- **WHEN** adopt runs for a workspace that already has a file
- **THEN** it exits with an error naming the file
