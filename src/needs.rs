//! The "needs you" list: one JSON object per line in
//! `$XDG_STATE_HOME/herdfile/needs.jsonl`. Other tools may append lines.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::lock::FileLock;
use crate::paths;

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
