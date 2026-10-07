## ADDED Requirements

### Requirement: Workspace agents carry herdr agent names
A workspace name MUST match herdr's agent-name pattern `[a-z][a-z0-9_-]{0,31}`. Whenever herdfile
starts a workspace's agent, it SHALL give it the workspace's name as its herdr agent name.

#### Scenario: Direct herdr use
- **WHEN** workspace `review-pr-312` has a running agent
- **THEN** `herdr agent prompt review-pr-312 "..."` reaches it without herdfile

#### Scenario: Invalid name
- **WHEN** `ws add` is given `Review PR 312`
- **THEN** it fails, quoting the allowed pattern

### Requirement: Tell by name
`herdfile tell <name> "<text>"` SHALL call `herdr agent prompt` on the named agent with the text
prefixed `From <sender>: `, where the sender is the calling workspace's name. `parent` SHALL resolve
to the caller's parent. `--wait`, `--until`, and `--timeout` SHALL pass through to herdr. With
`--wait`, it SHALL print the reply read with `herdr agent read --source recent-unwrapped`. herdr's
errors SHALL pass through unchanged.

#### Scenario: Child reports to parent
- **WHEN** the agent in `review-pr-312` runs `herdfile tell parent "approved, ready to merge"`
- **THEN** the agent in `land-prs` receives `From review-pr-312: approved, ready to merge`

#### Scenario: Unknown name
- **WHEN** `tell` names a workspace that is not in the file
- **THEN** it fails with the list of known names

#### Scenario: Wait for a reply
- **WHEN** `land-prs` runs `herdfile tell review-pr-312 --wait "is it approved?"`
- **THEN** it returns when that agent settles, printing its reply

### Requirement: Blocked agents refuse messages
herdfile MUST NOT queue or retry a message to a blocked agent. It SHALL fail with herdr's
`agent_blocked` error.

#### Scenario: Target blocked
- **WHEN** `tell land-prs "..."` runs while `land-prs` is waiting for approval
- **THEN** the command fails with `agent_blocked` and nothing is typed into that pane
