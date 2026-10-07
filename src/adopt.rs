//! `herdfile adopt`: write a workspace file from what is on screen.

use std::collections::HashSet;

use anyhow::{bail, Result};

use crate::apply::workspace_dir;
use crate::backend::herdr::Herdr;
use crate::backend::Backend;
use crate::commands::tree_from_live;
use crate::layout::{Leaf, Mark};
use crate::live::LiveNode;
use crate::lock::FileLock;
use crate::paths;
use crate::services::{Services, RESERVED};
use crate::writeback::label_unlabelled;
use crate::wsfile::WorkspaceFile;

pub fn adopt(ws: &str, force: bool) -> Result<()> {
    let backend = Herdr::from_env();
    let path = adopt_with(&backend, ws, force)?;
    println!("wrote {}", path.display());
    crate::control::warn_if_no_watcher();
    Ok(())
}

pub fn adopt_with(backend: &dyn Backend, ws: &str, force: bool) -> Result<std::path::PathBuf> {
    paths::ensure_state_dir()?;
    let path = paths::workspace_file(ws);
    let _lock = FileLock::acquire(&paths::lock_for(&path))?;
    if path.exists() && !force {
        bail!(
            "{} already exists; pass --force to overwrite it",
            path.display()
        );
    }
    let mut snap = backend.snapshot()?;
    if snap.workspace(ws).is_none() {
        bail!("workspace {ws} is not open in herdr");
    }

    // The workspace's own agent: a pane already labelled `agent`, else the
    // first unlabelled pane hosting an agent.
    if snap.pane_by_label(ws, RESERVED).is_none() {
        let mut tabs: Vec<_> = snap.tabs_of(ws).cloned().collect();
        tabs.sort_by_key(|t| t.number);
        let first_agent = tabs.iter().find_map(|t| {
            let tree = LiveNode::of_tab(&snap, &t.tab_id)?;
            tree.leaves().into_iter().find_map(|(id, label)| {
                let p = snap.pane(&id)?;
                (label.is_none() && p.agent.is_some()).then(|| id.clone())
            })
        });
        if let Some(id) = first_agent {
            backend.rename_pane(&id, Some(RESERVED))?;
            if let Some(p) = snap.panes.iter_mut().find(|p| p.pane_id == id) {
                p.label = Some(RESERVED.to_string());
            }
        }
    }
    label_unlabelled(backend, &mut snap, ws, &HashSet::new())?;

    let mut file = WorkspaceFile::empty(&path);
    let dir = workspace_dir(&file, &snap, ws);
    let services = match &dir {
        Some(d) => Services::discover(d)?,
        None => Services::default(),
    };
    if let Some(d) = &dir {
        file.set_dir(&d.to_string_lossy());
    }
    let mut tabs: Vec<_> = snap.tabs_of(ws).cloned().collect();
    tabs.sort_by_key(|t| t.number);
    let mut tab_names = HashSet::new();
    for tab in tabs {
        let Some(tree) = LiveNode::of_tab(&snap, &tab.tab_id) else {
            continue;
        };
        let mut name = tab.label.clone().unwrap_or_else(|| tab.number.to_string());
        if !tab_names.insert(name.clone()) {
            // Tab labels need not be unique in herdr; the file's must be.
            name = format!("{name}-{}", tab.number);
            backend.rename_tab(&tab.tab_id, &name)?;
            tab_names.insert(name.clone());
        }
        let leaf_for = |label: &str| {
            if label == RESERVED || services.contains(label) {
                Leaf::new(label)
            } else {
                let cwd = snap.pane_by_label(ws, label).and_then(|p| p.cwd.clone());
                Leaf {
                    name: label.to_string(),
                    size: None,
                    mark: Mark::Unmanaged,
                    cwd,
                }
            }
        };
        let mut container = tree_from_live(&tree.shape(), &leaf_for);
        if container.validate(&name).is_err() {
            strip_sizes(&mut container);
        }
        file.set_tab(&name, container);
    }
    file.validate(&services)?;
    file.save()?;
    Ok(path)
}

fn strip_sizes(c: &mut crate::layout::Container) {
    for child in &mut c.children {
        child.set_size(None);
        if let crate::layout::Node::Box(inner) = child {
            strip_sizes(inner);
        }
    }
}
