## ADDED Requirements

### Requirement: Services are named commands in the repo
A repo SHALL declare services in `.herdr/services.toml`. Each top-level table is one service. Its
name is the table key. Each service MUST have `cmd` and MAY have `cwd` (relative to the repo root),
`env` (a table of strings), and `ready` (text that means the service is up).

#### Scenario: Valid file
- **WHEN** `.herdr/services.toml` contains `[dev]` with `cmd = "pnpm dev"` and `ready = "ready in"`
- **THEN** a service named `dev` is available to every workspace whose directory is in that repo

#### Scenario: Missing cmd
- **WHEN** a service table has no `cmd`
- **THEN** loading fails with an error naming the service and the file, and no pane is opened for it

### Requirement: The name agent is reserved
A service MUST NOT be named `agent`. That name means the workspace's own agent pane.

#### Scenario: Service named agent
- **WHEN** `.herdr/services.toml` contains `[agent]`
- **THEN** loading fails with an error saying the name is reserved

### Requirement: The services file says nothing about placement
The services file SHALL NOT contain tabs, panes, splits, or focus. Unknown keys MUST be rejected.

#### Scenario: Placement key in services file
- **WHEN** a service has `tab = "main"`
- **THEN** loading fails with an error naming the unknown key
