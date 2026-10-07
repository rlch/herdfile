//! A tab's layout: a tree of rows and columns of named panes, with optional sizes.

use anyhow::{anyhow, bail, Result};
use toml_edit::{Array, InlineTable, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dir {
    /// Left to right.
    Row,
    /// Top to bottom.
    Column,
}

impl Dir {
    pub fn key(self) -> &'static str {
        match self {
            Dir::Row => "row",
            Dir::Column => "column",
        }
    }

    /// herdr's split direction that puts the new pane after the target.
    pub fn split(self) -> &'static str {
        match self {
            Dir::Row => "right",
            Dir::Column => "down",
        }
    }

    pub fn from_split(direction: &str) -> Option<Dir> {
        match direction {
            "right" => Some(Dir::Row),
            "down" => Some(Dir::Column),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mark {
    /// Opened and closed by the watcher.
    #[default]
    Managed,
    /// The operator's own pane. Never closed by the watcher.
    Mine,
    /// Opened outside the file. Never closed automatically.
    Unmanaged,
}

impl Mark {
    pub fn parse(s: &str) -> Option<Mark> {
        match s {
            "managed" => Some(Mark::Managed),
            "mine" => Some(Mark::Mine),
            "unmanaged" => Some(Mark::Unmanaged),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mark::Managed => "managed",
            Mark::Mine => "mine",
            Mark::Unmanaged => "unmanaged",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leaf {
    pub name: String,
    pub size: Option<u8>,
    pub mark: Mark,
    pub cwd: Option<String>,
}

impl Leaf {
    pub fn new(name: &str) -> Leaf {
        Leaf {
            name: name.to_string(),
            size: None,
            mark: Mark::Managed,
            cwd: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    pub dir: Dir,
    pub size: Option<u8>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Leaf(Leaf),
    Box(Container),
}

impl Node {
    pub fn size(&self) -> Option<u8> {
        match self {
            Node::Leaf(l) => l.size,
            Node::Box(c) => c.size,
        }
    }

    pub fn set_size(&mut self, size: Option<u8>) {
        match self {
            Node::Leaf(l) => l.size = size,
            Node::Box(c) => c.size = size,
        }
    }

    pub fn first_leaf(&self) -> &Leaf {
        match self {
            Node::Leaf(l) => l,
            Node::Box(c) => c.children[0].first_leaf(),
        }
    }

    pub fn leaves<'a>(&'a self, out: &mut Vec<&'a Leaf>) {
        match self {
            Node::Leaf(l) => out.push(l),
            Node::Box(c) => c.children.iter().for_each(|n| n.leaves(out)),
        }
    }
}

impl Container {
    pub fn leaves(&self) -> Vec<&Leaf> {
        let mut out = Vec::new();
        self.children.iter().for_each(|n| n.leaves(&mut out));
        out
    }

    pub fn leaf_names(&self) -> Vec<String> {
        self.leaves().iter().map(|l| l.name.clone()).collect()
    }

    pub fn find_leaf_mut(&mut self, name: &str) -> Option<&mut Leaf> {
        for child in &mut self.children {
            match child {
                Node::Leaf(l) if l.name == name => return Some(l),
                Node::Leaf(_) => {}
                Node::Box(c) => {
                    if let Some(l) = c.find_leaf_mut(name) {
                        return Some(l);
                    }
                }
            }
        }
        None
    }

    /// Remove a leaf. Containers left with one child collapse into it; empty
    /// containers disappear. Returns the removed leaf.
    pub fn remove_leaf(&mut self, name: &str) -> Option<Leaf> {
        let mut removed = None;
        let mut i = 0;
        while i < self.children.len() {
            let drop_child = match &mut self.children[i] {
                Node::Leaf(l) if l.name == name => {
                    removed = Some(l.clone());
                    true
                }
                Node::Leaf(_) => false,
                Node::Box(c) => {
                    if let Some(l) = c.remove_leaf(name) {
                        removed = Some(l);
                    }
                    c.children.is_empty()
                }
            };
            if drop_child {
                self.children.remove(i);
            } else {
                if let Node::Box(c) = &self.children[i] {
                    if c.children.len() == 1 {
                        let size = c.size;
                        let mut only = c.children[0].clone();
                        only.set_size(size);
                        self.children[i] = only;
                    }
                }
                i += 1;
            }
            if removed.is_some() {
                break;
            }
        }
        removed
    }

    /// Insert `node` next to the leaf `anchor`. With `dir` equal to the
    /// anchor's container direction (or `None`), it goes into that container;
    /// otherwise the anchor is wrapped in a new container of `dir`.
    pub fn insert_near(&mut self, anchor: &str, node: Node, dir: Option<Dir>, after: bool) -> bool {
        let own_dir = self.dir;
        for i in 0..self.children.len() {
            match &mut self.children[i] {
                Node::Leaf(l) if l.name == anchor => {
                    if dir.is_none() || dir == Some(own_dir) {
                        let at = if after { i + 1 } else { i };
                        self.children.insert(at, node);
                    } else {
                        let mut existing = self.children[i].clone();
                        let size = existing.size();
                        existing.set_size(None);
                        let children = if after {
                            vec![existing, node]
                        } else {
                            vec![node, existing]
                        };
                        self.children[i] = Node::Box(Container {
                            dir: dir.unwrap_or(own_dir),
                            size,
                            children,
                        });
                    }
                    return true;
                }
                Node::Leaf(_) => {}
                Node::Box(c) => {
                    if c.insert_near(anchor, node.clone(), dir, after) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Each child's share of this container, summing to 1.
    pub fn fractions(&self) -> Vec<f64> {
        let sized: u32 = self
            .children
            .iter()
            .filter_map(|c| c.size())
            .map(u32::from)
            .sum();
        let unsized_count = self.children.iter().filter(|c| c.size().is_none()).count();
        if unsized_count == 0 {
            let total = sized.max(1) as f64;
            return self
                .children
                .iter()
                .map(|c| c.size().unwrap_or(0) as f64 / total)
                .collect();
        }
        let rest = (100.0 - sized as f64).max(0.0) / unsized_count as f64;
        self.children
            .iter()
            .map(|c| c.size().map(f64::from).unwrap_or(rest) / 100.0)
            .collect()
    }

    /// Check sizes in this container and below. `tab` names the tab in errors.
    pub fn validate(&self, tab: &str) -> Result<()> {
        if self.children.is_empty() {
            bail!("tab `{tab}`: a {} must not be empty", self.dir.key());
        }
        let mut sum = 0u32;
        let mut unsized_count = 0;
        for child in &self.children {
            match child.size() {
                Some(0) | Some(100..) => {
                    bail!("tab `{tab}`: size must be between 1 and 99 (percent of its parent)")
                }
                Some(s) => sum += u32::from(s),
                None => unsized_count += 1,
            }
            if let Node::Box(c) = child {
                c.validate(tab)?;
            }
        }
        if sum > 100 {
            bail!(
                "tab `{tab}`: sizes in one {} add up to {sum}, more than 100",
                self.dir.key()
            );
        }
        if sum == 100 && unsized_count > 0 {
            bail!(
                "tab `{tab}`: sizes in one {} add up to 100, leaving nothing for entries without a size",
                self.dir.key()
            );
        }
        Ok(())
    }

    pub fn to_array(&self) -> Array {
        let mut array = Array::new();
        for child in &self.children {
            array.push(node_to_value(child));
        }
        array
    }
}

fn node_to_value(node: &Node) -> Value {
    match node {
        Node::Leaf(l) if l.size.is_none() && l.mark == Mark::Managed && l.cwd.is_none() => {
            Value::from(l.name.as_str())
        }
        Node::Leaf(l) => {
            let mut t = InlineTable::new();
            t.insert("pane", Value::from(l.name.as_str()));
            if let Some(size) = l.size {
                t.insert("size", Value::from(i64::from(size)));
            }
            if l.mark != Mark::Managed {
                t.insert("mark", Value::from(l.mark.as_str()));
            }
            if let Some(cwd) = &l.cwd {
                t.insert("cwd", Value::from(cwd.as_str()));
            }
            Value::InlineTable(t)
        }
        Node::Box(c) => {
            let mut t = InlineTable::new();
            t.insert(c.dir.key(), Value::Array(c.to_array()));
            if let Some(size) = c.size {
                t.insert("size", Value::from(i64::from(size)));
            }
            Value::InlineTable(t)
        }
    }
}

/// Parse one entry of a row or column.
pub fn parse_node(value: &Value, tab: &str) -> Result<Node> {
    match value {
        Value::String(s) => Ok(Node::Leaf(Leaf::new(s.value()))),
        Value::InlineTable(t) => {
            let mut leaf_name = None;
            let mut container = None;
            let mut size = None;
            let mut mark = Mark::Managed;
            let mut cwd = None;
            for (key, v) in t.iter() {
                match key {
                    "pane" => {
                        leaf_name = Some(
                            v.as_str()
                                .ok_or_else(|| anyhow!("tab `{tab}`: `pane` must be a string"))?
                                .to_string(),
                        )
                    }
                    "row" | "column" => {
                        if container.is_some() {
                            bail!("tab `{tab}`: an entry has both `row` and `column`");
                        }
                        let dir = if key == "row" { Dir::Row } else { Dir::Column };
                        container = Some(parse_container(dir, v, tab)?);
                    }
                    "size" => {
                        let n = v
                            .as_integer()
                            .ok_or_else(|| anyhow!("tab `{tab}`: `size` must be a whole percent"))?;
                        if !(1..=99).contains(&n) {
                            bail!("tab `{tab}`: size must be between 1 and 99 (percent of its parent)");
                        }
                        size = Some(n as u8);
                    }
                    "mark" => {
                        let s = v.as_str().unwrap_or_default();
                        mark = Mark::parse(s).ok_or_else(|| {
                            anyhow!("tab `{tab}`: mark must be `mine` or `unmanaged`, not `{s}`")
                        })?;
                    }
                    "cwd" => {
                        cwd = Some(
                            v.as_str()
                                .ok_or_else(|| anyhow!("tab `{tab}`: `cwd` must be a string"))?
                                .to_string(),
                        )
                    }
                    other => bail!(
                        "tab `{tab}`: unknown key `{other}` (allowed: pane, row, column, size, mark, cwd)"
                    ),
                }
            }
            match (leaf_name, container) {
                (Some(name), None) => Ok(Node::Leaf(Leaf {
                    name,
                    size,
                    mark,
                    cwd,
                })),
                (None, Some(mut c)) => {
                    if mark != Mark::Managed || cwd.is_some() {
                        bail!(
                            "tab `{tab}`: `mark` and `cwd` belong on a pane, not a row or column"
                        );
                    }
                    c.size = size;
                    Ok(Node::Box(c))
                }
                (Some(_), Some(_)) => {
                    bail!("tab `{tab}`: an entry is both a pane and a row/column")
                }
                (None, None) => bail!("tab `{tab}`: an entry needs `pane`, `row`, or `column`"),
            }
        }
        _ => bail!("tab `{tab}`: each entry must be a pane name or an inline table"),
    }
}

pub fn parse_container(dir: Dir, value: &Value, tab: &str) -> Result<Container> {
    let array = value
        .as_array()
        .ok_or_else(|| anyhow!("tab `{tab}`: `{}` must be an array", dir.key()))?;
    let children = array
        .iter()
        .map(|v| parse_node(v, tab))
        .collect::<Result<Vec<_>>>()?;
    Ok(Container {
        dir,
        size: None,
        children,
    })
}

/// Parse the tree argument of `herdfile set`: `row = [...]`, `column = [...]`,
/// or a bare array (a row).
pub fn parse_tree_arg(arg: &str, tab: &str) -> Result<Container> {
    let trimmed = arg.trim();
    let text = if trimmed.starts_with('[') {
        format!("row = {trimmed}")
    } else {
        trimmed.to_string()
    };
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| anyhow!("cannot read the tree: {e}"))?;
    let mut found = None;
    for (key, item) in doc.iter() {
        let dir = match key {
            "row" => Dir::Row,
            "column" => Dir::Column,
            other => bail!("unknown key `{other}`: give `row = [...]` or `column = [...]`"),
        };
        let value = item
            .as_value()
            .ok_or_else(|| anyhow!("`{key}` must be an array"))?;
        if found.is_some() {
            bail!("give exactly one of `row` or `column`");
        }
        found = Some(parse_container(dir, value, tab)?);
    }
    let c = found.ok_or_else(|| anyhow!("give `row = [...]` or `column = [...]`"))?;
    c.validate(tab)?;
    Ok(c)
}

/// A normalised tree: single-child containers dissolved and same-direction
/// nesting flattened. Two layouts look the same on screen iff their normal
/// forms are equal.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Leaf(String),
    Split(Dir, Vec<(Shape, f64)>),
}

impl Shape {
    pub fn first_leaf(&self) -> &str {
        match self {
            Shape::Leaf(l) => l,
            Shape::Split(_, children) => children[0].0.first_leaf(),
        }
    }

    /// Same structure and labels, ignoring sizes.
    pub fn same_layout(&self, other: &Shape) -> bool {
        match (self, other) {
            (Shape::Leaf(a), Shape::Leaf(b)) => a == b,
            (Shape::Split(d1, c1), Shape::Split(d2, c2)) => {
                d1 == d2
                    && c1.len() == c2.len()
                    && c1.iter().zip(c2).all(|(a, b)| a.0.same_layout(&b.0))
            }
            _ => false,
        }
    }

    /// Same structure ignoring labels.
    pub fn same_shape(&self, other: &Shape) -> bool {
        match (self, other) {
            (Shape::Leaf(_), Shape::Leaf(_)) => true,
            (Shape::Split(d1, c1), Shape::Split(d2, c2)) => {
                d1 == d2
                    && c1.len() == c2.len()
                    && c1.iter().zip(c2).all(|(a, b)| a.0.same_shape(&b.0))
            }
            _ => false,
        }
    }

    pub fn leaves(&self, out: &mut Vec<String>) {
        match self {
            Shape::Leaf(l) => out.push(l.clone()),
            Shape::Split(_, c) => c.iter().for_each(|(s, _)| s.leaves(out)),
        }
    }

    /// Build a normalised shape from parts, flattening as needed.
    pub fn split(dir: Dir, parts: Vec<(Shape, f64)>) -> Shape {
        let mut flat: Vec<(Shape, f64)> = Vec::new();
        for (shape, share) in parts {
            match shape {
                Shape::Split(d, inner) if d == dir => {
                    for (s, f) in inner {
                        flat.push((s, f * share));
                    }
                }
                other => flat.push((other, share)),
            }
        }
        if flat.len() == 1 {
            flat.pop().unwrap().0
        } else {
            Shape::Split(dir, flat)
        }
    }
}

/// The normal form of a file tree, keeping only leaves `keep` accepts.
pub fn shape_of(c: &Container, keep: &dyn Fn(&Leaf) -> bool) -> Option<Shape> {
    let fractions = c.fractions();
    let mut parts = Vec::new();
    let mut kept_total = 0.0;
    for (child, frac) in c.children.iter().zip(fractions) {
        let shape = match child {
            Node::Leaf(l) => keep(l).then(|| Shape::Leaf(l.name.clone())),
            Node::Box(inner) => shape_of(inner, keep),
        };
        if let Some(shape) = shape {
            kept_total += frac;
            parts.push((shape, frac));
        }
    }
    if parts.is_empty() {
        return None;
    }
    if kept_total > 0.0 {
        for part in &mut parts {
            part.1 /= kept_total;
        }
    }
    Some(Shape::split(c.dir, parts))
}

/// Round a share (0..1) to the nearest 5 percent.
pub fn round5(share: f64) -> u8 {
    ((share * 100.0 / 5.0).round() * 5.0).clamp(5.0, 95.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(text: &str) -> Container {
        parse_tree_arg(text, "t").unwrap()
    }

    #[test]
    fn parses_nested_with_sizes() {
        let c = tree(
            r#"row = ["agent", { column = ["test", { pane = "dev", size = 30 }], size = 40 }]"#,
        );
        assert_eq!(c.dir, Dir::Row);
        assert_eq!(c.leaf_names(), ["agent", "test", "dev"]);
        let f = c.fractions();
        assert!((f[0] - 0.6).abs() < 1e-9 && (f[1] - 0.4).abs() < 1e-9);
    }

    #[test]
    fn sizes_over_100_rejected() {
        let err = parse_tree_arg(
            r#"row = [{ pane = "a", size = 60 }, { pane = "b", size = 50 }]"#,
            "main",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("tab `main`") && err.contains("110"), "{err}");
    }

    #[test]
    fn unknown_key_rejected() {
        assert!(parse_tree_arg(r#"row = [{ pane = "a", focus = true }]"#, "t").is_err());
    }

    #[test]
    fn bare_array_is_a_row() {
        assert_eq!(tree(r#"["a", "b"]"#).dir, Dir::Row);
    }

    #[test]
    fn round_trip() {
        let c = tree(
            r#"row = ["agent", { column = ["test", "dev"], size = 40 }, { pane = "s", mark = "unmanaged", cwd = "/x" }]"#,
        );
        let text = format!("row = {}", c.to_array());
        assert_eq!(tree(&text), c);
    }

    #[test]
    fn remove_collapses_single_child() {
        let mut c = tree(r#"row = ["agent", { column = ["test", "dev"], size = 40 }]"#);
        c.remove_leaf("dev").unwrap();
        assert_eq!(
            c.children[1],
            Node::Leaf(Leaf {
                size: Some(40),
                ..Leaf::new("test")
            })
        );
    }

    #[test]
    fn insert_near_wraps_on_cross_direction() {
        let mut c = tree(r#"row = ["agent", "test"]"#);
        assert!(c.insert_near(
            "test",
            Node::Leaf(Leaf::new("dev")),
            Some(Dir::Column),
            true
        ));
        assert_eq!(
            format!("{}", c.to_array()),
            r#"["agent", { column = ["test", "dev"] }]"#
        );
        assert!(c.insert_near("agent", Node::Leaf(Leaf::new("logs")), Some(Dir::Row), true));
        assert_eq!(c.leaf_names(), ["agent", "logs", "test", "dev"]);
    }

    #[test]
    fn shapes_flatten_same_direction() {
        let a = shape_of(&tree(r#"row = ["a", { row = ["b", "c"] }]"#), &|_| true).unwrap();
        let b = shape_of(&tree(r#"row = ["a", "b", "c"]"#), &|_| true).unwrap();
        assert!(a.same_layout(&b));
        let c = shape_of(&tree(r#"row = ["a", { column = ["b", "c"] }]"#), &|_| true).unwrap();
        assert!(!a.same_layout(&c));
    }

    #[test]
    fn rounding() {
        assert_eq!(round5(0.73), 75);
        assert_eq!(round5(0.61), 60);
        assert_eq!(round5(0.625), 65);
    }

    #[test]
    fn fractions_share_the_rest() {
        let c = tree(r#"row = [{ pane = "a", size = 40 }, "b", "c"]"#);
        assert_eq!(c.fractions(), [0.4, 0.3, 0.3]);
        let all = tree(r#"row = [{ pane = "a", size = 30 }, { pane = "b", size = 30 }]"#);
        assert_eq!(
            all.fractions(),
            [0.5, 0.5],
            "all sized and short of 100: scaled"
        );
    }

    #[test]
    fn sizes_must_leave_room() {
        assert!(parse_tree_arg(r#"row = [{ pane = "a", size = 100 }]"#, "t").is_err());
        assert!(parse_tree_arg(
            r#"row = [{ pane = "a", size = 60 }, { pane = "b", size = 40 }, "c"]"#,
            "t"
        )
        .is_err());
        assert!(parse_tree_arg(r#"row = []"#, "t").is_err());
        assert!(parse_tree_arg(r#"row = [{ row = ["a"], column = ["b"] }]"#, "t").is_err());
    }
}
