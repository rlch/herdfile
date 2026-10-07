//! herdr's live layout of a tab as a binary tree, rebuilt from a snapshot.

use std::collections::HashMap;

use crate::backend::{Layout, Snapshot};
use crate::layout::{Dir, Shape};

#[derive(Debug, Clone)]
pub enum LiveNode {
    Leaf {
        pane_id: String,
        label: Option<String>,
    },
    Split {
        dir: Dir,
        ratio: f64,
        /// herdr's split path: `false` is the first child, `true` the second.
        path: Vec<bool>,
        first: Box<LiveNode>,
        second: Box<LiveNode>,
    },
}

impl LiveNode {
    /// Rebuild a tab's tree. herdr lists splits by path and panes in
    /// depth-first order.
    pub fn of_layout(layout: &Layout, snap: &Snapshot) -> Option<LiveNode> {
        let splits: HashMap<Vec<bool>, (Dir, f64)> = layout
            .splits
            .iter()
            .filter_map(|s| {
                let path = parse_split_path(&s.id)?;
                Some((path, (Dir::from_split(&s.direction)?, s.ratio)))
            })
            .collect();
        let mut panes = layout.panes.iter();
        fn build(
            path: Vec<bool>,
            splits: &HashMap<Vec<bool>, (Dir, f64)>,
            panes: &mut std::slice::Iter<'_, crate::backend::LayoutPane>,
            snap: &Snapshot,
        ) -> Option<LiveNode> {
            if let Some(&(dir, ratio)) = splits.get(&path) {
                let mut p1 = path.clone();
                p1.push(false);
                let mut p2 = path.clone();
                p2.push(true);
                let first = build(p1, splits, panes, snap)?;
                let second = build(p2, splits, panes, snap)?;
                Some(LiveNode::Split {
                    dir,
                    ratio,
                    path,
                    first: Box::new(first),
                    second: Box::new(second),
                })
            } else {
                let pane = panes.next()?;
                Some(LiveNode::Leaf {
                    pane_id: pane.pane_id.clone(),
                    label: snap.pane(&pane.pane_id).and_then(|p| p.label.clone()),
                })
            }
        }
        build(Vec::new(), &splits, &mut panes, snap)
    }

    pub fn of_tab(snap: &Snapshot, tab_id: &str) -> Option<LiveNode> {
        LiveNode::of_layout(snap.layout(tab_id)?, snap)
    }

    /// Display name of a leaf: its label, or `#<pane id>` when unlabelled.
    fn leaf_name(pane_id: &str, label: &Option<String>) -> String {
        label.clone().unwrap_or_else(|| format!("#{pane_id}"))
    }

    pub fn first_leaf(&self) -> (&str, Option<&str>) {
        match self {
            LiveNode::Leaf { pane_id, label } => (pane_id, label.as_deref()),
            LiveNode::Split { first, .. } => first.first_leaf(),
        }
    }

    /// Pane ids and labels in depth-first order.
    pub fn leaves(&self) -> Vec<(String, Option<String>)> {
        let mut out = Vec::new();
        fn walk(n: &LiveNode, out: &mut Vec<(String, Option<String>)>) {
            match n {
                LiveNode::Leaf { pane_id, label } => out.push((pane_id.clone(), label.clone())),
                LiveNode::Split { first, second, .. } => {
                    walk(first, out);
                    walk(second, out);
                }
            }
        }
        walk(self, &mut out);
        out
    }

    /// Keep only leaves whose label `keep` accepts; splits that lose a side
    /// dissolve. Surviving splits keep their herdr paths.
    pub fn prune(&self, keep: &dyn Fn(Option<&str>) -> bool) -> Option<LiveNode> {
        match self {
            LiveNode::Leaf { label, .. } => keep(label.as_deref()).then(|| self.clone()),
            LiveNode::Split {
                dir,
                ratio,
                path,
                first,
                second,
            } => match (first.prune(keep), second.prune(keep)) {
                (Some(a), Some(b)) => Some(LiveNode::Split {
                    dir: *dir,
                    ratio: *ratio,
                    path: path.clone(),
                    first: Box::new(a),
                    second: Box::new(b),
                }),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            },
        }
    }

