# write-back Specification

## Purpose
TBD - created by archiving change declarative-workspaces. Update Purpose after archive.
## Requirements
### Requirement: Hand closes are written back
When a pane listed in the file is closed by something other than the watcher, the watcher SHALL
remove it from the file and MUST NOT reopen it.

#### Scenario: Operator closes dev
- **WHEN** the operator closes the `dev` pane by hand
- **THEN** `dev` is removed from the file and stays closed

### Requirement: Unknown panes are recorded
The watcher SHALL record every pane in a managed workspace that the file does not know, in its tab
and position, labelled with a generated unique name if it has none. A pane whose label is a service
or `agent` SHALL be recorded as managed; any other pane SHALL be marked `unmanaged`. herdr events do
not say which client created a pane, so a hand split and an outside agent's split look the same.
The operator MAY change the mark to `mine`. The tab's sizes SHALL be written from what is on screen,
so recording a pane moves no divider.

#### Scenario: Operator opens a shell
- **WHEN** the operator splits a new shell in tab `main`
- **THEN** the file lists it in `main` marked `unmanaged`, apply never closes it, and no divider
  moves

### Requirement: First sight records a workspace as it is
A managed workspace with no file is one whose panes are all unknown. The watcher SHALL write its
file from what is on screen: every tab, the tree of rows and columns with sizes, and the pane
recorded as `agent` being the first unlabelled pane that hosts an agent when none is labelled
`agent`. There is no separate adopt command. Recording a workspace MUST NOT close, move, or resize
any pane.

#### Scenario: A workspace seen for the first time
- **WHEN** a workspace has tab `1` with an agent pane at 65% and a bare shell, and no file
- **THEN** its file has `[tab.1]` with `{ pane = "agent", size = 65 }` and an `unmanaged` entry for
  the shell, both panes are labelled in herdr, and nothing on screen changes

#### Scenario: Apply after first sight
- **WHEN** the watcher has just recorded a workspace and apply runs
- **THEN** herdr is unchanged

### Requirement: Tab renames are written back
When a tab is renamed by something other than the watcher, the watcher SHALL rename that tab in the
file, keeping its place and contents, and MUST NOT move panes to restore the old name.

#### Scenario: An agent titles its tab
- **WHEN** a tab listed as `[tab.1]` is renamed to `my title` outside herdfile
- **THEN** the file lists `[tab."my title"]` with the same panes, and no pane moves

### Requirement: Layout changes are written back
The watcher SHALL write a tab into the file as it is on screen when the tab's layout (its rows,
columns, and which pane is where) was changed by something other than the watcher, including a
pane moved in from another tab or two panes swapped, and MUST NOT move panes back. Entries the
file lists for that tab that are not on screen yet SHALL keep their place after their neighbour.

#### Scenario: Pane moved to another tab
- **WHEN** `test` is moved by hand from tab `main` to tab `services`
- **THEN** the file lists `test` in `services`, not in `main`, and it stays where it was put

#### Scenario: Panes swapped
- **WHEN** `agent` and `test` are swapped by hand in `row = ["agent", "test"]`
- **THEN** the file has `row = ["test", "agent"]` and they stay swapped

#### Scenario: Placed while the operator splits
- **WHEN** an agent places `logs` after `agent` and the operator splits a pane in the same tab
  before apply runs
- **THEN** the file keeps `logs` after `agent`, records the new pane, and `logs` is opened

#### Scenario: Unnamed tabs keep their names
- **WHEN** a tab still has herdr's default name (its position) and another tab closes
- **THEN** the tab keeps the name the file uses for it

### Requirement: Dragged sizes are written back
When the operator changes a split ratio by hand, the watcher SHALL write the new size into the file,
rounded to the nearest 5%. If the rounded size equals the file's size, the file SHALL NOT change.

#### Scenario: Drag a divider
- **WHEN** `agent` has `size = 60` and the operator drags it to 73%
- **THEN** the file has `size = 75` and apply does not resize the pane again

#### Scenario: Small drag
- **WHEN** `agent` has `size = 60` and the operator drags it to 61%
- **THEN** the file is unchanged

### Requirement: The watcher ignores its own changes
Events caused by the watcher's own operations MUST NOT be written back.

#### Scenario: Apply closes test
- **WHEN** apply closes `test` and herdr emits `pane.closed`
- **THEN** the file is not changed by write-back

### Requirement: Hand changes win collisions
The hand change SHALL win when a hand change (a close or a move) and a command touch the same pane
before apply runs, and the command SHALL report that its change was dropped.

#### Scenario: Close versus move
- **WHEN** an agent runs `herdfile place dev --tab main`, and the operator closes `dev` before
  apply runs
- **THEN** `dev` is removed from the file, is not reopened, and the drop is reported

### Requirement: Write-back keeps the file's text
Write-back SHALL change only the entries it must. Key order and formatting elsewhere in the file MUST be
preserved.

#### Scenario: Hand close changes one line
- **WHEN** the operator closes `test` in tab `main`
- **THEN** only the `row` line of `[tab.main]` changes

