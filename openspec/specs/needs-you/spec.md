# needs-you Specification

## Purpose
TBD - created by archiving change declarative-workspaces. Update Purpose after archive.
## Requirements
### Requirement: One list of things waiting on the operator
herdfile SHALL keep one list at `$XDG_STATE_HOME/herdfile/needs.jsonl`, shown by `herdfile needs`.
Each entry SHALL have an id, the workspace name, a reason, and a time.

#### Scenario: Show the list
- **WHEN** the operator runs `herdfile needs`
- **THEN** every open entry is printed, oldest first, with its workspace and reason

### Requirement: Entries come from blocked agents, held removals, and agents asking
An entry SHALL be added when an agent turns `blocked`, when a removal is held by unmerged commits,
and when an agent runs `herdfile ask "<question>"`. Other tools MAY append entries in the documented
format.

#### Scenario: Agent asks
- **WHEN** an agent runs `herdfile ask "ship the migration tonight?"`
- **THEN** an entry with that question and the agent's workspace is added

### Requirement: Entries clear when the cause clears
An entry SHALL be cleared when its agent leaves `blocked`, when its branch merges, or with
`herdfile needs done <id>`.

#### Scenario: Approval answered
- **WHEN** the operator answers a blocked agent's approval
- **THEN** that agent's entry is cleared

