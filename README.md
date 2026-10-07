# herdfile

Declarative workspaces for [herdr](https://herdr.dev).

A repo says what services it has. A per-workspace file says what is on screen:
which tabs, which panes in each. A watcher makes herdr match the file, and writes
hand changes back into it. Built so AI agents can read and change the layout of
your terminal workspaces without guessing.

Status: design. See `openspec/changes/`.
