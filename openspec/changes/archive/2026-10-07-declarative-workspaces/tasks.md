## 1. Setup

- [x] 1.1 Scaffold the Rust crate (`herdfile` binary, `toml_edit`), CI, and release builds for macOS and Linux
- [x] 1.4 Plugin manifest with `[[startup]]` running `herdfile watch --detach`; single-instance lock; log file; `herdfile status`
- [x] 1.2 Backend trait plus its herdr implementation: `api snapshot`, `events.subscribe`, create/split/close/move/rename, with `--no-focus` on every create
- [x] 1.3 Per-workspace lock
- [x] 1.5 Plugin `[[build]]`: download the matching release binary, fall back to `cargo install`

## 2. Files

- [x] 2.1 Parse and validate `.herdr/services.toml` (required `cmd`, reserved `agent`, unknown keys rejected)
- [x] 2.2 Parse and validate the workspace file (unique names, known names, ownership marks)
- [x] 2.3 State-folder location and `herdfile path`
- [x] 2.4 Layout tree: parse rows, columns, sizes; validate sizes per parent
- [x] 2.5 `herdfile place`, `remove`, `set`, `show` under the workspace lock; local socket to ask the watcher to apply now; wait for the snapshot to match

## 3. Apply

- [x] 3.1 Diff desired (file) against live (snapshot) by label: open, close, move
- [x] 3.7 Map the n-ary tree to herdr's binary splits and ratios; reshape with `pane split`, `pane move`, `layout.set_split_ratio`
- [x] 3.2 Open a service pane: split in place, set label, run `cmd` in `cwd` with `env`
- [x] 3.3 Busy-agent rule: mark for removal, close on next `idle`/`done`, cancel if re-added
- [x] 3.4 Defer moves and reorders in the tab the operator is viewing
- [x] 3.5 Invalid file changes nothing and reports the error with file and line
- [x] 3.6 Debounce file edits; full snapshot diff after each apply

## 4. First sight

- [x] 4.1 A workspace with no file is recorded by write-back from the snapshot (labels for unlabelled panes, `agent` for the first agent pane, sizes from screen); no adopt command
- [x] 4.2 `[watch] workspaces` scope; first apply after first sight changes nothing
- [x] 4.3 `herdfile plan`: a pass recorded against a backend that changes nothing

## 5. Write-back

- [x] 5.1 Record the watcher's own operations and ignore matching events
- [x] 5.2 Hand close removes the pane from the file
- [x] 5.3 Hand-opened pane is added as `unmanaged` in its tab and position
- [x] 5.5 Dragged sizes: on `layout_updated`, read ratios, round to 5%, write back if changed
- [x] 5.4 Collision rule: hand change wins, dropped edit is reported
- [x] 5.6 Layout changes made outside herdfile (moves between tabs, swaps, splits) are written back; tab renames too; unnamed tabs are pinned

## 6. Workspaces file

- [x] 6.1 Parse and validate `workspaces.toml` (unique names, known parents, no cycles)
- [x] 6.2 `herdfile ws add`: `herdr worktree create --no-focus` or workspace on `dir`, write its workspace file, start the agent (`agent start --kind` or a typed wrapper `command`), wait for ready, send the brief with `agent prompt`
- [x] 6.3 `herdfile ws remove`: idle wait, merged check, `herdr worktree remove`, re-parent children
- [x] 6.4 `herdfile tree` with live status; workspace write-back, including first sight

## 7. Messaging and needs you

- [x] 7.1 Name each workspace agent after its workspace (`agent start <name>` / `agent rename`); validate names against herdr's pattern
- [x] 7.4 `herdfile tell <name|parent>`: sender prefix, pass `--wait/--until/--timeout` to `herdr agent prompt`, print reply via `agent read`, pass errors through
- [x] 7.2 `needs.jsonl` format (documented), `herdfile needs`, `needs done`, `ask`
- [x] 7.3 Add and clear entries on blocked, held removal, merge

## 8. Verify

- [x] 8.1 Integration tests against a throwaway herdr server, one per spec scenario
- [x] 8.2 Dogfood: scope one workspace, then all; confirm nothing closes or moves, then edit one file by hand
