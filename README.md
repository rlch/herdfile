# herdfile

Declarative workspaces for [herdr](https://herdr.dev).

A repo says what services it has. A per-workspace file says what is on screen:
which tabs, which panes in each. A watcher makes herdr match the file, and writes
hand changes back into it. Built so AI agents can read and change the layout of
your terminal workspaces without guessing.

Status: the pane layer (services file, workspace file, apply, adopt, write-back)
works against herdr 0.9.x. See `openspec/changes/declarative-workspaces/`.