    /// The normalised shape, with each child's actual share.
    pub fn shape(&self) -> Shape {
        match self {
            LiveNode::Leaf { pane_id, label } => Shape::Leaf(Self::leaf_name(pane_id, label)),
            LiveNode::Split {
                dir,
                ratio,
                first,
                second,
                ..
            } => Shape::split(
                *dir,
                vec![(first.shape(), *ratio), (second.shape(), 1.0 - *ratio)],
            ),
        }
    }

    /// Every split's path and ratio.
    pub fn ratios(&self) -> Vec<(Vec<bool>, f64)> {
        let mut out = Vec::new();
        fn walk(n: &LiveNode, out: &mut Vec<(Vec<bool>, f64)>) {
            if let LiveNode::Split {
                path,
                ratio,
                first,
                second,
                ..
            } = n
            {
                out.push((path.clone(), *ratio));
                walk(first, out);
                walk(second, out);
            }
        }
        walk(self, &mut out);
        out
    }

    /// For each split, the ratio that gives `desired`'s shares. Returns
    /// (path, current ratio, wanted ratio). The tree must already have the
    /// desired layout.
    pub fn wanted_ratios(&self, desired: &Shape) -> Vec<(Vec<bool>, f64, f64)> {
        let shares = shares_by_parent(desired);
        let mut out = Vec::new();
        fn weight(n: &LiveNode, group: Dir, shares: &HashMap<(String, Dir), f64>) -> f64 {
            match n {
                LiveNode::Split {
                    dir, first, second, ..
                } if *dir == group => weight(first, group, shares) + weight(second, group, shares),
                other => {
                    let (pane_id, label) = other.first_leaf();
                    let name = label
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("#{pane_id}"));
                    shares.get(&(name, group)).copied().unwrap_or(0.0)
                }
            }
        }
        fn walk(
            n: &LiveNode,
            shares: &HashMap<(String, Dir), f64>,
            out: &mut Vec<(Vec<bool>, f64, f64)>,
        ) {
            if let LiveNode::Split {
                dir,
                ratio,
                path,
                first,
                second,
            } = n
            {
                let a = weight(first, *dir, shares);
                let b = weight(second, *dir, shares);
                if a + b > 0.0 {
                    out.push((path.clone(), *ratio, a / (a + b)));
                }
                walk(first, shares, out);
                walk(second, shares, out);
            }
        }
        walk(self, &shares, &mut out);
        out
    }

    /// The sibling of a leaf: (split direction, is the leaf the second
    /// child, sibling subtree).
    pub fn sibling_of(&self, pane: &str) -> Option<(Dir, bool, &LiveNode)> {
        if let LiveNode::Split {
            dir, first, second, ..
        } = self
        {
            if matches!(&**first, LiveNode::Leaf { pane_id, .. } if pane_id == pane) {
                return Some((*dir, false, second));
            }
            if matches!(&**second, LiveNode::Leaf { pane_id, .. } if pane_id == pane) {
                return Some((*dir, true, first));
            }
            return first.sibling_of(pane).or_else(|| second.sibling_of(pane));
        }
        None
    }
}

/// Each normalised child's share, keyed by (its first leaf, its parent's
/// direction). Directions alternate in a normalised tree, so keys are unique.
pub fn shares_by_parent(shape: &Shape) -> HashMap<(String, Dir), f64> {
    let mut out = HashMap::new();
    fn walk(s: &Shape, out: &mut HashMap<(String, Dir), f64>) {
        if let Shape::Split(dir, children) = s {
            for (child, share) in children {
                out.insert((child.first_leaf().to_string(), *dir), *share);
                walk(child, out);
            }
        }
    }
    walk(shape, &mut out);
    out
}

