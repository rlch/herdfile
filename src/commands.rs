//! Commands that change a workspace file: each takes the workspace lock, edits
//! the current file, then asks the watcher to apply and waits.

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};

use crate::apply::{load_services, Report};
use crate::backend::herdr::Herdr;
use crate::backend::Backend;
use crate::control::{self, Reply, Request};
use crate::layout::{Container, Dir, Leaf, Mark, Node};
use crate::lock::FileLock;
use crate::paths;
use crate::wsfile::WorkspaceFile;

/// The workspace a command is about: the flag, else `$HERDR_WORKSPACE_ID`.
pub fn resolve_workspace(flag: Option<String>) -> Result<String> {
    flag.or_else(|| {
        std::env::var("HERDR_WORKSPACE_ID")
            .ok()
            .filter(|s| !s.is_empty())
    })
    .ok_or_else(|| anyhow!("no workspace: run inside a herdr pane or pass --workspace <id>"))
}

#[derive(Debug, Clone, Copy)]
pub enum Anchor {
    RightOf,
    LeftOf,
    Above,
    Below,
    After,
    Before,
}

impl Anchor {
    fn dir_after(self) -> (Option<Dir>, bool) {
        match self {
            Anchor::RightOf => (Some(Dir::Row), true),
            Anchor::LeftOf => (Some(Dir::Row), false),
            Anchor::Below => (Some(Dir::Column), true),
            Anchor::Above => (Some(Dir::Column), false),
            Anchor::After => (None, true),
            Anchor::Before => (None, false),
        }
    }
}

pub struct Wait {
    pub no_wait: bool,
}

/// Lock, load, edit, validate, save. Returns what the edit touched.
fn edit(
    backend: &dyn Backend,
    ws: &str,
    f: impl FnOnce(&mut WorkspaceFile) -> Result<Vec<String>>,
) -> Result<Vec<String>> {
    paths::ensure_state_dir()?;
    let path = paths::workspace_file(ws);
    let _lock = FileLock::acquire(&paths::lock_for(&path))?;
    let mut file = WorkspaceFile::load_or_empty(&path)?;
    let touched = f(&mut file)?;
    let snap = backend.snapshot()?;
    let services = load_services(&file, &snap, ws)?;
    file.validate(&services)?;
    file.save()?;
    Ok(touched)
}

/// Ask the watcher to apply now and report. Exits non-zero (via `Err`) when
/// apply fails or a change was dropped.
fn finish(backend: &dyn Backend, ws: &str, touched: &[String], wait: &Wait) -> Result<()> {
    // The lock, not a ping: a busy watcher still answers, just later.
    if !crate::lock::is_held(&paths::watch_lock()) {
        eprintln!(
            "herdfile: warning: the watcher is not running, so the edit will not be applied until it starts"
        );
        return Ok(());
    }
    if wait.no_wait {
        println!("edited {}", paths::workspace_file(ws).display());
        return Ok(());
    }
    let reply = control::send(
        &Request::Apply {
            workspace: ws.to_string(),
        },
        Duration::from_secs(120),
    )?;
    report_reply(backend, &reply, touched)
}

pub fn report_reply(backend: &dyn Backend, reply: &Reply, touched: &[String]) -> Result<()> {
    if !reply.ok {
        bail!(
            "{}",
            reply.error.clone().unwrap_or_else(|| "apply failed".into())
        );
    }
    let report = reply.report.clone().unwrap_or_default();
    print_report(&report);
    wait_ready(backend, &report);
    let dropped: Vec<&String> = touched
        .iter()
        .filter(|t| report.writeback.dropped.contains(t))
        .collect();
    if !dropped.is_empty() {
        bail!(
            "your change was dropped: {} was closed by hand before it applied, and hand changes win",
            dropped.iter().map(|s| format!("`{s}`")).collect::<Vec<_>>().join(", ")
        );
    }
    if !reply.problems.is_empty() {
        bail!(
            "herdr does not match the file yet: {}",
            reply.problems.join("; ")
        );
    }
    Ok(())
}

fn print_report(report: &Report) {
    let lines = report.summary();
    if lines.is_empty() {
        println!("herdr already matches");
    }
    for line in lines {
        println!("{line}");
    }
}

/// For services with readiness text, wait until it shows.
fn wait_ready(backend: &dyn Backend, report: &Report) {
    for (name, pane, ready) in &report.opened_ready {
        let result = backend.cli(&[
            "pane",
            "wait-output",
            pane,
            "--match",
            ready,
            "--timeout",
            "60000",
        ]);
        match result {
            Ok((0, _, _)) => println!("{name} is ready"),
            _ => eprintln!("herdfile: `{name}` did not print `{ready}` within 60s"),
        }
    }
}

