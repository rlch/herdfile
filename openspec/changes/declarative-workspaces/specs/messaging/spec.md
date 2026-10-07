## ADDED Requirements

### Requirement: Tell by name
`herdfile tell <name> "<text>"` SHALL deliver the text to the named workspace's agent with
`herdr agent prompt`, prefixed with `From <sender>: ` where the sender is the calling workspace's
name. `parent` SHALL resolve to the caller's parent.

#### Scenario: Child reports to parent
- **WHEN** the agent in `review-pr-312` runs `herdfile tell parent "approved, ready to merge"`
- **THEN** the agent in `land-prs` receives `From review-pr-312: approved, ready to merge`

#### Scenario: Unknown name
- **WHEN** `tell` names a workspace that is not in the file
- **THEN** it fails with the list of known names

### Requirement: Blocked agents get queued messages
If the target agent is blocked, herdfile SHALL queue the message, deliver it when the agent leaves
`blocked`, and return immediately saying it was queued.

#### Scenario: Target blocked
- **WHEN** `tell land-prs "..."` runs while `land-prs` is waiting for approval
- **THEN** the message is delivered after the operator answers the approval