/// `split_2_11` → `[true, true]`; `split_0_root` → `[]`.
pub fn parse_split_path(id: &str) -> Option<Vec<bool>> {
    let rest = id.strip_prefix("split_")?;
    let (_, bits) = rest.split_once('_')?;
    if bits == "root" {
        return Some(Vec::new());
    }
    bits.chars()
        .map(|c| match c {
            '0' => Some(false),
            '1' => Some(true),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{parse_tree_arg, shape_of, Shape};

    fn snap_json(v: serde_json::Value) -> Snapshot {
        serde_json::from_value(v).unwrap()
    }

    fn sample() -> Snapshot {
        // agent | (test / (x | y)) as herdr reports it.
        snap_json(serde_json::json!({
            "panes": [
                {"pane_id": "w1:p1", "tab_id": "w1:t1", "workspace_id": "w1", "label": "agent"},
                {"pane_id": "w1:p2", "tab_id": "w1:t1", "workspace_id": "w1", "label": "test"},
                {"pane_id": "w1:p3", "tab_id": "w1:t1", "workspace_id": "w1"},
                {"pane_id": "w1:p4", "tab_id": "w1:t1", "workspace_id": "w1", "label": "y"}
            ],
            "layouts": [{
                "tab_id": "w1:t1", "workspace_id": "w1",
                "panes": [{"pane_id": "w1:p1"}, {"pane_id": "w1:p2"}, {"pane_id": "w1:p3"}, {"pane_id": "w1:p4"}],
                "splits": [
                    {"id": "split_0_root", "direction": "right", "ratio": 0.6},
                    {"id": "split_1_1", "direction": "down", "ratio": 0.5},
                    {"id": "split_2_11", "direction": "right", "ratio": 0.5}
                ]
            }]
        }))
    }

    #[test]
    fn paths() {
        assert_eq!(parse_split_path("split_0_root"), Some(vec![]));
        assert_eq!(parse_split_path("split_2_10"), Some(vec![true, false]));
    }

    #[test]
    fn rebuilds_tree_and_prunes() {
        let snap = sample();
        let tree = LiveNode::of_tab(&snap, "w1:t1").unwrap();
        let names: Vec<_> = tree.leaves().into_iter().map(|(id, _)| id).collect();
        assert_eq!(names, ["w1:p1", "w1:p2", "w1:p3", "w1:p4"]);
        let pruned = tree.prune(&|l| l.is_some()).unwrap();
        let want = shape_of(
            &parse_tree_arg(
                r#"row = [{ pane = "agent", size = 60 }, { column = ["test", "y"] }]"#,
                "t",
            )
            .unwrap(),
            &|_| true,
        )
        .unwrap();
        assert!(pruned.shape().same_layout(&want));
        let (dir, second, sib) = tree.sibling_of("w1:p3").unwrap();
        assert_eq!(dir, Dir::Row);
        assert!(!second);
        assert_eq!(sib.first_leaf().1, Some("y"));
    }

    #[test]
    fn wanted_ratios_match_sizes() {
        let snap = sample();
        let tree = LiveNode::of_tab(&snap, "w1:t1")
            .unwrap()
            .prune(&|l| l.is_some())
            .unwrap();
        let want = shape_of(
            &parse_tree_arg(r#"row = [{ pane = "agent", size = 70 }, { column = ["test", { pane = "y", size = 25 }] }]"#, "t").unwrap(),
            &|_| true,
        )
        .unwrap();
        let ratios = tree.wanted_ratios(&want);
        let root = ratios.iter().find(|r| r.0.is_empty()).unwrap();
        assert!((root.2 - 0.7).abs() < 1e-9);
        let col = ratios.iter().find(|r| r.0 == vec![true]).unwrap();
        assert!((col.2 - 0.75).abs() < 1e-9);
    }

    #[test]
    fn rebuilds_a_real_layout() {
        let snap: Snapshot =
            serde_json::from_str(include_str!("../tests/fixtures/snapshot.json")).unwrap();
        let tree = LiveNode::of_tab(&snap, "w1:t1").unwrap();
        let ids: Vec<_> = tree.leaves().into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, ["w1:p1", "w1:p2", "w1:p3"]);
        let ratios = tree.ratios();
        assert_eq!(ratios[0].0, Vec::<bool>::new());
        assert!((ratios[0].1 - 0.6).abs() < 1e-6);
        let shape = tree.shape();
        assert!(matches!(&shape, Shape::Split(Dir::Row, c) if c.len() == 2));
        assert!(LiveNode::of_tab(&snap, "w1:t2").is_some());
        assert!(LiveNode::of_tab(&snap, "w1:t9").is_none());
    }
}
