//! Integration tests: one crate, so it builds once and the harness can cap
//! how many throwaway herdr servers run at a time.
//!
//! Every test runs against its own herdr server in a named session under a
//! fresh `XDG_CONFIG_HOME` in /tmp, never the operator's. See `harness`.

mod apply;
mod commands;
mod first_sight;
mod handoff;
mod harness;
mod plan;
mod watcher;
mod workspaces;
mod writeback;
