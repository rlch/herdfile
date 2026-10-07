//! Make herdr match a workspace file: close what the file left out, open what
//! it adds, and reshape each tab into its rows and columns.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};

use crate::backend::{Backend, Placement, Snapshot, Spawn};
use crate::layout::{shape_of, Leaf, Mark, Shape};
use crate::live::LiveNode;
use crate::services::{Service, Services};
use crate::writeback::{writeback, WritebackReport, WsState};
use crate::wsfile::{Tab, WorkspaceFile};

pub const STAGING_TAB: &str = "herdfile-staging";
/// Ratios closer than this are left alone, so a drag that rounds to the
/// file's size is not undone.
const RATIO_TOLERANCE: f64 = 0.025;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Report {
    pub opened: Vec<String>,
    /// Newly opened service panes: (label, pane id, ready text).
    #[serde(default)]
    pub opened_ready: Vec<(String, String, String)>,
    pub closed: Vec<String>,
    pub moved: Vec<String>,
    pub resized: Vec<String>,
    /// Busy panes left out of the file, closed once idle.
    pub pending: Vec<String>,
    /// Reshapes waiting for the operator to leave the tab.
    pub deferred: Vec<String>,
    pub notes: Vec<String>,
    pub writeback: WritebackReport,
}

impl Report {
    pub fn ops(&self) -> usize {
        self.opened.len() + self.closed.len() + self.moved.len() + self.resized.len()
    }

    pub fn summary(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let mut push = |what: &str, items: &[String]| {
            if !items.is_empty() {
                lines.push(format!("{what}: {}", items.join(", ")));
            }
        };
        push("opened", &self.opened);
        push("closed", &self.closed);
        push("moved", &self.moved);
        push("resized", &self.resized);
        push(
            "closed by hand, dropped from the file",
            &self.writeback.dropped,
        );
        push("recorded as unmanaged", &self.writeback.recorded);
        push("sizes written back", &self.writeback.resized);
        push("marked for removal once idle", &self.pending);
        push("deferred until you leave the tab", &self.deferred);
        lines.extend(self.notes.iter().cloned());
        lines
    }
}

/// Everything apply needs to know about one workspace.
pub struct Target<'a> {
    pub backend: &'a dyn Backend,
    pub ws: &'a str,
    pub file_path: PathBuf,
}

/// The folder a workspace's services are found from: the file's `dir`, else
/// the cwd of its agent pane, else of its first pane.
pub fn workspace_dir(file: &WorkspaceFile, snap: &Snapshot, ws: &str) -> Option<PathBuf> {
    if let Some(dir) = &file.dir {
        return Some(crate::paths::expand_tilde(dir));
    }
    let agent = snap.pane_by_label(ws, "agent").and_then(|p| p.cwd.clone());
    agent
        .or_else(|| snap.panes_of(ws).find_map(|p| p.cwd.clone()))
        .map(PathBuf::from)
}

pub fn load_services(file: &WorkspaceFile, snap: &Snapshot, ws: &str) -> Result<Services> {
    match workspace_dir(file, snap, ws) {
        Some(dir) => Services::discover(&dir),
        None => Ok(Services::default()),
    }
}

/// Load and fully validate a workspace file against its services.
pub fn load_checked(path: &Path, snap: &Snapshot, ws: &str) -> Result<(WorkspaceFile, Services)> {
    let file = WorkspaceFile::load(path)?;
    let services = load_services(&file, snap, ws)?;
    file.validate(&services)?;
    Ok((file, services))
}

/// One pass: write back hand changes, then converge herdr to the file.
/// The caller holds the workspace lock.
pub fn apply(target: &Target, state: &mut WsState) -> Result<Report> {
    let backend = target.backend;
    let ws = target.ws;
    let mut snap = backend.snapshot()?;
    if snap.workspace(ws).is_none() {
        bail!("workspace {ws} is not open in herdr");
    }
    // An invalid file changes nothing, not even write-back.
    let (mut file, _) = load_checked(&target.file_path, &snap, ws)?;

    let mut report = Report::default();
    let before = file.to_text();
    report.writeback = writeback(backend, &mut snap, ws, &mut file, state)?;
    if file.to_text() != before {
        file.save()?;
    }
    let services = load_services(&file, &snap, ws)?;
    file.validate(&services)?;

    let mut ctx = Ctx {
        backend,
        ws,
        services: &services,
        report: &mut report,
        state,
    };
    ctx.close_unlisted(&file)?;
    for tab in &file.tabs {
        let Some(tab_id) = ctx.ensure_tab(tab)? else {
            continue;
        };
        ctx.reshape(tab, &tab_id)?;
    }
    ctx.order_tabs(&file)?;
    ctx.record(&file)?;
    Ok(report)
}

