//! The "needs you" list: one JSON object per line in
//! `$XDG_STATE_HOME/herdfile/needs.jsonl`. Other tools may append lines.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

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

/// The list at one path. The module functions use the default path.
pub struct NeedsList {
    pub path: PathBuf,
}

impl NeedsList {
    pub fn at(path: &Path) -> NeedsList {
        NeedsList {
            path: path.to_path_buf(),
        }
    }

    pub fn default_path() -> NeedsList {
        NeedsList::at(&paths::needs_file())
    }

    pub fn read(&self) -> Vec<Need> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn write(&self, needs: &[Need]) -> Result<()> {
        let mut text = String::new();
        for n in needs {
            text.push_str(&serde_json::to_string(n)?);
            text.push('\n');
        }
        paths::write_atomic(&self.path, &text)?;
        Ok(())
    }

    /// Change the list under its lock.
    pub fn update<T>(&self, f: impl FnOnce(&mut Vec<Need>) -> T) -> Result<T> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let _lock = FileLock::acquire(&paths::lock_for(&self.path))?;
        let mut needs = self.read();
        let before = needs.clone();
        let out = f(&mut needs);
        if needs != before {
            self.write(&needs)?;
        }
        Ok(out)
    }

    /// Add an entry, or return the id of the same one already open.
    pub fn add(
        &self,
        workspace: &str,
        reason: &str,
        source: &str,
        pane: Option<&str>,
    ) -> Result<String> {
        self.update(|needs| {
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

    pub fn clear(&self, pred: impl Fn(&Need) -> bool) -> Result<usize> {
        self.update(|needs| {
            let before = needs.len();
            needs.retain(|n| !pred(n));
            before - needs.len()
        })
    }

    pub fn done(&self, id: &str) -> Result<()> {
        if self.clear(|n| n.id == id)? == 0 {
            bail!("no entry `{id}` in {}", self.path.display());
        }
        Ok(())
    }

    /// Keep `blocked` entries in step with herdr: add one per blocked agent,
    /// clear it when the agent leaves blocked.
    pub fn track(&self, snap: &Snapshot) -> Result<()> {
        let blocked: Vec<_> = snap
            .panes
            .iter()
            .filter(|p| p.agent_status.as_deref() == Some("blocked"))
            .collect();
        let blocked_ids: HashSet<&str> = blocked.iter().map(|p| p.pane_id.as_str()).collect();
        let is_blocked = |n: &Need| {
            n.pane
                .as_deref()
                .map(|p| blocked_ids.contains(p))
                .unwrap_or(false)
        };
        let current = self.read();
        let stale = current
            .iter()
            .any(|n| n.source == BLOCKED && !is_blocked(n));
        let missing: Vec<_> = blocked
            .iter()
            .filter(|p| {
                !current
                    .iter()
                    .any(|n| n.source == BLOCKED && n.pane.as_deref() == Some(&p.pane_id))
            })
            .collect();
        if !stale && missing.is_empty() {
            return Ok(());
        }
        self.update(|needs| {
            needs.retain(|n| n.source != BLOCKED || is_blocked(n));
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
        })
    }

    /// Open entries, oldest first.
    pub fn listing(&self) -> Vec<Need> {
        let mut needs = self.read();
        needs.sort_by(|a, b| a.time.cmp(&b.time));
        needs
    }
}

pub fn add(workspace: &str, reason: &str, source: &str, pane: Option<&str>) -> Result<String> {
    NeedsList::default_path().add(workspace, reason, source, pane)
}

pub fn clear(pred: impl Fn(&Need) -> bool) -> Result<usize> {
    NeedsList::default_path().clear(pred)
}

pub fn done(id: &str) -> Result<()> {
    NeedsList::default_path().done(id)
}

pub fn track(snap: &Snapshot) {
    if let Err(e) = NeedsList::default_path().track(snap) {
        crate::watch::log(&format!("needs: {e:#}"));
    }
}

pub fn print() {
    let needs = NeedsList::default_path().listing();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> (tempfile::TempDir, NeedsList) {
        let dir = tempfile::tempdir().unwrap();
        let list = NeedsList::at(&dir.path().join("needs.jsonl"));
        (dir, list)
    }

    #[test]
    fn add_dedupes_and_done_clears() {
        let (_d, l) = list();
        let a = l.add("w", "ship it?", ASK, None).unwrap();
        assert_eq!(l.add("w", "ship it?", ASK, None).unwrap(), a);
        let b = l.add("w", "other?", ASK, None).unwrap();
        assert_ne!(a, b);
        assert_eq!(l.read().len(), 2);
        l.done(&a).unwrap();
        assert_eq!(l.read().len(), 1);
        assert!(l.done(&a).unwrap_err().to_string().contains(&a));
    }

    #[test]
    fn other_tools_may_append_lines() {
        let (_d, l) = list();
        std::fs::write(
            &l.path,
            "{\"id\":\"pr-1\",\"workspace\":\"land\",\"reason\":\"review\",\"source\":\"lander\",\"time\":\"2026-01-01T00:00:00Z\"}\nnot json\n",
        )
        .unwrap();
        let needs = l.read();
        assert_eq!(needs.len(), 1);
        assert_eq!(needs[0].source, "lander");
        l.add("w", "q", ASK, None).unwrap();
        assert_eq!(l.listing()[0].id, "pr-1", "oldest first");
    }

    #[test]
    fn track_follows_blocked_agents() {
        let (_d, l) = list();
        let mut snap: Snapshot =
            serde_json::from_str(include_str!("../tests/fixtures/snapshot.json")).unwrap();
        l.track(&snap).unwrap();
        assert!(l.read().is_empty(), "nothing blocked yet");
        snap.panes[0].agent_status = Some("blocked".into());
        l.track(&snap).unwrap();
        let needs = l.read();
        assert_eq!(needs.len(), 1);
        assert_eq!(needs[0].workspace, "demo");
        assert_eq!(needs[0].pane.as_deref(), Some("w1:p1"));
        assert!(needs[0].reason.contains("`agent`"));
        l.track(&snap).unwrap();
        assert_eq!(l.read().len(), 1, "not added twice");
        snap.panes[0].agent_status = Some("idle".into());
        l.track(&snap).unwrap();
        assert!(l.read().is_empty(), "cleared once answered");
    }
}
