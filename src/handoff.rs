//! `herdfile handoff`: an agent replaces itself with a fresh one in the same
//! place. The successor starts in a pane split from the caller's, takes the
//! workspace's agent name and the caller's label once it is ready and has its
//! brief, and the caller's pane is closed last, which ends the caller.

use anyhow::{anyhow, bail, Result};

use crate::backend::herdr::Herdr;
use crate::backend::{Backend, Placement, Spawn};
use crate::layout::Dir;
use crate::lock::FileLock;
use crate::paths;
use crate::workspaces::{prompt, start_agent, valid_name};

pub struct HandoffArgs {
    /// The pane being replaced (default: the calling pane).
    pub pane: Option<String>,
    pub brief: String,
    pub model: Option<String>,
}

pub fn handoff(args: HandoffArgs) -> Result<()> {
    let backend = Herdr::from_env();
    let pane = args
        .pane
        .clone()
        .or_else(|| {
            std::env::var("HERDR_PANE_ID")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .ok_or_else(|| anyhow!("no pane: run inside the agent's herdr pane or pass --pane"))?;
    let (successor, lock) = handoff_with(&backend, &pane, &args.brief, args.model.as_deref())?;
    println!("handed off to {successor}; closing this pane");
    // Last, still holding the workspace: this usually ends the process
    // running it, and the lock goes with it.
    backend.close_pane(&pane)?;
    drop(lock);
    Ok(())
}

/// Everything but closing the old pane. Returns the successor's pane id and
/// the workspace lock, to be held until the old pane is closed.
pub fn handoff_with(
    backend: &dyn Backend,
    old: &str,
    brief: &str,
    model: Option<&str>,
) -> Result<(String, FileLock)> {
    let snap = backend.snapshot()?;
    let old_pane = snap
        .pane(old)
        .ok_or_else(|| anyhow!("pane {old} is not open"))?
        .clone();
    if old_pane.agent.is_none() {
        bail!("pane {old} is not running an agent; there is nothing to hand off");
    }
    let ws = old_pane.workspace_id.clone();
    let ws_label = snap
        .workspace(&ws)
        .and_then(|w| w.label.clone())
        .unwrap_or_default();
    let label = old_pane
        .label
        .clone()
        .unwrap_or_else(|| crate::services::RESERVED.to_string());

    // Hold the workspace so the watcher never sees the swap half done.
    paths::ensure_state_dir()?;
    let lock = FileLock::acquire(&paths::lock_for(&paths::workspace_file(&ws)))?;

    // The successor takes over the workspace's agent name: the old agent's,
    // else the workspace's own name when it is a valid one.
    let old_name = backend
        .cli(&["agent", "get", old])
        .ok()
        .filter(|(code, _, _)| *code == 0)
        .and_then(|(_, out, _)| serde_json::from_str::<serde_json::Value>(&out).ok())
        .and_then(|v| {
            v.pointer("/result/agent/name")
                .and_then(|n| n.as_str())
                .map(str::to_string)
        });
    let name = old_name
        .clone()
        .or_else(|| valid_name(&ws_label).then(|| ws_label.clone()))
        .unwrap_or_else(|| format!("handoff-{}", old.replace(':', "-").to_lowercase()));
    if !valid_name(&name) {
        bail!("cannot name the successor `{name}`");
    }

    let cwd = old_pane.cwd.clone();
    let new = backend.split(
        &Placement {
            target_pane: old,
            dir: Dir::Row,
            ratio: None,
        },
        &Spawn {
            cwd: cwd.as_deref(),
            env: Vec::new(),
        },
    )?;
    let started = (|| -> Result<()> {
        backend.rename_pane(&new, Some(&format!("{label}-next")))?;
        if old_name.is_some() {
            // Free the name for the successor.
            let _ = backend.cli(&["agent", "rename", old, "--clear"]);
        }
        start_agent(backend, &name, &new, model)?;
        prompt(backend, &new, &new, &format!("Read {brief} and follow it."))?;
        Ok(())
    })();
    if let Err(e) = started {
        // Leave things as they were: close the successor, give the name back.
        let _ = backend.close_pane(&new);
        if let Some(n) = &old_name {
            let _ = backend.cli(&["agent", "rename", old, n]);
        }
        return Err(e.context("the successor did not start; nothing was handed off"));
    }
    backend.rename_pane(old, Some(&format!("{label}-old")))?;
    backend.rename_pane(&new, Some(&label))?;
    Ok((new, lock))
}
