//! The per-workspace file: `[tab.<label>]` tables in order, each with one
//! `row` or `column` tree. Edited with `toml_edit` so only changed entries move.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use toml_edit::{DocumentMut, Item, Table, TomlError, Value};

use crate::layout::{parse_container, Container, Dir, Leaf, Mark, Node};
use crate::services::{Services, RESERVED};

#[derive(Debug, Clone)]
pub struct Tab {
    pub name: String,
    pub tree: Container,
}

#[derive(Debug, Clone)]
pub struct WorkspaceFile {
    pub path: PathBuf,
    pub doc: DocumentMut,
    /// The workspace's root folder, used to find `.herdr/services.toml`.
    pub dir: Option<String>,
    pub tabs: Vec<Tab>,
}

/// "file:line:col: message" for a TOML syntax error.
pub fn describe_toml_error(text: &str, err: &TomlError) -> String {
    match err.span() {
        Some(span) => {
            let before = &text[..span.start.min(text.len())];
            let line = before.matches('\n').count() + 1;
            let col = before.len() - before.rfind('\n').map(|i| i + 1).unwrap_or(0) + 1;
            format!("line {line}, column {col}: {}", err.message())
        }
        None => err.message().to_string(),
    }
}

impl WorkspaceFile {
    pub fn empty(path: &Path) -> WorkspaceFile {
        WorkspaceFile {
            path: path.to_path_buf(),
            doc: DocumentMut::new(),
            dir: None,
            tabs: Vec::new(),
        }
    }

    pub fn load(path: &Path) -> Result<WorkspaceFile> {
        let text = std::fs::read_to_string(path).map_err(|e| anyhow!("{}: {e}", path.display()))?;
        Self::parse(&text, path)
    }

    pub fn load_or_empty(path: &Path) -> Result<WorkspaceFile> {
        if path.exists() {
            Self::load(path)
        } else {
            Ok(Self::empty(path))
        }
    }

    /// Parse and check structure, sizes, and unique names. Names are checked
    /// against services separately ([`WorkspaceFile::check_names`]).
    pub fn parse(text: &str, path: &Path) -> Result<WorkspaceFile> {
        let shown = path.display();
        let doc: DocumentMut = text
            .parse()
            .map_err(|e: TomlError| anyhow!("{shown}: {}", describe_toml_error(text, &e)))?;
        let mut dir = None;
        let mut tabs = Vec::new();
        for (key, item) in doc.iter() {
            match key {
                "dir" => {
                    dir = Some(
                        item.as_str()
                            .ok_or_else(|| anyhow!("{shown}: `dir` must be a string"))?
                            .to_string(),
                    )
                }
                "tab" => {
                    let table = item.as_table().ok_or_else(|| {
                        anyhow!("{shown}: `tab` must hold tables like [tab.main]")
                    })?;
                    for (name, tab_item) in table.iter() {
                        tabs.push(parse_tab(name, tab_item).map_err(|e| anyhow!("{shown}: {e}"))?);
                    }
                }
                other => bail!("{shown}: unknown key `{other}` (allowed: dir, [tab.<name>])"),
            }
        }
        let file = WorkspaceFile {
            path: path.to_path_buf(),
            doc,
            dir,
            tabs,
        };
        file.check_unique().map_err(|e| anyhow!("{shown}: {e}"))?;
        Ok(file)
    }

    fn check_unique(&self) -> Result<()> {
        let mut seen: HashMap<String, String> = HashMap::new();
        for tab in &self.tabs {
            for leaf in tab.tree.leaves() {
                if let Some(first) = seen.insert(leaf.name.clone(), tab.name.clone()) {
                    if first == tab.name {
                        bail!("`{}` appears twice in tab `{}`", leaf.name, tab.name);
                    }
                    bail!(
                        "`{}` appears in tab `{first}` and in tab `{}`",
                        leaf.name,
                        tab.name
                    );
                }
            }
        }
        Ok(())
    }

