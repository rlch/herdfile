//! Fold hand changes into the workspace file before each apply: hand closes,
//! panes opened outside herdfile, and dragged sizes.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;

use crate::backend::{Backend, Pane, Snapshot};
use crate::layout::{round5, shape_of, Container, Dir, Leaf, Mark, Node};
use crate::live::{shares_by_parent, LiveNode};
use crate::wsfile::WorkspaceFile;

/// What the watcher remembers about one workspace between passes. Nothing
/// here is written to the file.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct WsState {
    /// Labels seen live while listed in the file. A known label that
    /// disappears was closed by hand; an unknown one has not been opened yet.
    pub known: HashSet<String>,
    /// Labels last listed as `mine` or `unmanaged`: never closed, even once
    /// dropped from the file.
    #[serde(default)]
    pub protected: HashSet<String>,
    /// Split ratios herdr had after our last pass, per tab label.
    #[serde(skip)]
    pub ratios: HashMap<String, BTreeMap<Vec<bool>, f64>>,
    /// Labels waiting for their agent to go idle before closing.
    #[serde(skip)]
    pub pending: HashSet<String>,
}

impl WsState {
    /// What the watcher saw, kept beside the workspace file so a restarted
    /// watcher still tells a hand close from a pane never opened.
    fn seen_path(ws: &str) -> std::path::PathBuf {
        crate::paths::state_dir().join(format!(".{ws}.seen.json"))
    }

