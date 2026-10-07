//! Fold what is live into the workspace file before each apply: panes the
//! file does not know (all of them, the first time a workspace is seen),
//! hand closes, and dragged sizes.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::Result;

use crate::backend::{Backend, Pane, Snapshot};
use crate::layout::{round5, shape_of, Container, Dir, Leaf, Mark, Node, Shape};
use crate::live::{shares_by_parent, LiveNode};
use crate::services::{Services, RESERVED};
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
    /// Each tab's label after our last pass, by tab id, to notice renames.
    #[serde(default)]
    pub tabs: BTreeMap<String, String>,
    /// Tabs given a fixed label, by tab id.
    #[serde(default)]
    pub pinned: BTreeSet<String>,
    /// Each tab's layout (structure and labels, no sizes) after our last
    /// pass, by tab id. A tab that differs was changed outside herdfile.
    #[serde(default)]
    pub layouts: BTreeMap<String, String>,
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
    /// Tabs renamed outside herdfile, as `old -> new`.
    #[serde(default)]
    pub renamed: Vec<String>,
    /// Panes moved to another tab outside herdfile.
    #[serde(default)]
    pub moved: Vec<String>,
    /// Tabs whose layout was changed outside herdfile and taken as it is.
    #[serde(default)]
    pub reshaped: Vec<String>,
}