    /// Every name must be a service, `agent`, or a marked pane.
    pub fn check_names(&self, services: &Services) -> Result<()> {
        for tab in &self.tabs {
            for leaf in tab.tree.leaves() {
                let known = leaf.name == RESERVED
                    || leaf.mark != Mark::Managed
                    || services.contains(&leaf.name);
                if !known {
                    let where_ = services
                        .file
                        .as_ref()
                        .map(|f| format!(" in {}", f.display()))
                        .unwrap_or_else(|| " (no .herdr/services.toml found)".to_string());
                    bail!(
                        "{}: tab `{}`: `{}` is not a service{where_}, `agent`, or a pane marked mine/unmanaged",
                        self.path.display(),
                        tab.name,
                        leaf.name
                    );
                }
            }
        }
        Ok(())
    }

    pub fn validate(&self, services: &Services) -> Result<()> {
        self.check_unique()
            .map_err(|e| anyhow!("{}: {e}", self.path.display()))?;
        for tab in &self.tabs {
            tab.tree
                .validate(&tab.name)
                .map_err(|e| anyhow!("{}: {e}", self.path.display()))?;
        }
        self.check_names(services)
    }

    pub fn tab(&self, name: &str) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.name == name)
    }

    /// Which tab holds this pane name, and the leaf itself.
    pub fn find(&self, name: &str) -> Option<(&Tab, &Leaf)> {
        self.tabs.iter().find_map(|t| {
            t.tree
                .leaves()
                .into_iter()
                .find(|l| l.name == name)
                .map(|l| (t, l))
        })
    }

    pub fn leaf_names(&self) -> Vec<String> {
        self.tabs.iter().flat_map(|t| t.tree.leaf_names()).collect()
    }

    fn tab_mut(&mut self, name: &str) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.name == name)
    }

    /// Write one tab's tree back into the document, touching only that tab.
    fn sync_tab(&mut self, name: &str) {
        // `row = [{ column = [...] }]` reads as `column = [...]`.
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.name == name) {
            while let [Node::Box(inner)] = tab.tree.children.as_slice() {
                let mut inner = inner.clone();
                inner.size = None;
                tab.tree = inner;
            }
        }
        let tree = self
            .tabs
            .iter()
            .find(|t| t.name == name)
            .map(|t| t.tree.clone());
        let tabs_table = self.tabs_table();
        match tree {
            Some(tree) if !tree.children.is_empty() => {
                if !tabs_table.contains_key(name) {
                    tabs_table.insert(name, Item::Table(Table::new()));
                }
                let table = tabs_table[name].as_table_mut().expect("tab is a table");
                let key = tree.dir.key();
                let other = match tree.dir {
                    Dir::Row => "column",
                    Dir::Column => "row",
                };
                table.remove(other);
                let new_value = Value::Array(tree.to_array());
                let unchanged = table
                    .get(key)
                    .and_then(Item::as_value)
                    .map(|v| v.to_string().trim() == new_value.to_string().trim())
                    .unwrap_or(false);
                if !unchanged {
                    // Keep the key's own decoration (comments) if it existed.
                    match table.get_mut(key).and_then(Item::as_value_mut) {
                        Some(v) => {
                            let decor = v.decor().clone();
                            *v = new_value;
                            *v.decor_mut() = decor;
                        }
                        None => {
                            table.insert(key, toml_edit::value(new_value));
                        }
                    }
                }
            }
            _ => {
                tabs_table.remove(name);
                self.tabs.retain(|t| t.name != name);
            }
        }
    }

    fn tabs_table(&mut self) -> &mut Table {
        if !self.doc.contains_key("tab") {
            let mut t = Table::new();
            t.set_implicit(true);
            self.doc.insert("tab", Item::Table(t));
        }
        self.doc["tab"].as_table_mut().expect("tab is a table")
    }

    pub fn set_dir(&mut self, dir: &str) {
        self.dir = Some(dir.to_string());
        self.doc.insert("dir", toml_edit::value(dir));
    }

    /// Replace one tab's tree (creating the tab at the end if missing).
    pub fn set_tab(&mut self, name: &str, tree: Container) {
        match self.tab_mut(name) {
            Some(tab) => tab.tree = tree,
            None => self.tabs.push(Tab {
                name: name.to_string(),
                tree,
            }),
        }
        self.sync_tab(name);
    }

    /// Remove a pane from wherever it is. Returns the tab it was in and the leaf.
    pub fn remove(&mut self, name: &str) -> Option<(String, Leaf)> {
        let tab = self.find(name)?.0.name.clone();
        let leaf = self.tab_mut(&tab)?.tree.remove_leaf(name)?;
        self.sync_tab(&tab);
        Some((tab, leaf))
    }

    /// Add a leaf next to `anchor`, or at the end of `tab`'s top container.
    pub fn insert(
        &mut self,
        tab: &str,
        leaf: Leaf,
        anchor: Option<(&str, Option<Dir>, bool)>,
    ) -> Result<()> {
        if let Some((anchor_name, dir, after)) = anchor {
            let anchor_tab = self
                .find(anchor_name)
                .map(|(t, _)| t.name.clone())
                .ok_or_else(|| anyhow!("`{anchor_name}` is not in the file"))?;
            let t = self.tab_mut(&anchor_tab).expect("found above");
            t.tree
                .insert_near(anchor_name, crate::layout::Node::Leaf(leaf), dir, after);
            self.sync_tab(&anchor_tab);
            return Ok(());
        }
        match self.tab_mut(tab) {
            Some(t) => t.tree.children.push(crate::layout::Node::Leaf(leaf)),
            None => self.tabs.push(Tab {
                name: tab.to_string(),
                tree: Container {
                    dir: Dir::Row,
                    size: None,
                    children: vec![crate::layout::Node::Leaf(leaf)],
                },
            }),
        }
        self.sync_tab(tab);
        Ok(())
    }

    /// Change one leaf in place (size, mark, cwd).
    pub fn update_leaf(&mut self, name: &str, f: impl FnOnce(&mut Leaf)) -> bool {
        let Some(tab) = self.find(name).map(|(t, _)| t.name.clone()) else {
            return false;
        };
        if let Some(leaf) = self.tab_mut(&tab).and_then(|t| t.tree.find_leaf_mut(name)) {
            f(leaf);
        }
        self.sync_tab(&tab);
        true
    }

    /// Replace a tab's tree only if it changed; for write-back of sizes.
    pub fn replace_tree_if_changed(&mut self, name: &str, tree: Container) -> bool {
        match self.tab(name) {
            Some(t) if t.tree == tree => false,
            _ => {
                self.set_tab(name, tree);
                true
            }
        }
    }

    pub fn to_text(&self) -> String {
        self.doc.to_string()
    }

    pub fn save(&self) -> Result<()> {
        crate::paths::write_atomic(&self.path, &self.to_text())
            .map_err(|e| anyhow!("{}: {e}", self.path.display()))
    }
}

