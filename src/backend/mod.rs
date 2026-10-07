//! Everything herdfile asks of the multiplexer goes through [`Backend`].
//! herdr's socket API is the only implementation.

pub mod herdr;

use anyhow::Result;
use serde::Deserialize;

use crate::layout::Dir;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Snapshot {
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub tabs: Vec<Tab>,
    #[serde(default)]
    pub panes: Vec<Pane>,
    #[serde(default)]
    pub layouts: Vec<Layout>,
    #[serde(default)]
    pub focused_tab_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Workspace {
    pub workspace_id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub worktree: Option<WorktreeInfo>,
}

/// Where a workspace's checkout is, as herdr reports it.
#[derive(Debug, Clone, Deserialize)]
pub struct WorktreeInfo {
    pub checkout_path: String,
    #[serde(default)]
    pub is_linked_worktree: bool,
    #[serde(default)]
    pub repo_root: Option<String>,
}

impl Workspace {
    /// The checkout, when this workspace is a linked git worktree.
    pub fn linked_worktree(&self) -> Option<&WorktreeInfo> {
        self.worktree.as_ref().filter(|w| w.is_linked_worktree)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tab {
    pub tab_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub number: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Pane {
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub agent_status: Option<String>,
}

impl Pane {
    /// Working or blocked: never closed.
    pub fn busy(&self) -> bool {
        matches!(self.agent_status.as_deref(), Some("working" | "blocked"))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Layout {
    pub tab_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub panes: Vec<LayoutPane>,
    #[serde(default)]
    pub splits: Vec<LayoutSplit>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LayoutPane {
    pub pane_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LayoutSplit {
    pub id: String,
    pub direction: String,
    pub ratio: f64,
}

impl Snapshot {
    pub fn workspace(&self, id: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.workspace_id == id)
    }

    pub fn workspace_by_label(&self, label: &str) -> Option<&Workspace> {
        self.workspaces
            .iter()
            .find(|w| w.label.as_deref() == Some(label))
    }

    pub fn tabs_of<'a>(&'a self, workspace: &'a str) -> impl Iterator<Item = &'a Tab> + 'a {
        self.tabs
            .iter()
            .filter(move |t| t.workspace_id == workspace)
    }

    pub fn panes_of<'a>(&'a self, workspace: &'a str) -> impl Iterator<Item = &'a Pane> + 'a {
        self.panes
            .iter()
            .filter(move |p| p.workspace_id == workspace)
    }

    pub fn pane(&self, id: &str) -> Option<&Pane> {
        self.panes.iter().find(|p| p.pane_id == id)
    }

    pub fn pane_by_label(&self, workspace: &str, label: &str) -> Option<&Pane> {
        self.panes
            .iter()
            .find(|p| p.workspace_id == workspace && p.label.as_deref() == Some(label))
    }

    pub fn tab_by_label(&self, workspace: &str, label: &str) -> Option<&Tab> {
        self.tabs
            .iter()
            .find(|t| t.workspace_id == workspace && t.label.as_deref() == Some(label))
    }

    pub fn layout(&self, tab: &str) -> Option<&Layout> {
        self.layouts.iter().find(|l| l.tab_id == tab)
    }

    /// The tab the operator is looking at.
    pub fn viewed_tab(&self) -> Option<&str> {
        self.focused_tab_id.as_deref()
    }
}

/// Where to put a pane relative to another.
#[derive(Debug, Clone)]
pub struct Placement<'a> {
    pub target_pane: &'a str,
    pub dir: Dir,
    /// The target's share after the split (herdr's first-child ratio).
    pub ratio: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct Spawn<'a> {
    pub cwd: Option<&'a str>,
    pub env: Vec<(String, String)>,
}

/// A change event; only its kind and any ids are used.
#[derive(Debug, Clone)]
pub struct Event {
    pub kind: String,
    pub data: serde_json::Value,
}

impl Event {
    /// The workspace this event is about, if it says.
    pub fn workspace_id(&self) -> Option<String> {
        fn find(v: &serde_json::Value) -> Option<String> {
            match v {
                serde_json::Value::Object(map) => {
                    if let Some(w) = map.get("workspace_id").and_then(|w| w.as_str()) {
                        return Some(w.to_string());
                    }
                    for key in ["pane_id", "tab_id"] {
                        if let Some(id) = map.get(key).and_then(|w| w.as_str()) {
                            if let Some((ws, _)) = id.split_once(':') {
                                return Some(ws.to_string());
                            }
                        }
                    }
                    map.values().find_map(find)
                }
                _ => None,
            }
        }
        find(&self.data)
    }
}

pub trait Backend {
    fn snapshot(&self) -> Result<Snapshot>;
    /// Blocking stream of events. Returns when the connection ends.
    fn subscribe(&self, on_event: &mut dyn FnMut(Event) -> bool) -> Result<()>;

    fn create_workspace(&self, label: &str, cwd: &str) -> Result<(String, String, String)>;
    fn close_workspace(&self, workspace: &str) -> Result<()>;
    /// Returns (tab id, root pane id).
    fn create_tab(&self, workspace: &str, label: &str, spawn: &Spawn) -> Result<(String, String)>;
    fn rename_tab(&self, tab: &str, label: &str) -> Result<()>;
    fn move_tab(&self, tab: &str, index: usize) -> Result<()>;
    /// Returns the new pane id.
    fn split(&self, at: &Placement, spawn: &Spawn) -> Result<String>;
    /// Move a pane into another tab next to a target. Returns its id afterwards.
    fn move_pane(&self, pane: &str, tab: &str, at: &Placement) -> Result<String>;
    /// Move a pane into a new tab. Returns (pane id, tab id).
    fn move_pane_new_tab(
        &self,
        pane: &str,
        workspace: &str,
        label: &str,
    ) -> Result<(String, String)>;
    fn swap(&self, a: &str, b: &str) -> Result<()>;
    fn close_pane(&self, pane: &str) -> Result<()>;
    fn rename_pane(&self, pane: &str, label: Option<&str>) -> Result<()>;
    /// Type a command at the pane's shell and press Enter.
    fn run(&self, pane: &str, command: &str) -> Result<()>;
    fn set_ratio(&self, tab: &str, path: &[bool], ratio: f64) -> Result<()>;

    /// Run a herdr CLI command against this server, returning
    /// (exit code, stdout, stderr). Used where the CLI adds behaviour
    /// (agent start/prompt/wait/read, worktrees).
    fn cli(&self, args: &[&str]) -> Result<(i32, String, String)>;
}