pub fn place(
    ws: &str,
    name: &str,
    tab: Option<String>,
    anchor: Option<(Anchor, String)>,
    size: Option<u8>,
    wait: Wait,
) -> Result<()> {
    let backend = Herdr::from_env();
    if let Some((_, a)) = &anchor {
        if a == name {
            bail!("cannot place `{name}` next to itself");
        }
    }
    let touched = edit(&backend, ws, |file| {
        let old = file.remove(name);
        let mut leaf = old
            .as_ref()
            .map(|(_, l)| l.clone())
            .unwrap_or_else(|| Leaf::new(name));
        if size.is_some() {
            leaf.size = size;
        }
        match &anchor {
            Some((kind, anchor_name)) => {
                let (anchor_tab, _) = file
                    .find(anchor_name)
                    .ok_or_else(|| anyhow!("`{anchor_name}` is not in the file"))?;
                if let Some(t) = &tab {
                    if &anchor_tab.name != t {
                        bail!("`{anchor_name}` is in tab `{}`, not `{t}`", anchor_tab.name);
                    }
                }
                let anchor_tab = anchor_tab.name.clone();
                let (dir, after) = kind.dir_after();
                file.insert(&anchor_tab, leaf, Some((anchor_name, dir, after)))?;
            }
            None => {
                let target = tab
                    .clone()
                    .or_else(|| old.as_ref().map(|(t, _)| t.clone()))
                    .or_else(|| file.tabs.first().map(|t| t.name.clone()))
                    .unwrap_or_else(|| "main".to_string());
                file.insert(&target, leaf, None)?;
            }
        }
        Ok(vec![name.to_string()])
    })?;
    finish(&backend, ws, &touched, &wait)
}

pub fn remove(ws: &str, name: &str, wait: Wait) -> Result<()> {
    let backend = Herdr::from_env();
    let touched = edit(&backend, ws, |file| {
        file.remove(name)
            .ok_or_else(|| anyhow!("`{name}` is not in the file"))?;
        Ok(vec![name.to_string()])
    })?;
    finish(&backend, ws, &touched, &wait)
}

pub fn set(ws: &str, tab: &str, tree_arg: &str, wait: Wait) -> Result<()> {
    let backend = Herdr::from_env();
    let mut tree = crate::layout::parse_tree_arg(tree_arg, tab)?;
    let touched = edit(&backend, ws, |file| {
        let names: Vec<String> = tree.leaf_names();
        // Names listed elsewhere move here, keeping their marks.
        for n in &names {
            let elsewhere = file.find(n).map(|(t, l)| (t.name.clone(), l.clone()));
            if let Some((t, old)) = elsewhere {
                if t != tab {
                    file.remove(n);
                }
                if let Some(leaf) = tree.find_leaf_mut(n) {
                    if leaf.mark == Mark::Managed && old.mark != Mark::Managed {
                        leaf.mark = old.mark;
                        leaf.cwd = old.cwd.clone();
                    }
                }
            }
        }
        let old_names: HashSet<String> = file
            .tab(tab)
            .map(|t| t.tree.leaf_names().into_iter().collect())
            .unwrap_or_default();
        file.set_tab(tab, tree.clone());
        let mut touched: Vec<String> = names;
        touched.extend(old_names);
        Ok(touched)
    })?;
    finish(&backend, ws, &touched, &wait)
}

pub fn mark(ws: &str, name: &str, mark: Mark, wait: Wait) -> Result<()> {
    let backend = Herdr::from_env();
    let touched = edit(&backend, ws, |file| {
        let cwd = backend
            .snapshot()
            .ok()
            .and_then(|s| s.pane_by_label(ws, name).and_then(|p| p.cwd.clone()));
        if !file.update_leaf(name, |l| {
            l.mark = mark;
            l.cwd = if mark == Mark::Managed {
                None
            } else {
                l.cwd.clone().or(cwd)
            };
        }) {
            bail!("`{name}` is not in the file");
        }
        Ok(vec![name.to_string()])
    })?;
    finish(&backend, ws, &touched, &wait)
}

pub fn show(ws: &str) -> Result<()> {
    let path = paths::workspace_file(ws);
    match std::fs::read_to_string(&path) {
        Ok(text) => print!("{text}"),
        Err(_) => println!("# no workspace file yet at {}", path.display()),
    }
    Ok(())
}

pub fn path(ws: &str) -> Result<()> {
    println!("{}", paths::workspace_file(ws).display());
    control::warn_if_no_watcher();
    Ok(())
}

/// `herdfile apply`: through the watcher if running, else in this process.
pub fn apply_now(ws: &str) -> Result<()> {
    let backend = Herdr::from_env();
    let reply = if control::watcher_pid().is_some() {
        control::send(
            &Request::Apply {
                workspace: ws.to_string(),
            },
            Duration::from_secs(120),
        )?
    } else {
        crate::watch::apply_once(&backend, ws)?
    };
    report_reply(&backend, &reply, &[])
}

pub fn status() -> Result<()> {
    match control::watcher_pid() {
        Some(pid) => println!("watcher running (pid {pid})"),
        None => println!("watcher not running"),
    }
    println!("state: {}", paths::state_dir().display());
    println!("log:   {}", paths::watch_log().display());
    println!(
        "herdr: {}",
        crate::backend::herdr::default_socket().display()
    );
    Ok(())
}

/// Build a file tree from a live tab, with sizes to the whole percent when
/// panes are not shared equally.
pub fn tree_from_live(shape: &crate::layout::Shape, leaf: &dyn Fn(&str) -> Leaf) -> Container {
    use crate::layout::Shape;
    match shape {
        Shape::Leaf(name) => Container {
            dir: Dir::Row,
            size: None,
            children: vec![Node::Leaf(leaf(name))],
        },
        Shape::Split(dir, children) => {
            let n = children.len() as f64;
            let equal = children.iter().all(|(_, s)| (s - 1.0 / n).abs() < 0.01);
            let mut out = Vec::new();
            for (i, (child, share)) in children.iter().enumerate() {
                let size = if equal || i + 1 == children.len() {
                    None
                } else {
                    Some(((share * 100.0).round() as u8).clamp(1, 99))
                };
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