    pub fn load(ws: &str) -> WsState {
        std::fs::read_to_string(Self::seen_path(ws))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, ws: &str) {
        if let Ok(text) = serde_json::to_string(self) {
            let _ = crate::paths::write_atomic(&Self::seen_path(ws), &text);
        }
    }
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct WritebackReport {
    /// Panes closed by hand and dropped from the file.
    pub dropped: Vec<String>,
    /// Panes opened outside herdfile and recorded as unmanaged.
    pub recorded: Vec<String>,
    /// Tabs whose sizes were written back from a drag.
    pub resized: Vec<String>,
}

impl WritebackReport {
    pub fn is_empty(&self) -> bool {
        self.dropped.is_empty() && self.recorded.is_empty() && self.resized.is_empty()
    }
}

/// A fresh label like `shell-1` that is neither live nor in the file.
pub fn generate_label(prefix: &str, taken: &HashSet<String>) -> String {
    (1..)
        .map(|n| format!("{prefix}-{n}"))
        .find(|l| !taken.contains(l))
        .expect("unbounded")
}

pub fn label_prefix(pane: &Pane) -> String {
    match pane.agent.as_deref() {
        Some(agent) if !agent.is_empty() => agent
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect(),
        _ => "shell".to_string(),
    }
}

/// Label every unlabelled pane in a workspace. Returns the labels given.
pub fn label_unlabelled(
    backend: &dyn Backend,
    snap: &mut Snapshot,
    ws: &str,
    file_names: &HashSet<String>,
) -> Result<Vec<String>> {
    let mut taken: HashSet<String> = file_names.clone();
    taken.extend(snap.panes_of(ws).filter_map(|p| p.label.clone()));
    let mut given = Vec::new();
    for pane in snap.panes.iter_mut().filter(|p| p.workspace_id == ws) {
        if pane.label.is_none() {
            let label = generate_label(&label_prefix(pane), &taken);
            backend.rename_pane(&pane.pane_id, Some(&label))?;
            taken.insert(label.clone());
            pane.label = Some(label.clone());
            given.push(label);
        }
    }
    Ok(given)
}

/// Where a new pane sits in the file: next to its live sibling when that
/// sibling is a single pane listed in the file.
fn anchor_for(tree: &LiveNode, pane_id: &str, file: &WorkspaceFile) -> Option<(String, Dir, bool)> {
    let (dir, is_second, sibling) = tree.sibling_of(pane_id)?;
    let LiveNode::Leaf {
        label: Some(label), ..
    } = sibling
    else {
        return None;
    };
    file.find(label)?;
    Some((label.clone(), dir, is_second))
}

/// Fold hand changes in workspace `ws` into `file`. Renames unlabelled panes
/// in herdr (labels are herdfile's identity). Returns what changed.
pub fn writeback(
    backend: &dyn Backend,
    snap: &mut Snapshot,
    ws: &str,
    file: &mut WorkspaceFile,
    state: &mut WsState,
) -> Result<WritebackReport> {
    let mut report = WritebackReport::default();
    let file_names: HashSet<String> = file.leaf_names().into_iter().collect();
    let labelled = label_unlabelled(backend, snap, ws, &file_names)?;

    // Panes opened outside herdfile: unlabelled ones we just named, and
    // labelled ones never seen in the file.
    let new_panes: HashSet<String> = snap
        .panes_of(ws)
        .filter_map(|p| p.label.clone())
        .filter(|l| {
            labelled.contains(l)
                || (!file_names.contains(l)
                    && !state.known.contains(l)
                    && !state.pending.contains(l))
        })
        .collect();
    let mut tabs: Vec<_> = snap.tabs_of(ws).cloned().collect();
    tabs.sort_by_key(|t| t.number);
    for tab in &tabs {
        let Some(tree) = LiveNode::of_tab(snap, &tab.tab_id) else {
            continue;
        };
        let tab_name = tab.label.clone().unwrap_or_else(|| tab.number.to_string());
        for (pane_id, label) in tree.leaves() {
            let Some(label) = label else { continue };
            if !new_panes.contains(&label) {
                continue;
            }
            let pane = snap.pane(&pane_id);
            let leaf = Leaf {
                name: label.clone(),
                size: None,
                mark: Mark::Unmanaged,
                cwd: pane.and_then(|p| p.cwd.clone()),
            };
            let anchor = anchor_for(&tree, &pane_id, file);
            let anchor_ref = anchor
                .as_ref()
                .map(|(a, d, after)| (a.as_str(), Some(*d), *after));
            file.insert(&tab_name, leaf, anchor_ref)?;
            state.known.insert(label.clone());
            report.recorded.push(label);
        }
    }

    // Listed panes that are gone.
    let live: HashSet<String> = snap.panes_of(ws).filter_map(|p| p.label.clone()).collect();
    for name in file.leaf_names() {
        if live.contains(&name) {
            continue;
        }
        let mark = file.find(&name).map(|(_, l)| l.mark).unwrap_or_default();
        if mark != Mark::Managed || state.known.contains(&name) {
            file.remove(&name);
            state.known.remove(&name);
            report.dropped.push(name);
        }
    }

    // Dragged sizes: only for tabs whose ratios moved since our last pass.
    for tab in &tabs {
        let Some(label) = &tab.label else { continue };
        let Some(before) = state.ratios.get(label) else {
            continue;
        };
        let Some(tree) = LiveNode::of_tab(snap, &tab.tab_id) else {
            continue;
        };
        let now: BTreeMap<Vec<bool>, f64> = tree.ratios().into_iter().collect();
        let moved = now.len() == before.len()
            && now
                .iter()
                .any(|(p, r)| before.get(p).map(|b| (b - r).abs() > 0.001).unwrap_or(true));
        if !moved {
            continue;
        }
        if let Some(tree_file) = file.tab(label).map(|t| t.tree.clone()) {
            if let Some(updated) = sizes_from_live(&tree_file, &tree) {
                if file.replace_tree_if_changed(label, updated) {
                    report.resized.push(label.clone());
                }
            }
        }
        state.ratios.insert(label.clone(), now);
    }
    Ok(report)
}

/// Write live shares into a file tree, rounded to 5%. Returns `None` when the
/// live layout differs from the file's or nothing rounds differently.
pub fn sizes_from_live(tree: &Container, live: &LiveNode) -> Option<Container> {
    let names: HashSet<String> = tree.leaf_names().into_iter().collect();
    let pruned = live.prune(&|l| l.map(|l| names.contains(l)).unwrap_or(false))?;
    let live_shape = pruned.shape();
    let file_shape = shape_of(tree, &|_| true)?;
    if !live_shape.same_layout(&file_shape) {
        return None;
    }
    let live_shares = shares_by_parent(&live_shape);
    let mut out = tree.clone();
    let mut changed = false;
    fn walk(c: &mut Container, shares: &HashMap<(String, Dir), f64>, changed: &mut bool) {
        let normal = c.children.len() > 1
            && c
                .children
                .iter()
                .all(|ch| !matches!(ch, Node::Box(inner) if inner.dir == c.dir || inner.children.len() < 2));
        if normal {
            let expected = c.fractions();
            let last_unsized = c.children.iter().rposition(|ch| ch.size().is_none());
            let mut proposal = c.clone();
            let mut any = false;
            for (i, child) in c.children.iter().enumerate() {
                let Some(actual) = shares.get(&(child.first_leaf().name.clone(), c.dir)) else {
                    continue;
                };
                if round5(*actual) == round5(expected[i]) || Some(i) == last_unsized {
                    continue;
                }
                proposal.children[i].set_size(Some(round5(*actual)));
                any = true;
            }
            if any && proposal.validate("").is_ok() {
                *c = proposal;
                *changed = true;
            }
        }
        for child in &mut c.children {
            if let Node::Box(inner) = child {
                walk(inner, shares, changed);
            }
        }
    }
    walk(&mut out, &live_shares, &mut changed);
    changed.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::parse_tree_arg;

    fn live(ratio: f64) -> LiveNode {
        LiveNode::Split {
            dir: Dir::Row,
            ratio,
            path: vec![],
            first: Box::new(LiveNode::Leaf {
                pane_id: "w1:p1".into(),
                label: Some("agent".into()),
            }),
            second: Box::new(LiveNode::Leaf {
                pane_id: "w1:p2".into(),
                label: Some("test".into()),
            }),
        }
    }

    #[test]
    fn drag_rounds_to_five() {
        let tree =
            parse_tree_arg(r#"row = [{ pane = "agent", size = 60 }, "test"]"#, "main").unwrap();
        let out = sizes_from_live(&tree, &live(0.73)).unwrap();
        assert_eq!(
            format!("{}", out.to_array()),
            r#"[{ pane = "agent", size = 75 }, "test"]"#
        );
    }

    #[test]
    fn small_drag_changes_nothing() {
        let tree =
            parse_tree_arg(r#"row = [{ pane = "agent", size = 60 }, "test"]"#, "main").unwrap();
        assert!(sizes_from_live(&tree, &live(0.61)).is_none());
    }

    #[test]
    fn unsized_pair_gets_first_size() {
        let tree = parse_tree_arg(r#"row = ["agent", "test"]"#, "main").unwrap();
        let out = sizes_from_live(&tree, &live(0.7)).unwrap();
        assert_eq!(
            format!("{}", out.to_array()),
            r#"[{ pane = "agent", size = 70 }, "test"]"#
        );
    }

    #[test]
    fn labels_are_unique() {
        let taken: HashSet<String> = ["shell-1".to_string()].into();
        assert_eq!(generate_label("shell", &taken), "shell-2");
    }
}