impl WritebackReport {
    pub fn is_empty(&self) -> bool {
        self.dropped.is_empty()
            && self.recorded.is_empty()
            && self.resized.is_empty()
            && self.renamed.is_empty()
            && self.moved.is_empty()
            && self.reshaped.is_empty()
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

/// Give every tab of a workspace a unique, fixed label: the file names tabs
/// by label, and herdr neither requires labels to be unique nor keeps an
/// unnamed tab's label (it is its position, and shifts when tabs move or
/// close). Each tab is renamed once, to the label it shows, which fixes it.
fn label_tabs(
    backend: &dyn Backend,
    snap: &mut Snapshot,
    ws: &str,
    pinned: &mut BTreeSet<String>,
) -> Result<()> {
    let mut tabs: Vec<(String, u32, Option<String>)> = snap
        .tabs_of(ws)
        .map(|t| (t.tab_id.clone(), t.number, t.label.clone()))
        .collect();
    tabs.sort_by_key(|t| t.1);
    let mut seen = HashSet::new();
    for (id, number, label) in tabs {
        let mut name = label.clone().unwrap_or_else(|| number.to_string());
        if !seen.insert(name.clone()) {
            name = format!("{name}-{number}");
            seen.insert(name.clone());
        }
        if label.as_deref() != Some(name.as_str()) || !pinned.contains(&id) {
            backend.rename_tab(&id, &name)?;
            pinned.insert(id.clone());
            if let Some(t) = snap.tabs.iter_mut().find(|t| t.tab_id == id) {
                t.label = Some(name);
            }
        }
    }
    Ok(())
}

/// Label every unlabelled pane in a workspace, in screen order. The first
/// one hosting an agent becomes `agent` when the workspace has none.
pub fn label_unlabelled(
    backend: &dyn Backend,
    snap: &mut Snapshot,
    ws: &str,
    file_names: &HashSet<String>,
) -> Result<Vec<String>> {
    let mut taken: HashSet<String> = file_names.clone();
    taken.extend(snap.panes_of(ws).filter_map(|p| p.label.clone()));
    let mut agent_free = !taken.contains(RESERVED);
    let mut tabs: Vec<_> = snap.tabs_of(ws).cloned().collect();
    tabs.sort_by_key(|t| t.number);
    let order: Vec<String> = tabs
        .iter()
        .filter_map(|t| LiveNode::of_tab(snap, &t.tab_id))
        .flat_map(|tree| tree.leaves().into_iter().map(|(id, _)| id))
        .collect();
    let mut given = Vec::new();
    for id in order {
        let Some(pane) = snap.panes.iter_mut().find(|p| p.pane_id == id) else {
            continue;
        };
        if pane.label.is_some() {
            continue;
        }
        let label = if agent_free && pane.agent.is_some() {
            agent_free = false;
            RESERVED.to_string()
        } else {
            generate_label(&label_prefix(pane), &taken)
        };
        backend.rename_pane(&pane.pane_id, Some(&label))?;
        taken.insert(label.clone());
        pane.label = Some(label.clone());
        given.push(label);
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

/// Fold what is live into `file`: panes it does not know (including, on
/// first sight, every pane of a workspace with no file yet), hand closes,
/// and dragged sizes. Renames unlabelled panes and tabs in herdr, since
/// labels are herdfile's identity. Returns what changed.
pub fn writeback(
    backend: &dyn Backend,
    snap: &mut Snapshot,
    ws: &str,
    file: &mut WorkspaceFile,
    state: &mut WsState,
    services: &Services,
) -> Result<WritebackReport> {
    let mut report = WritebackReport::default();
    // A tab renamed outside herdfile keeps its place in the file under the
    // new name, rather than being rebuilt under the old one.
    for tab in snap.tabs_of(ws) {
        let (Some(old), Some(new)) = (state.tabs.get(&tab.tab_id), &tab.label) else {
            continue;
        };
        if old != new && file.tab(new).is_none() && file.rename_tab(old, new) {
            if let Some(r) = state.ratios.remove(old) {
                state.ratios.insert(new.clone(), r);
            }
            report.renamed.push(format!("{old} -> {new}"));
        }
    }
    label_tabs(backend, snap, ws, &mut state.pinned)?;
    let file_names: HashSet<String> = file.leaf_names().into_iter().collect();
    let labelled = label_unlabelled(backend, snap, ws, &file_names)?;

    // Panes the file does not know: unlabelled ones we just named, and
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
    // Listed panes that are gone: closed by hand.
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

    // Services and the agent are the file's own; anything else was opened
    // outside it and is never closed automatically. A pane the file already
    // lists keeps its mark.
    let listed: HashMap<String, (String, Leaf)> = file
        .tabs
        .iter()
        .flat_map(|t| {
            t.tree
                .leaves()
                .into_iter()
                .map(|l| (l.name.clone(), (t.name.clone(), l.clone())))
                .collect::<Vec<_>>()
        })
        .collect();
    let leaf_for = |label: &str| {
        if let Some((_, l)) = listed.get(label) {
            return Leaf {
                size: None,
                ..l.clone()
            };
        }
        if label == RESERVED || services.contains(label) {
            Leaf::new(label)
        } else {
            Leaf {
                name: label.to_string(),
                size: None,
                mark: Mark::Unmanaged,
                cwd: snap.pane_by_label(ws, label).and_then(|p| p.cwd.clone()),
            }
        }
    };

    let mut tabs: Vec<_> = snap.tabs_of(ws).cloned().collect();
    tabs.sort_by_key(|t| t.number);

    // Tabs whose layout changed since our last pass were changed outside
    // herdfile (a split, a move, a swap): the file takes them as they are.
    let changed: Vec<(String, LiveNode)> = tabs
        .iter()
        .filter_map(|t| {
            let before = state.layouts.get(&t.tab_id)?;
            let tree = LiveNode::of_tab(snap, &t.tab_id)?;
            (&layout_key(&tree.shape()) != before).then(|| {
                (
                    t.label.clone().unwrap_or_else(|| t.number.to_string()),
                    tree,
                )
            })
        })
        .collect();
    let placed: HashSet<String> = changed
        .iter()
        .flat_map(|(_, tree)| tree.leaves().into_iter().filter_map(|(_, l)| l))
        .collect();
    for (tab_name, tree) in &changed {
        // What the file still wants in this tab but is not on screen here
        // (not opened yet, or placed by a command): kept after its neighbour.
        let carried: Vec<(Option<String>, Leaf)> = file
            .tab(tab_name)
            .map(|t| {
                let leaves = t.tree.leaves();
                leaves
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| !placed.contains(&l.name))
                    .map(|(i, l)| {
                        (
                            i.checked_sub(1).map(|j| leaves[j].name.clone()),
                            (*l).clone(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (_, label) in tree.leaves() {
            let Some(label) = label else { continue };
            if let Some((was, _)) = listed.get(&label) {
                if was != tab_name {
                    report.moved.push(label.clone());
                }
            } else if new_panes.contains(&label) {
                state.known.insert(label.clone());
                report.recorded.push(label.clone());
            }
            // Listed in another tab: it moved here.
            if file
                .find(&label)
                .map(|(t, _)| &t.name != tab_name)
                .unwrap_or(false)
            {
                file.remove(&label);
            }
        }
        file.set_tab(tab_name, tree_from_live(&tree.shape(), &leaf_for));
        for (prev, leaf) in carried {
            let anchor = prev
                .as_deref()
                .filter(|p| {
                    file.tab(tab_name)
                        .map(|t| t.tree.leaf_names().iter().any(|n| n == p))
                        .unwrap_or(false)
                })
                .map(|p| (p, None, true));
            file.insert(tab_name, leaf, anchor)?;
        }
        report.reshaped.push(tab_name.clone());
    }
    let changed_names: HashSet<String> = changed.iter().map(|(n, _)| n.clone()).collect();

    // Elsewhere, panes the file does not know are added where they sit.
    for tab in &tabs {
        let Some(tree) = LiveNode::of_tab(snap, &tab.tab_id) else {
            continue;
        };
        let tab_name = tab.label.clone().unwrap_or_else(|| tab.number.to_string());
        if changed_names.contains(&tab_name) {
            continue;
        }
        let leaves = tree.leaves();
        let new_here: Vec<(String, String)> = leaves
            .iter()
            .filter_map(|(id, l)| {
                l.clone()
                    .filter(|l| new_panes.contains(l) && file.find(l).is_none())
                    .map(|l| (id.clone(), l))
            })
            .collect();
        if new_here.is_empty() {
            continue;
        }
        if file.tab(&tab_name).is_none() && new_here.len() == leaves.len() {
            // A whole tab the file has never seen: take it as it is.
            file.set_tab(&tab_name, tree_from_live(&tree.shape(), &leaf_for));
        } else {
            for (pane_id, label) in &new_here {
                let anchor = anchor_for(&tree, pane_id, file);
                let anchor_ref = anchor
                    .as_ref()
                    .map(|(a, d, after)| (a.as_str(), Some(*d), *after));
                file.insert(&tab_name, leaf_for(label), anchor_ref)?;
            }
            // Keep the screen as it is: the tab's sizes come from live.
            if let Some(current) = file.tab(&tab_name).map(|t| t.tree.clone()) {
                if let Some(updated) = exact_sizes_from_live(&current, &tree) {
                    file.replace_tree_if_changed(&tab_name, updated);
                }
            }
        }
        for (_, label) in new_here {
            state.known.insert(label.clone());
            report.recorded.push(label);
        }
    }

    // Dragged sizes: only for tabs whose ratios moved since our last pass.
    for tab in &tabs {
        let Some(label) = &tab.label else { continue };
        if changed_names.contains(label) {
            if let Some(tree) = LiveNode::of_tab(snap, &tab.tab_id) {
                state
                    .ratios
                    .insert(label.clone(), tree.ratios().into_iter().collect());
            }
            continue;
        }
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

/// A tab's layout as text: structure and labels, no sizes.
pub fn layout_key(shape: &Shape) -> String {
    match shape {
        Shape::Leaf(l) => l.clone(),
        Shape::Split(dir, children) => format!(
            "{}({})",
            dir.key(),
            children
                .iter()
                .map(|(c, _)| layout_key(c))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

/// A file tree for a live layout, with sizes to the whole percent when panes
/// are not shared equally.
pub fn tree_from_live(shape: &Shape, leaf: &dyn Fn(&str) -> Leaf) -> Container {
    match shape {
        Shape::Leaf(name) => Container {
            dir: Dir::Row,
            size: None,
            children: vec![Node::Leaf(leaf(name))],
        },
        Shape::Split(dir, children) => {
            let shares: Vec<f64> = children.iter().map(|c| c.1).collect();
            let sizes = whole_sizes(&shares);
            let mut out = Vec::new();
            for ((child, _), size) in children.iter().zip(sizes) {
                let mut node = match child {
                    Shape::Leaf(name) => Node::Leaf(leaf(name)),
                    Shape::Split(..) => Node::Box(tree_from_live(child, leaf)),
                };
                node.set_size(size);
                out.push(node);
            }
            Container {
                dir: *dir,
                size: None,
                children: out,
            }
        }
    }
}

/// Sizes for children with these shares: none when equal, else whole
/// percents for all but the last, which takes the rest.
fn whole_sizes(shares: &[f64]) -> Vec<Option<u8>> {
    let n = shares.len() as f64;
    if shares.iter().all(|s| (s - 1.0 / n).abs() < 0.01) {
        return vec![None; shares.len()];
    }
    let mut out: Vec<Option<u8>> = shares[..shares.len() - 1]
        .iter()
        .map(|s| Some(((s * 100.0).round() as u8).clamp(1, 98)))
        .collect();
    let used: u32 = out.iter().flatten().map(|s| u32::from(*s)).sum();
    if used >= 100 {
        return vec![None; shares.len()];
    }
    out.push(None);
    out
}

/// After a pane is recorded into an existing tab, set every size in the tab
/// to what is on screen (whole percents), so apply moves no divider.
/// `None` when the layouts differ or nothing changes.
pub fn exact_sizes_from_live(tree: &Container, live: &LiveNode) -> Option<Container> {
    let names: HashSet<String> = tree.leaf_names().into_iter().collect();
    let pruned = live.prune(&|l| l.map(|l| names.contains(l)).unwrap_or(false))?;
    let live_shape = pruned.shape();
    if !live_shape.same_layout(&shape_of(tree, &|_| true)?) {
        return None;
    }
    let shares = shares_by_parent(&live_shape);
    fn walk(c: &mut Container, shares: &HashMap<(String, Dir), f64>) -> bool {
        let normal = c.children.len() > 1
            && c.children.iter().all(|ch| {
                !matches!(ch, Node::Box(inner) if inner.dir == c.dir || inner.children.len() < 2)
            });
        if !normal {
            return false;
        }
        let live: Option<Vec<f64>> = c
            .children
            .iter()
            .map(|ch| shares.get(&(ch.first_leaf().name.clone(), c.dir)).copied())
            .collect();
        let Some(live) = live else { return false };
        for (child, size) in c.children.iter_mut().zip(whole_sizes(&live)) {
            child.set_size(size);
        }
        c.children.iter_mut().all(|ch| match ch {
            Node::Box(inner) => walk(inner, shares),
            Node::Leaf(_) => true,
        })
    }
    let mut out = tree.clone();
    if !walk(&mut out, &shares) || out.validate("").is_err() || &out == tree {
        return None;
    }
    Some(out)
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

    #[test]
    fn whole_sizes_round_and_leave_the_last() {
        assert_eq!(whole_sizes(&[0.5, 0.5]), [None, None]);
        assert_eq!(whole_sizes(&[0.333, 0.334, 0.333]), [None, None, None]);
        assert_eq!(whole_sizes(&[0.65, 0.35]), [Some(65), None]);
        assert_eq!(whole_sizes(&[0.25, 0.25, 0.5]), [Some(25), Some(25), None]);
    }

    #[test]
    fn layout_key_ignores_sizes() {
        let a = live(0.7).shape();
        let b = live(0.3).shape();
        assert_eq!(layout_key(&a), "row(agent,test)");
        assert_eq!(layout_key(&a), layout_key(&b));
    }

    #[test]
    fn tree_from_live_keeps_the_screen() {
        let tree = tree_from_live(&live(0.65).shape(), &|n: &str| Leaf::new(n));
        assert_eq!(
            format!("{}", tree.to_array()),
            r#"[{ pane = "agent", size = 65 }, "test"]"#
        );
        let back = shape_of(&tree, &|_| true).unwrap();
        assert!(back.same_layout(&live(0.65).shape()));
    }

    #[test]
    fn exact_sizes_follow_the_screen() {
        let tree = parse_tree_arg(r#"row = ["agent", "test"]"#, "main").unwrap();
        let out = exact_sizes_from_live(&tree, &live(0.62)).unwrap();
        assert_eq!(
            format!("{}", out.to_array()),
            r#"[{ pane = "agent", size = 62 }, "test"]"#
        );
        assert!(
            exact_sizes_from_live(&tree, &live(0.5)).is_none(),
            "already equal"
        );
        let other = parse_tree_arg(r#"column = ["agent", "test"]"#, "main").unwrap();
        assert!(
            exact_sizes_from_live(&other, &live(0.62)).is_none(),
            "different layout"
        );
    }

    #[test]
    fn label_prefix_names_the_agent() {
        let pane = |agent: Option<&str>| Pane {
            pane_id: "w1:p1".into(),
            tab_id: "w1:t1".into(),
            workspace_id: "w1".into(),
            label: None,
            cwd: None,
            agent: agent.map(str::to_string),
            agent_status: None,
        };
        assert_eq!(label_prefix(&pane(None)), "shell");
        assert_eq!(label_prefix(&pane(Some("claude"))), "claude");
        assert_eq!(label_prefix(&pane(Some("Open Code"))), "open-code");
    }
}
