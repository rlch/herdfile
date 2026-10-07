## 1. Setup

- [ ] 1.1 Scaffold the Rust crate (`herdfile` binary, `toml_edit`), CI, and release builds for macOS and Linux
- [ ] 1.4 Plugin manifest with `[[startup]]` running `herdfile watch --detach`; single-instance lock; log file; `herdfile status`
- [ ] 1.2 Backend trait plus its herdr implementation: `api snapshot`, `events.subscribe`, create/split/close/move/rename, with `--no-focus` on every create
- [ ] 1.3 Per-workspace lock
- [ ] 1.5 Plugin `[[build]]`: download the matching release binary, fall back to `cargo install`

## 2. Files

- [ ] 2.1 Parse and validate `.herdr/services.toml` (required `cmd`, reserved `agent`, unknown keys rejected)
- [ ] 2.2 Parse and validate the workspace file (unique names, known names, ownership marks)
- [ ] 2.3 State-folder location and `herdfile path`
- [ ] 2.4 Layout tree: parse rows, columns, sizes; validate sizes per parent
- [ ] 2.5 `herdfile place`, `remove`, `set`, `show` under the workspace lock; local socket to ask the watcher to apply now; wait for the snapshot to match

## 3. Apply

- [ ] 3.1 Diff desired (file) against live (snapshot) by label: open, close, move
- [ ] 3.7 Map the n-ary tree to herdr's binary splits and ratios; reshape with `pane split`, `pane move`, `layout.set_split_ratio`
- [ ] 3.2 Open a service pane: split in place, set label, run `cmd` in `cwd` with `env`
- [ ] 3.3 Busy-agent rule: mark for removal, close on next `idle`/`done`, cancel if re-added
- [ ] 3.4 Defer moves and reorders in the tab the operator is viewing
- [ ] 3.5 Invalid file changes nothing and reports the error with file and line
- [ ] 3.6 Debounce file edits; full snapshot diff after each apply

## 4. Adopt

- [ ] 4.1 `herdfile adopt [<workspace>]` writes a file from the snapshot, generating labels for unlabelled panes
- [ ] 4.2 Refuse to overwrite without `--force`; first apply after adopt changes nothing

## 5. Write-back

- [ ] 5.1 Record the watcher's own operations and ignore matching events
- [ ] 5.2 Hand close removes the pane from the file
- [ ] 5.3 Hand-opened pane is added as `unmanaged` in its tab and position
- [ ] 5.4 Collision rule: hand change wins, dropped edit is reported

## 6. Verify

- [ ] 6.1 Integration tests against a throwaway herdr server, one per spec scenario
- [ ] 6.2 Dogfood: adopt every open workspace, confirm nothing closes, then edit one file by hand
