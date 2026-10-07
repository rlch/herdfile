//! `herdfile plan`: what a pass would do, without doing it. Write-back runs
//! against a backend that records instead of acting, and the result is
//! compared with the snapshot the way apply would.

use std::cell::RefCell;
use std::collections::HashSet;

use anyhow::{bail, Result};

use crate::apply::{load_services, workspace_dir, RATIO_TOLERANCE};
use crate::backend::herdr::Herdr;
use crate::backend::{Backend, Event, Placement, Snapshot, Spawn};
use crate::layout::{shape_of, Mark};
use crate::live::LiveNode;
use crate::paths;
use crate::writeback::{writeback, WsState};
use crate::wsfile::WorkspaceFile;

/// Reads go to herdr; every change is only written down.
struct Recorder<'a> {
    inner: &'a dyn Backend,
    log: RefCell<Vec<String>>,
}

impl Recorder<'_> {
    fn note(&self, what: String) -> Result<String> {
        self.log.borrow_mut().push(what);
        Ok("planned".into())
    }
}

impl Backend for Recorder<'_> {
    fn snapshot(&self) -> Result<Snapshot> {
        self.inner.snapshot()
    }
    fn subscribe(&self, _: &mut dyn FnMut(Event) -> bool) -> Result<()> {
        bail!("plan does not subscribe")
    }
    fn create_workspace(&self, label: &str, _: &str) -> Result<(String, String, String)> {
        self.note(format!("create workspace {label}"))?;
        bail!("plan does not create workspaces")
    }
    fn close_workspace(&self, ws: &str) -> Result<()> {
        self.note(format!("close workspace {ws}")).map(|_| ())
    }
    fn create_tab(&self, _: &str, label: &str, _: &Spawn) -> Result<(String, String)> {
        self.note(format!("create tab {label}"))?;
        bail!("plan does not create tabs")
    }
    fn rename_tab(&self, tab: &str, label: &str) -> Result<()> {
        self.note(format!("rename tab {tab} to {label}"))
            .map(|_| ())
    }
    fn move_tab(&self, tab: &str, index: usize) -> Result<()> {
        self.note(format!("move tab {tab} to {index}")).map(|_| ())
    }
    fn split(&self, at: &Placement, _: &Spawn) -> Result<String> {
        self.note(format!("split {}", at.target_pane))
    }
    fn move_pane(&self, pane: &str, tab: &str, _: &Placement) -> Result<String> {
        self.note(format!("move {pane} to {tab}"))
    }
    fn move_pane_new_tab(&self, pane: &str, _: &str, label: &str) -> Result<(String, String)> {
        self.note(format!("move {pane} to new tab {label}"))
            .map(|s| (s.clone(), s))
    }
    fn swap(&self, a: &str, b: &str) -> Result<()> {
        self.note(format!("swap {a} {b}")).map(|_| ())
    }
    fn close_pane(&self, pane: &str) -> Result<()> {
        self.note(format!("close {pane}")).map(|_| ())
    }
    fn rename_pane(&self, pane: &str, label: Option<&str>) -> Result<()> {
        self.note(format!("label {pane} `{}`", label.unwrap_or("")))
            .map(|_| ())
    }
    fn run(&self, pane: &str, command: &str) -> Result<()> {
        self.note(format!("run `{command}` in {pane}")).map(|_| ())
    }
    fn set_ratio(&self, tab: &str, path: &[bool], ratio: f64) -> Result<()> {
        self.note(format!("set ratio {tab} {path:?} {ratio:.2}"))
            .map(|_| ())
    }
    fn cli(&self, args: &[&str]) -> Result<(i32, String, String)> {
        bail!("plan does not run herdr {}", args.join(" "))
    }
}

