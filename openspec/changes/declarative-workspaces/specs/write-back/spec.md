## ADDED Requirements

### Requirement: Hand closes are written back
When a pane listed in the file is closed by something other than the watcher, the watcher SHALL
remove it from the file and MUST NOT reopen it.

#### Scenario: Operator closes dev
- **WHEN** the operator closes the `dev` pane by hand
- **THEN** `dev` is removed from the file and stays closed

### Requirement: Hand-opened panes are recorded
The watcher SHALL record a pane created in a managed workspace by something other than itself. It
SHALL add it to the file in its tab and position, labelled with a generated unique name if it has
none, and mark it `unmanaged`. herdr events do not say which client created a pane, so a hand split
and an outside agent's split look the same. The operator MAY change the mark to `mine`.

#### Scenario: Operator opens a shell
- **WHEN** the operator splits a new shell in tab `main`
- **THEN** the file lists it in `main` marked `unmanaged`, and apply never closes it

### Requirement: The watcher ignores its own changes
Events caused by the watcher's own operations MUST NOT be written back.

#### Scenario: Apply closes test
- **WHEN** apply closes `test` and herdr emits `pane.closed`
- **THEN** the file is not changed by write-back

### Requirement: Hand changes win collisions
When a hand change and a file edit touch the same pane before apply runs, the hand change SHALL
win, and the edit SHALL be reported as dropped.

#### Scenario: Close versus move
- **WHEN** an agent moves `dev` to tab `main` in the file, and the operator closes `dev` before
  apply runs
- **THEN** `dev` is removed from the file, is not reopened, and the drop is reported

### Requirement: Write-back keeps the file's text
Write-back SHALL change only the entries it must. Comments, key order, and formatting elsewhere in
the file MUST be preserved.

#### Scenario: Comment survives a hand close
- **WHEN** the file has `panes = ["agent", "test"]   # test on the right` and the operator closes
  `test`
- **THEN** the line becomes `panes = ["agent"]   # test on the right`