struct Ctx<'a> {
    backend: &'a dyn Backend,
    ws: &'a str,
    services: &'a Services,
    report: &'a mut Report,
    state: &'a mut WsState,
}

impl Ctx<'_> {
    fn snap(&self) -> Result<Snapshot> {
        self.backend.snapshot()
    }

    fn service(&self, leaf: &Leaf) -> Option<&Service> {
        if leaf.mark != Mark::Managed {
            return None;
        }
        self.services.get(&leaf.name)
    }

    /// Close labelled panes the file no longer lists. Busy agents wait.
    fn close_unlisted(&mut self, file: &WorkspaceFile) -> Result<()> {
        let listed: HashSet<String> = file.leaf_names().into_iter().collect();
        let snap = self.snap()?;
        self.state.pending.retain(|l| !listed.contains(l));
        for pane in snap.panes_of(self.ws) {
            let Some(label) = &pane.label else { continue };
            if listed.contains(label) || self.state.protected.contains(label) {
                continue;
            }
            if pane.busy() {
                self.state.pending.insert(label.clone());
                self.report.pending.push(label.clone());
                continue;
            }
            self.backend.close_pane(&pane.pane_id)?;
            self.state.pending.remove(label);
            self.state.known.remove(label);
            self.report.closed.push(label.clone());
        }
        Ok(())
    }

    /// Can this leaf be put on screen: it is live, or it is a service.
    fn placeable(&self, snap: &Snapshot, leaf: &Leaf) -> bool {
        snap.pane_by_label(self.ws, &leaf.name).is_some() || self.service(leaf).is_some()
    }

    fn open_service(&mut self, svc: &Service, pane: &str) -> Result<()> {
        self.backend.rename_pane(pane, Some(&svc.name))?;
        self.backend.run(pane, &svc.cmd)?;
        self.state.known.insert(svc.name.clone());
        self.report.opened.push(svc.name.clone());
        if let Some(ready) = &svc.ready {
            self.report
                .opened_ready
                .push((svc.name.clone(), pane.to_string(), ready.clone()));
        }
        Ok(())
    }

    fn spawn_for(svc: &Service) -> (String, Vec<(String, String)>) {
        (
            svc.cwd.to_string_lossy().into_owned(),
            svc.env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        )
    }

    /// Find or create the tab. A new tab starts from its first placeable pane.
    fn ensure_tab(&mut self, tab: &Tab) -> Result<Option<String>> {
        let snap = self.snap()?;
        if let Some(t) = snap.tab_by_label(self.ws, &tab.name) {
            return Ok(Some(t.tab_id.clone()));
        }
        let leaves = tab.tree.leaves();
        let Some(first) = leaves.iter().find(|l| self.placeable(&snap, l)) else {
            self.report.notes.push(format!(
                "tab `{}`: nothing to open ({} not running)",
                tab.name,
                tab.tree.leaf_names().join(", ")
            ));
            return Ok(None);
        };
        if let Some(pane) = snap.pane_by_label(self.ws, &first.name) {
            let (_, tab_id) = self
                .backend
                .move_pane_new_tab(&pane.pane_id, self.ws, &tab.name)?;
            self.report.moved.push(first.name.clone());
            return Ok(Some(tab_id));
        }
        let svc = self.service(first).expect("placeable").clone();
        let (cwd, env) = Self::spawn_for(&svc);
        let (tab_id, root) = self.backend.create_tab(
            self.ws,
            &tab.name,
            &Spawn {
                cwd: Some(&cwd),
                env,
            },
        )?;
        self.open_service(&svc, &root)?;
        Ok(Some(tab_id))
    }

    fn desired_shape(&self, snap: &Snapshot, tab: &Tab) -> Option<Shape> {
        shape_of(&tab.tree, &|l| self.placeable(snap, l))
    }

    fn live_pruned(snap: &Snapshot, tab_id: &str, names: &HashSet<String>) -> Option<LiveNode> {
        LiveNode::of_tab(snap, tab_id)?.prune(&|l| l.map(|l| names.contains(l)).unwrap_or(false))
    }

    fn reshape(&mut self, tab: &Tab, tab_id: &str) -> Result<()> {
        let snap = self.snap()?;
        let Some(desired) = self.desired_shape(&snap, tab) else {
            return Ok(());
        };
        let mut names = Vec::new();
        desired.leaves(&mut names);
        let name_set: HashSet<String> = names.iter().cloned().collect();
        let live = Self::live_pruned(&snap, tab_id, &name_set);
        let viewed = snap.viewed_tab() == Some(tab_id);

        let matches = live
            .as_ref()
            .map(|l| l.shape().same_layout(&desired))
            .unwrap_or(false);
        if !matches {
            let live_names: HashSet<String> = live
                .as_ref()
                .map(|l| l.leaves().into_iter().filter_map(|(_, n)| n).collect())
                .unwrap_or_default();
            let same_shape = live
                .as_ref()
                .map(|l| l.shape().same_shape(&desired))
                .unwrap_or(false);
            if viewed {
                self.open_missing_in_place(tab, tab_id)?;
                self.report.deferred.push(tab.name.clone());
            } else if same_shape && live_names == name_set {
                self.swap_into_order(tab_id, &names)?;
            } else {
                self.rebuild(tab_id, &desired)?;
            }
        }
        self.fix_ratios(tab_id, &desired, &name_set)
    }

    /// The operator is looking at this tab: open missing services next to
    /// their file neighbour, but move nothing.
    fn open_missing_in_place(&mut self, tab: &Tab, tab_id: &str) -> Result<()> {
        let leaves: Vec<Leaf> = tab.tree.leaves().into_iter().cloned().collect();
        for (i, leaf) in leaves.iter().enumerate() {
            let snap = self.snap()?;
            if snap.pane_by_label(self.ws, &leaf.name).is_some() {
                continue;
            }
            let Some(svc) = self.service(leaf).cloned() else {
                continue;
            };
            let in_tab = |name: &str| {
                snap.pane_by_label(self.ws, name)
                    .filter(|p| p.tab_id == tab_id)
                    .map(|p| p.pane_id.clone())
            };
            let target = leaves[..i]
                .iter()
                .rev()
                .find_map(|l| in_tab(&l.name))
                .or_else(|| {
                    snap.layout(tab_id)
                        .and_then(|l| l.panes.last())
                        .map(|p| p.pane_id.clone())
                });
            let Some(target) = target else { continue };
            let dir = container_dir_of(&tab.tree, &leaf.name).unwrap_or(tab.tree.dir);
            let (cwd, env) = Self::spawn_for(&svc);
            let pane = self.backend.split(
                &Placement {
                    target_pane: &target,
                    dir,
                    ratio: None,
                },
                &Spawn {
                    cwd: Some(&cwd),
                    env,
                },
            )?;
            self.open_service(&svc, &pane)?;
        }
        Ok(())
    }

    /// Same shape, wrong order: swap panes into place.
    fn swap_into_order(&mut self, tab_id: &str, names: &[String]) -> Result<()> {
        let name_set: HashSet<String> = names.iter().cloned().collect();
        for (i, want) in names.iter().enumerate() {
            let snap = self.snap()?;
            let Some(live) = Self::live_pruned(&snap, tab_id, &name_set) else {
                break;
            };
            let leaves = live.leaves();
            let Some((here_id, Some(here))) = leaves.get(i).cloned() else {
                break;
            };
            if &here == want {
                continue;
            }
            let Some(there) = snap.pane_by_label(self.ws, want) else {
                continue;
            };
            self.backend.swap(&here_id, &there.pane_id)?;
            self.report.moved.push(want.clone());
        }
        Ok(())
    }

    /// Rebuild a tab's layout around its first pane. Panes to be rearranged
    /// are parked in a staging tab, then moved back one by one, each next to
    /// a pane already in place. herdr cannot move a pane within its own tab.
    fn rebuild(&mut self, tab_id: &str, desired: &Shape) -> Result<()> {
        let mut names = Vec::new();
        desired.leaves(&mut names);
        let first = desired.first_leaf().to_string();

        // The first pane anchors the rebuild; bring it here if needed.
        let snap = self.snap()?;
        let in_tab = |s: &Snapshot, name: &str| {
            s.pane_by_label(self.ws, name)
                .map(|p| p.tab_id == tab_id)
                .unwrap_or(false)
        };
        if !in_tab(&snap, &first) {
            let any = snap
                .layout(tab_id)
                .and_then(|l| l.panes.first())
                .map(|p| p.pane_id.clone())
                .ok_or_else(|| anyhow!("tab {tab_id} has no panes"))?;
            self.place(&first, tab_id, &any, crate::layout::Dir::Row, None)?;
        }

        // Park every other listed pane of this tab.
        let snap = self.snap()?;
        let mut staging: Option<(String, String)> = None;
        let to_park: Vec<String> = snap
            .panes_of(self.ws)
            .filter(|p| p.tab_id == tab_id)
            .filter_map(|p| p.label.clone())
            .filter(|l| l != &first && names.contains(l))
            .collect();
        for label in to_park {
            let snap = self.snap()?;
            let pane = snap
                .pane_by_label(self.ws, &label)
                .ok_or_else(|| anyhow!("{label} vanished"))?
                .pane_id
                .clone();
            match &staging {
                None => {
                    let (pane, tab) =
                        self.backend
                            .move_pane_new_tab(&pane, self.ws, STAGING_TAB)?;
                    staging = Some((tab, pane));
                }
                Some((stage_tab, stage_pane)) => {
                    self.backend.move_pane(
                        &pane,
                        stage_tab,
                        &Placement {
                            target_pane: stage_pane,
                            dir: crate::layout::Dir::Row,
                            ratio: None,
                        },
                    )?;
                }
            }
        }

        self.build(tab_id, desired)?;
        Ok(())
    }

    /// Lay out `shape`, whose first leaf is already in place.
    fn build(&mut self, tab_id: &str, shape: &Shape) -> Result<()> {
        let Shape::Split(dir, children) = shape else {
            return Ok(());
        };
        let mut prev = children[0].0.first_leaf().to_string();
        for k in 1..children.len() {
            let rest: f64 = children[k - 1..].iter().map(|c| c.1).sum();
            let ratio = if rest > 0.0 {
                children[k - 1].1 / rest
            } else {
                0.5
            };
            let name = children[k].0.first_leaf().to_string();
            let snap = self.snap()?;
            let target = snap
                .pane_by_label(self.ws, &prev)
                .ok_or_else(|| anyhow!("{prev} vanished"))?
                .pane_id
                .clone();
            self.place(&name, tab_id, &target, *dir, Some(ratio))?;
            prev = name;
        }
        for (child, _) in children {
            self.build(tab_id, child)?;
        }
        Ok(())
    }

    /// Put `name` next to `target`: move it if live, open it if a service.
    fn place(
        &mut self,
        name: &str,
        tab_id: &str,
        target: &str,
        dir: crate::layout::Dir,
        ratio: Option<f64>,
    ) -> Result<()> {
        let snap = self.snap()?;
        let at = Placement {
            target_pane: target,
            dir,
            ratio,
        };
        if let Some(pane) = snap.pane_by_label(self.ws, name) {
            self.backend.move_pane(&pane.pane_id, tab_id, &at)?;
            self.report.moved.push(name.to_string());
            return Ok(());
        }
        let svc = self
            .services
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow!("cannot open `{name}`: not a service"))?;
        let (cwd, env) = Self::spawn_for(&svc);
        let pane = self.backend.split(
            &at,
            &Spawn {
                cwd: Some(&cwd),
                env,
            },
        )?;
        self.open_service(&svc, &pane)
    }

    fn fix_ratios(&mut self, tab_id: &str, desired: &Shape, names: &HashSet<String>) -> Result<()> {
        let snap = self.snap()?;
        let Some(live) = Self::live_pruned(&snap, tab_id, names) else {
            return Ok(());
        };
        if !live.shape().same_layout(desired) {
            return Ok(());
        }
        let mut changed = false;
        for (path, now, want) in live.wanted_ratios(desired) {
            if (now - want).abs() >= RATIO_TOLERANCE {
                self.backend.set_ratio(tab_id, &path, want)?;
                changed = true;
            }
        }
        if changed {
            let label = snap
                .tabs
                .iter()
                .find(|t| t.tab_id == tab_id)
                .and_then(|t| t.label.clone())
                .unwrap_or_else(|| tab_id.to_string());
            self.report.resized.push(label);
        }
        Ok(())
    }

    /// Tabs in file order. Tabs the file does not list keep their places after.
    fn order_tabs(&mut self, file: &WorkspaceFile) -> Result<()> {
        for (index, tab) in file.tabs.iter().enumerate() {
            let snap = self.snap()?;
            let mut tabs: Vec<_> = snap.tabs_of(self.ws).collect();
            tabs.sort_by_key(|t| t.number);
            let Some(pos) = tabs
                .iter()
                .position(|t| t.label.as_deref() == Some(&tab.name))
            else {
                continue;
            };
            let want = file.tabs[..index]
                .iter()
                .filter(|t| tabs.iter().any(|lt| lt.label.as_deref() == Some(&t.name)))
                .count();
            if pos != want {
                self.backend.move_tab(&tabs[pos].tab_id, want)?;
            }
        }
        Ok(())
    }

    /// Remember what is live and the ratios we left, for the next write-back.
    fn record(&mut self, file: &WorkspaceFile) -> Result<()> {
        let snap = self.snap()?;
        let listed: HashSet<String> = file.leaf_names().into_iter().collect();
        let live: HashSet<String> = snap
            .panes_of(self.ws)
            .filter_map(|p| p.label.clone())
            .collect();
        for label in &live {
            if listed.contains(label) {
                self.state.known.insert(label.clone());
            }
        }
        self.state.known.retain(|l| live.contains(l));
        self.state.protected.retain(|l| live.contains(l));
        for tab in &file.tabs {
            for leaf in tab.tree.leaves() {
                if leaf.mark != Mark::Managed {
                    self.state.protected.insert(leaf.name.clone());
                } else {
                    self.state.protected.remove(&leaf.name);
                }
            }
        }
        self.state.save(self.ws);
        self.state.ratios.clear();
        for tab in snap.tabs_of(self.ws) {
            let (Some(label), Some(tree)) = (&tab.label, LiveNode::of_tab(&snap, &tab.tab_id))
            else {
                continue;
            };
            let ratios: BTreeMap<Vec<bool>, f64> = tree.ratios().into_iter().collect();
            self.state.ratios.insert(label.clone(), ratios);
        }
        for tab in &file.tabs {
            for leaf in tab.tree.leaves() {
                if leaf.name == crate::services::RESERVED
                    && snap.pane_by_label(self.ws, &leaf.name).is_none()
                {
                    self.report
                        .notes
                        .push(format!("tab `{}`: agent is not running", tab.name));
                }
            }
        }
        Ok(())
    }
}