/// What a pass would change in one workspace, as lines. Empty: nothing.
pub fn plan_workspace(
    backend: &dyn Backend,
    snap: &Snapshot,
    ws: &str,
    path: &std::path::Path,
    mut state: WsState,
) -> Result<Vec<String>> {
    let rec = Recorder {
        inner: backend,
        log: RefCell::new(Vec::new()),
    };
    let mut file = if path.exists() {
        WorkspaceFile::load(path)?
    } else {
        let mut f = WorkspaceFile::empty(path);
        if let Some(dir) = workspace_dir(&f, snap, ws) {
            f.set_dir(&dir.to_string_lossy());
        }
        f
    };
    let services = load_services(&file, snap, ws)?;
    let mut local = snap.clone();
    let wb = writeback(&rec, &mut local, ws, &mut file, &mut state, &services)?;
    file.validate(&services)?;

    let mut out: Vec<String> = rec.log.into_inner();
    if !wb.recorded.is_empty() {
        out.push(format!("record: {}", wb.recorded.join(", ")));
    }
    if !wb.dropped.is_empty() {
        out.push(format!("drop (closed by hand): {}", wb.dropped.join(", ")));
    }

    let listed: HashSet<String> = file.leaf_names().into_iter().collect();
    for pane in local.panes_of(ws) {
        let Some(label) = &pane.label else { continue };
        if !listed.contains(label) && !state.protected.contains(label) {
            out.push(format!(
                "close {label}{}",
                if pane.busy() { " once idle" } else { "" }
            ));
        }
    }
    for tab in &file.tabs {
        let placeable = |l: &crate::layout::Leaf| {
            local.pane_by_label(ws, &l.name).is_some()
                || (l.mark == Mark::Managed && services.contains(&l.name))
        };
        let Some(desired) = shape_of(&tab.tree, &placeable) else {
            continue;
        };
        let Some(live_tab) = local.tab_by_label(ws, &tab.name) else {
            out.push(format!("create tab {}", tab.name));
            continue;
        };
        let mut names = Vec::new();
        desired.leaves(&mut names);
        let set: HashSet<String> = names.iter().cloned().collect();
        for n in &names {
            if local.pane_by_label(ws, n).is_none() {
                out.push(format!("open {n} in tab {}", tab.name));
            }
        }
        let live = LiveNode::of_tab(&local, &live_tab.tab_id)
            .and_then(|l| l.prune(&|n| n.map(|n| set.contains(n)).unwrap_or(false)));
        match live {
            Some(l) if l.shape().same_layout(&desired) => {
                for (path, now, want) in l.wanted_ratios(&desired) {
                    if (now - want).abs() >= RATIO_TOLERANCE {
                        out.push(format!(
                            "resize tab {} split {path:?}: {:.0}% -> {:.0}%",
                            tab.name,
                            now * 100.0,
                            want * 100.0
                        ));
                    }
                }
            }
            _ => out.push(format!("reshape tab {} (panes would move)", tab.name)),
        }
    }
    Ok(out)
}

/// `herdfile plan`: every workspace in scope (or one), read-only.
pub fn plan(only: Option<String>) -> Result<()> {
    let backend = Herdr::from_env();
    let snap = backend.snapshot()?;
    let scope = crate::workspaces::watch_scope();
    let mut quiet = 0;
    let mut risky = 0;
    for w in &snap.workspaces {
        let label = w.label.clone().unwrap_or_default();
        match &only {
            Some(id) if id != &w.workspace_id && id != &label => continue,
            None if !crate::workspaces::in_scope(&scope, &label) => continue,
            _ => {}
        }
        let path = paths::workspace_file(&w.workspace_id);
        let state = WsState::load(&w.workspace_id);
        match plan_workspace(&backend, &snap, &w.workspace_id, &path, state) {
            Ok(lines) => {
                let moves = lines.iter().any(|l| {
                    l.starts_with("close ")
                        || l.starts_with("reshape ")
                        || l.starts_with("resize ")
                        || l.starts_with("move ")
                        || l.starts_with("open ")
                });
                if moves {
                    risky += 1;
                } else {
                    quiet += 1;
                }
                if lines.is_empty() {
                    println!("{} {label}: nothing", w.workspace_id);
                } else {
                    println!("{} {label}:", w.workspace_id);
                    for l in lines {
                        println!("  {l}");
                    }
                }
            }
            Err(e) => {
                risky += 1;
                println!("{} {label}: error: {e:#}", w.workspace_id);
            }
        }
    }
    println!(
        "\n{quiet} workspace(s) would only be labelled and recorded; {risky} would see panes opened, closed, moved, or resized"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Snapshot {
        serde_json::from_str(include_str!("../tests/fixtures/snapshot.json")).unwrap()
    }

    #[test]
    fn first_sight_plan_only_labels_and_records() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w1.toml");
        let lines = plan_workspace(
            &crate::backend::fake::Fake::default(),
            &fixture(),
            "w1",
            &path,
            WsState::default(),
        )
        .unwrap();
        assert!(
            lines.iter().any(|l| l == "label w1:p3 `shell-1`"),
            "{lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.starts_with("rename tab")),
            "titled tabs are left alone: {lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l == "record: agent, test, shell-1, dev"),
            "{lines:?}"
        );
        assert!(
            !lines.iter().any(|l| l.starts_with("close")
                || l.starts_with("reshape")
                || l.starts_with("resize")),
            "{lines:?}"
        );
        assert!(!path.exists(), "plan wrote a file");
    }

    #[test]
    fn plan_shows_what_apply_would_close() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w1.toml");
        std::fs::write(
            &path,
            "[tab.main]\nrow = [\"agent\", { pane = \"test\", mark = \"unmanaged\", cwd = \"/tmp\" }]\n",
        )
        .unwrap();
        // `dev` was seen in the file before; the file has since dropped it.
        let mut state = WsState::default();
        state.known.insert("dev".into());
        let lines = plan_workspace(
            &crate::backend::fake::Fake::default(),
            &fixture(),
            "w1",
            &path,
            state,
        )
        .unwrap();
        assert!(lines.iter().any(|l| l == "close dev"), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("close test")), "{lines:?}");
    }
}
