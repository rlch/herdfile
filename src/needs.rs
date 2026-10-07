//! The "needs you" list: one JSON object per line in
//! `$XDG_STATE_HOME/herdfile/needs.jsonl`. Other tools may append lines.

use std::collections::HashSet;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::backend::Snapshot;
use crate::lock::FileLock;
use crate::paths;

pub const BLOCKED: &str = "blocked";
pub const HELD_REMOVAL: &str = "held-removal";
pub const ASK: &str = "ask";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Need {
    pub id: String,
    /// The workspace's name (its herdr label).
    pub workspace: String,
    pub reason: String,
    /// `blocked`, `held-removal`, `ask`, or another tool's name.
    pub source: String,
    /// RFC 3339, UTC.
    pub time: String,
    /// The pane a `blocked` entry is about; cleared when it leaves blocked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
}

pub use crate::time::{now_secs, rfc3339};

fn new_id(existing: &[Need]) -> String {
    let base = now_secs();
    (0..)
        .map(|n| format!("n{:x}", base * 16 + n))
        .find(|id| existing.iter().all(|e| &e.id != id))
        .expect("unbounded")
}

pub fn read() -> Vec<Need> {
    let Ok(text) = std::fs::read_to_string(paths::needs_file()) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn write(needs: &[Need]) -> Result<()> {
    let mut text = String::new();
    for n in needs {
        text.push_str(&serde_json::to_string(n)?);
        text.push('\n');
    }
    paths::write_atomic(&paths::needs_file(), &text)?;
    Ok(())
}

/// Change the list under its lock.
pub fn update<T>(f: impl FnOnce(&mut Vec<Need>) -> T) -> Result<T> {
    paths::ensure_state_dir()?;
    let _lock = FileLock::acquire(&paths::lock_for(&paths::needs_file()))?;
    let mut needs = read();
    let before = needs.clone();
    let out = f(&mut needs);
    if needs != before {
        write(&needs)?;
    }
    Ok(out)
}

pub fn add(workspace: &str, reason: &str, source: &str, pane: Option<&str>) -> Result<String> {
    update(|needs| {
        if let Some(same) = needs
            .iter()
            .find(|n| n.workspace == workspace && n.source == source && n.reason == reason)
        {
            return same.id.clone();
        }
        let id = new_id(needs);
        needs.push(Need {
            id: id.clone(),
            workspace: workspace.to_string(),
            reason: reason.to_string(),
            source: source.to_string(),
            time: rfc3339(now_secs()),
            pane: pane.map(str::to_string),
        });
        id
    })
}

pub fn clear(pred: impl Fn(&Need) -> bool) -> Result<usize> {
    update(|needs| {
        let before = needs.len();
        needs.retain(|n| !pred(n));
        before - needs.len()
    })
}

pub fn done(id: &str) -> Result<()> {
    if clear(|n| n.id == id)? == 0 {
        bail!("no entry `{id}` in {}", paths::needs_file().display());
    }
    Ok(())
}

/// Keep `blocked` entries in step with herdr: add one per blocked agent,
/// clear it when the agent leaves blocked.
pub fn track(snap: &Snapshot) {
    let blocked: Vec<_> = snap
        .panes
        .iter()
        .filter(|p| p.agent_status.as_deref() == Some("blocked"))
        .collect();
    let blocked_ids: HashSet<&str> = blocked.iter().map(|p| p.pane_id.as_str()).collect();
    let current = read();
    let stale = current.iter().any(|n| {
        n.source == BLOCKED
            && !n
                .pane
                .as_deref()
                .map(|p| blocked_ids.contains(p))
                .unwrap_or(false)
    });
    let missing: Vec<_> = blocked
        .iter()
        .filter(|p| {
            !current
                .iter()
                .any(|n| n.source == BLOCKED && n.pane.as_deref() == Some(&p.pane_id))
        })
        .collect();
    if !stale && missing.is_empty() {
        return;
    }
    let result = update(|needs| {
        needs.retain(|n| {
            n.source != BLOCKED
                || n.pane
                    .as_deref()
                    .map(|p| blocked_ids.contains(p))
                    .unwrap_or(false)
        });
        for p in &missing {
            let ws = snap
                .workspace(&p.workspace_id)
                .and_then(|w| w.label.clone())
                .unwrap_or_else(|| p.workspace_id.clone());
            let who = p.label.clone().unwrap_or_else(|| p.pane_id.clone());
            let id = new_id(needs);
            needs.push(Need {
                id,
                workspace: ws,
                reason: format!("`{who}` is waiting for an answer"),
                source: BLOCKED.to_string(),
                time: rfc3339(now_secs()),
                pane: Some(p.pane_id.clone()),
            });
        }
    });
    if let Err(e) = result {
        crate::watch::log(&format!("needs: {e:#}"));
    }
}

pub fn print() {
    let mut needs = read();
    needs.sort_by(|a, b| a.time.cmp(&b.time));
    if needs.is_empty() {
        println!("nothing needs you");
        return;
    }
    for n in needs {
        println!("{}  {}  {}  {}", n.id, n.time, n.workspace, n.reason);
    }
}

/// `herdfile ask`: put a question on the "needs you" list.
pub fn ask(question: &str) -> Result<()> {
    use crate::backend::Backend;
    let backend = crate::backend::herdr::Herdr::from_env();
    let who = backend
        .snapshot()
        .ok()
        .and_then(|s| crate::workspaces::caller_name(&s))
        .unwrap_or_else(|| crate::workspaces::OPERATOR.to_string());
    let id = add(&who, question, ASK, None)?;
    println!("asked ({id}); it is on `herdfile needs`");
    Ok(())
}