fn container_dir_of(tree: &crate::layout::Container, name: &str) -> Option<crate::layout::Dir> {
    for child in &tree.children {
        match child {
            crate::layout::Node::Leaf(l) if l.name == name => return Some(tree.dir),
            crate::layout::Node::Box(c) => {
                if let Some(d) = container_dir_of(c, name) {
                    return Some(d);
                }
            }
            _ => {}
        }
    }
    None
}

/// Does herdr already match the file? Used after apply to decide whether a
/// command can return.
pub fn converged(
    backend: &dyn Backend,
    ws: &str,
    file: &WorkspaceFile,
    services: &Services,
) -> Result<Vec<String>> {
    let snap = backend.snapshot()?;
    let mut problems = Vec::new();
    for tab in &file.tabs {
        let placeable = |l: &Leaf| {
            snap.pane_by_label(ws, &l.name).is_some()
                || (l.mark == Mark::Managed && services.contains(&l.name))
        };
        let Some(desired) = shape_of(&tab.tree, &placeable) else {
            continue;
        };
        let Some(live_tab) = snap.tab_by_label(ws, &tab.name) else {
            problems.push(format!("tab `{}` is missing", tab.name));
            continue;
        };
        let mut names = Vec::new();
        desired.leaves(&mut names);
        let set: HashSet<String> = names.into_iter().collect();
        let live = LiveNode::of_tab(&snap, &live_tab.tab_id)
            .and_then(|l| l.prune(&|n| n.map(|n| set.contains(n)).unwrap_or(false)));
        if !live
            .map(|l| l.shape().same_layout(&desired))
            .unwrap_or(false)
        {
            problems.push(format!("tab `{}` does not match yet", tab.name));
        }
    }
    Ok(problems)
}