fn parse_tab(name: &str, item: &Item) -> Result<Tab> {
    let table = item
        .as_table_like()
        .ok_or_else(|| anyhow!("[tab.{name}] must be a table"))?;
    let mut tree = None;
    for (key, value) in table.iter() {
        let dir = match key {
            "row" => Dir::Row,
            "column" => Dir::Column,
            other => bail!("tab `{name}`: unknown key `{other}` (give one `row` or `column`)"),
        };
        if tree.is_some() {
            bail!("tab `{name}`: give exactly one of `row` or `column`");
        }
        let value = value
            .as_value()
            .ok_or_else(|| anyhow!("tab `{name}`: `{key}` must be an array"))?;
        tree = Some(parse_container(dir, value, name)?);
    }
    let tree = tree.ok_or_else(|| anyhow!("tab `{name}`: give one `row` or `column`"))?;
    tree.validate(name)?;
    Ok(Tab {
        name: name.to_string(),
        tree,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn file(text: &str) -> Result<WorkspaceFile> {
        WorkspaceFile::parse(text, Path::new("/s/w3.toml"))
    }

    fn services(names: &[&str]) -> Services {
        let mut map = BTreeMap::new();
        for n in names {
            map.insert(
                n.to_string(),
                crate::services::Service {
                    name: n.to_string(),
                    cmd: "true".into(),
                    cwd: "/".into(),
                    env: BTreeMap::new(),
                    ready: None,
                },
            );
        }
        Services {
            file: Some("/r/.herdr/services.toml".into()),
            map,
        }
    }

    #[test]
    fn two_tabs_in_order() {
        let f =
            file("[tab.main]\nrow = [\"agent\", \"test\"]\n\n[tab.services]\nrow = [\"dev\"]\n")
                .unwrap();
        assert_eq!(f.tabs.len(), 2);
        assert_eq!(f.tabs[0].name, "main");
        assert_eq!(f.tabs[0].tree.leaf_names(), ["agent", "test"]);
        assert_eq!(f.tabs[1].name, "services");
        f.validate(&services(&["dev", "test"])).unwrap();
    }

    #[test]
    fn unknown_name_rejected() {
        let f = file("[tab.main]\nrow = [\"agent\", \"nope\"]\n").unwrap();
        let err = f.validate(&services(&["dev"])).unwrap_err().to_string();
        assert!(err.contains("`nope`"), "{err}");
    }

    #[test]
    fn marked_panes_are_known() {
        let f = file("[tab.main]\nrow = [\"agent\", { pane = \"shell-1\", mark = \"mine\" }]\n")
            .unwrap();
        f.validate(&services(&[])).unwrap();
    }

    #[test]
    fn duplicate_names_both_tabs() {
        let err = file("[tab.a]\nrow = [\"dev\"]\n[tab.b]\nrow = [\"dev\"]\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("tab `a`") && err.contains("tab `b`"), "{err}");
    }

    #[test]
    fn syntax_error_has_line() {
        let err = file("[tab.main]\nrow = [\"agent\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("/s/w3.toml") && err.contains("line"), "{err}");
    }

    #[test]
    fn sizes_over_100_name_tab() {
        let err =
            file("[tab.main]\nrow = [{ pane = \"a\", size = 70 }, { pane = \"b\", size = 40 }]\n")
                .unwrap_err()
                .to_string();
        assert!(err.contains("tab `main`"), "{err}");
    }

    #[test]
    fn remove_changes_one_line() {
        let text = "# my layout\ndir = \"/r\"\n\n[tab.main]\nrow = [\"agent\", \"test\"]  # keep\n\n[tab.services]\nrow = [\"dev\"]\n";
        let mut f = file(text).unwrap();
        f.remove("test").unwrap();
        let out = f.to_text();
        let changed: Vec<_> = text
            .lines()
            .zip(out.lines())
            .filter(|(a, b)| a != b)
            .collect();
        assert_eq!(changed.len(), 1, "{out}");
        assert!(out.contains("row = [\"agent\"]  # keep"), "{out}");
    }

    #[test]
    fn single_nested_container_is_hoisted() {
        let mut f = file("[tab.main]\nrow = [\"a\", { column = [\"b\", \"c\"] }]\n").unwrap();
        f.remove("a").unwrap();
        assert_eq!(f.to_text(), "[tab.main]\ncolumn = [\"b\", \"c\"]\n");
    }

    #[test]
    fn removing_last_pane_drops_tab() {
        let mut f =
            file("[tab.main]\nrow = [\"agent\"]\n\n[tab.services]\nrow = [\"dev\"]\n").unwrap();
        f.remove("dev").unwrap();
        assert!(f.tab("services").is_none());
        assert!(!f.to_text().contains("services"));
    }

    #[test]
    fn insert_creates_tab_at_end() {
        let mut f = file("[tab.main]\nrow = [\"agent\"]\n").unwrap();
        f.insert("services", Leaf::new("dev"), None).unwrap();
        assert_eq!(
            f.to_text(),
            "[tab.main]\nrow = [\"agent\"]\n\n[tab.services]\nrow = [\"dev\"]\n"
        );
        let again = WorkspaceFile::parse(&f.to_text(), &f.path).unwrap();
        assert_eq!(again.tabs[1].tree.leaf_names(), ["dev"]);
    }
}
