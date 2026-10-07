//! The file of workspaces: one entry per herdr workspace, keyed by its name
//! (its herdr label), forming a tree through `parent`.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use toml_edit::{DocumentMut, InlineTable, Item, Table};

use crate::backend::herdr::Herdr;
use crate::backend::{Backend, Snapshot};
use crate::lock::FileLock;
use crate::paths;

pub const OPERATOR: &str = "operator";
pub const NAME_PATTERN: &str = "[a-z][a-z0-9_-]{0,31}";

/// herdr's agent-name rule, which workspace names follow so that
/// `herdr agent prompt <workspace>` works.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('a'..='z'))
        && name.len() <= 32
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_' | '-'))
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentSpec {
    pub brief: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    pub name: String,
    pub dir: String,
    pub branch: Option<String>,
    pub base: Option<String>,
    pub purpose: Option<String>,
    pub parent: Option<String>,
    pub agent: Option<AgentSpec>,
    /// Opened outside herdfile: never removed automatically.
    pub unmanaged: bool,
    /// `ws remove` was asked for; waiting for idle or a merge.
    pub removing: bool,
}

impl Entry {
    pub fn parent(&self) -> &str {
        self.parent.as_deref().unwrap_or(OPERATOR)
    }
}

pub struct WorkspacesFile {
    pub path: PathBuf,
    pub doc: DocumentMut,
    pub entries: Vec<Entry>,
}

fn opt_str(t: &dyn toml_edit::TableLike, key: &str, name: &str) -> Result<Option<String>> {
    match t.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_str()
            .map(|s| Some(s.to_string()))
            .ok_or_else(|| anyhow!("[{name}]: `{key}` must be a string")),
    }
}

impl WorkspacesFile {
    pub fn load(path: &Path) -> Result<WorkspacesFile> {
        let text = if path.exists() {
            std::fs::read_to_string(path).map_err(|e| anyhow!("{}: {e}", path.display()))?
        } else {
            String::new()
        };
        Self::parse(&text, path)
    }

    pub fn parse(text: &str, path: &Path) -> Result<WorkspacesFile> {
        let shown = path.display();
        let doc: DocumentMut = text
            .parse()
            .map_err(|e| anyhow!("{shown}: {}", crate::wsfile::describe_toml_error(text, &e)))?;
        let mut entries = Vec::new();
        for (name, item) in doc.iter() {
            let t = item
                .as_table_like()
                .ok_or_else(|| anyhow!("{shown}: `{name}` must be a table like [{name}]"))?;
            for (key, _) in t.iter() {
                if !matches!(
                    key,
                    "dir"
                        | "branch"
                        | "base"
                        | "purpose"
                        | "parent"
                        | "agent"
                        | "unmanaged"
                        | "removing"
                ) {
                    bail!("{shown}: [{name}]: unknown key `{key}`");
                }
            }
            let dir = opt_str(t, "dir", name)
                .map_err(|e| anyhow!("{shown}: {e}"))?
                .ok_or_else(|| anyhow!("{shown}: [{name}] has no `dir`"))?;
            let agent = match t.get("agent") {
                None => None,
                Some(a) => {
                    let a = a
                        .as_table_like()
                        .ok_or_else(|| anyhow!("{shown}: [{name}]: `agent` must be a table"))?;
                    for (key, _) in a.iter() {
                        if !matches!(key, "brief" | "model") {
                            bail!("{shown}: [{name}].agent: unknown key `{key}`");
                        }
                    }
                    Some(AgentSpec {
                        brief: opt_str(a, "brief", name)?,
                        model: opt_str(a, "model", name)?,
                    })
                }
            };
            let flag = |key: &str| t.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
            let entry = Entry {
                name: name.to_string(),
                dir,
                branch: opt_str(t, "branch", name)?,
                base: opt_str(t, "base", name)?,
                purpose: opt_str(t, "purpose", name)?,
                parent: opt_str(t, "parent", name)?,
                agent,
                unmanaged: flag("unmanaged"),
                removing: flag("removing"),
            };
            if !entry.unmanaged && !valid_name(name) {
                bail!("{shown}: `{name}` is not a valid name: use {NAME_PATTERN}");
            }
            entries.push(entry);
        }
        let file = WorkspacesFile {
            path: path.to_path_buf(),
            doc,
            entries,
        };
        file.check_tree().map_err(|e| anyhow!("{shown}: {e}"))?;
        Ok(file)
    }

    /// Parents must exist, and following them must reach the operator.
    pub fn check_tree(&self) -> Result<()> {
        let names: HashSet<&str> = self.entries.iter().map(|e| e.name.as_str()).collect();
        for e in &self.entries {
            let p = e.parent();
            if p != OPERATOR && !names.contains(p) {
                bail!("[{}]: parent `{p}` is not a workspace in the file", e.name);
            }
        }
        for e in &self.entries {
            let mut seen = HashSet::new();
            let mut cur = e.name.as_str();
            while cur != OPERATOR {
                if !seen.insert(cur) {
                    bail!("[{}]: parents form a cycle through `{cur}`", e.name);
                }
                cur = self.get(cur).map(|x| x.parent()).unwrap_or(OPERATOR);
            }
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    pub fn names(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.name.clone()).collect()
    }

    pub fn add(&mut self, entry: Entry) {
        let mut t = Table::new();
        t.insert("dir", toml_edit::value(entry.dir.as_str()));
        for (key, v) in [
            ("branch", &entry.branch),
            ("base", &entry.base),
            ("purpose", &entry.purpose),
            ("parent", &entry.parent),
        ] {
            if let Some(v) = v {
                t.insert(key, toml_edit::value(v.as_str()));
            }
        }
        if let Some(a) = &entry.agent {
            let mut it = InlineTable::new();
            if let Some(b) = &a.brief {
                it.insert("brief", b.as_str().into());
            }
            if let Some(m) = &a.model {
                it.insert("model", m.as_str().into());
            }
            t.insert("agent", toml_edit::value(it));
        }
        if entry.unmanaged {
            t.insert("unmanaged", toml_edit::value(true));
        }
        self.doc.insert(&entry.name, Item::Table(t));
        self.entries.push(entry);
    }

    pub fn set_removing(&mut self, name: &str) {
        if let Some(t) = self.doc.get_mut(name).and_then(Item::as_table_mut) {
            t.insert("removing", toml_edit::value(true));
        }
        if let Some(e) = self.entries.iter_mut().find(|e| e.name == name) {
            e.removing = true;
        }
    }

    /// Drop an entry; its children move up to its parent.
    pub fn remove(&mut self, name: &str) -> Option<Entry> {
        let idx = self.entries.iter().position(|e| e.name == name)?;
        let gone = self.entries.remove(idx);
        self.doc.remove(name);
        let new_parent = gone.parent().to_string();
        let children: Vec<String> = self
            .entries
            .iter()
            .filter(|e| e.parent() == name)
            .map(|e| e.name.clone())
            .collect();
        for child in children {
            if let Some(t) = self.doc.get_mut(&child).and_then(Item::as_table_mut) {
                t.insert("parent", toml_edit::value(new_parent.as_str()));
            }
            if let Some(e) = self.entries.iter_mut().find(|e| e.name == child) {
                e.parent = Some(new_parent.clone());
            }
        }
        Some(gone)
    }

    pub fn save(&self) -> Result<()> {
        paths::write_atomic(&self.path, &self.doc.to_string())?;
        Ok(())
    }
}

/// Lock, load, change, save the file of workspaces.
pub fn edit<T>(f: impl FnOnce(&mut WorkspacesFile) -> Result<T>) -> Result<T> {
    paths::ensure_state_dir()?;
    let path = paths::workspaces_file();
    let _lock = FileLock::acquire(&paths::lock_for(&path))?;
    let mut file = WorkspacesFile::load(&path)?;
    let before = file.doc.to_string();
    let out = f(&mut file)?;
    if file.doc.to_string() != before {
        file.check_tree()?;
        file.save()?;
    }
    Ok(out)
}

/// The calling workspace's name: its herdr label.
pub fn caller_name(snap: &Snapshot) -> Option<String> {
    let id = std::env::var("HERDR_WORKSPACE_ID").ok()?;
    snap.workspace(&id).and_then(|w| w.label.clone())
}

// ---------------------------------------------------------------- config --

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub kind: String,
    pub args: Vec<String>,
    pub command: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        AgentConfig {
            kind: "claude".into(),
            args: vec!["--model".into(), "{model}".into()],
            command: None,
        }
    }
}

pub fn load_config() -> Result<AgentConfig> {
    let path = paths::config_file();
    let mut cfg = AgentConfig::default();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(cfg);
    };
    let doc: DocumentMut = text
        .parse()
        .map_err(|e| anyhow!("{}: {e}", path.display()))?;
    if let Some(agent) = doc.get("agent").and_then(Item::as_table_like) {
        if let Some(k) = agent.get("kind").and_then(Item::as_str) {
            cfg.kind = k.to_string();
        }
        if let Some(a) = agent.get("args").and_then(Item::as_array) {
            cfg.args = a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
        }
        cfg.command = agent
            .get("command")
            .and_then(Item::as_str)
            .map(str::to_string);
    }
    Ok(cfg)
}

/// Fill `{model}` in each argument. Without a model, an argument naming it is
/// dropped, with the flag before it.
pub fn expand_args(args: &[String], model: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for arg in args {
        if arg.contains("{model}") {
            match model {
                Some(m) => out.push(arg.replace("{model}", m)),
                None => {
                    if arg == "{model}" && out.last().map(|l| l.starts_with('-')).unwrap_or(false) {
                        out.pop();
                    }
                }
            }
        } else {
            out.push(arg.clone());
        }
    }
    out
}

pub fn expand_command(command: &str, model: Option<&str>) -> String {
    match model {
        Some(m) => command.replace("{model}", m),
        None => command
            .split_whitespace()
            .filter(|w| !w.contains("{model}"))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

// ------------------------------------------------------------------ add --

pub struct AddArgs {
    pub name: String,
    pub dir: String,
    pub branch: Option<String>,
    pub base: Option<String>,
    pub path: Option<String>,
    pub parent: Option<String>,
    pub purpose: Option<String>,
    pub brief: Option<String>,
    pub model: Option<String>,
}

fn cli_json(backend: &dyn Backend, args: &[&str]) -> Result<Value> {
    let (code, out, err) = backend.cli(args)?;
    if code != 0 {
        bail!(
            "herdr {}: {}",
            args[..2.min(args.len())].join(" "),
            err.trim()
        );
    }
    serde_json::from_str(&out).with_context(|| format!("herdr {} printed: {out}", args.join(" ")))
}

pub fn add(args: AddArgs) -> Result<()> {
    let backend = Herdr::from_env();
    add_with(&backend, args)
}

pub fn add_with(backend: &dyn Backend, args: AddArgs) -> Result<()> {
    if !valid_name(&args.name) {
        bail!(
            "`{}` is not a valid workspace name: use {NAME_PATTERN} (herdr's agent-name rule)",
            args.name
        );
    }
    let snap = backend.snapshot()?;
    let parent = args
        .parent
        .clone()
        .or_else(|| caller_name(&snap))
        .unwrap_or_else(|| OPERATOR.to_string());
    let has_agent = args.brief.is_some() || args.model.is_some();
    let entry = Entry {
        name: args.name.clone(),
        dir: args.dir.clone(),
        branch: args.branch.clone(),
        base: args.base.clone(),
        purpose: args.purpose.clone(),
        parent: Some(parent.clone()),
        agent: has_agent.then(|| AgentSpec {
            brief: args.brief.clone(),
            model: args.model.clone(),
        }),
        unmanaged: false,
        removing: false,
    };
    // The entry goes in first, so the watcher never mistakes the new
    // workspace for one opened by hand.
    edit(|file| {
        if file.get(&args.name).is_some() {
            bail!("`{}` is already in {}", args.name, file.path.display());
        }
        if snap.workspace_by_label(&args.name).is_some() {
            bail!("a herdr workspace is already labelled `{}`", args.name);
        }
        if parent != OPERATOR && file.get(&parent).is_none() {
            bail!(
                "parent `{parent}` is not in the file (known: {})",
                file.names().join(", ")
            );
        }
        file.add(entry.clone());
        Ok(())
    })?;
    let created = create(backend, &args);
    let (ws, pane, dir) = match created {
        Ok(x) => x,
        Err(e) => {
            let _ = edit(|file| {
                file.remove(&args.name);
                Ok(())
            });
            return Err(e);
        }
    };
    println!("opened workspace {} ({ws})", args.name);

    // Its own workspace file: the agent alone in tab main.
    let wsfile_path = paths::workspace_file(&ws);
    {
        let _lock = FileLock::acquire(&paths::lock_for(&wsfile_path))?;
        let mut file = crate::wsfile::WorkspaceFile::empty(&wsfile_path);
        file.set_dir(&dir);
        file.insert(
            "main",
            crate::layout::Leaf::new(crate::services::RESERVED),
            None,
        )?;
        file.save()?;
    }

    if has_agent {
        start_agent(backend, &args.name, &pane, args.model.as_deref())?;
        println!("agent {} started", args.name);
        if let Some(brief) = &args.brief {
            prompt(backend, &args.name, &format!("Read {brief} and follow it."))?;
            println!("brief sent: {brief}");
        }
    }
    Ok(())
}

/// Create the workspace through herdr. Returns (workspace id, root pane, folder).
fn create(backend: &dyn Backend, args: &AddArgs) -> Result<(String, String, String)> {
    let dir = paths::expand_tilde(&args.dir);
    let dir_s = dir.to_string_lossy().into_owned();
    let (ws, tab, pane, folder) = match &args.branch {
        Some(branch) => {
            let mut cli = vec![
                "worktree",
                "create",
                "--cwd",
                &dir_s,
                "--branch",
                branch,
                "--label",
                &args.name,
                "--no-focus",
            ];
            if let Some(base) = &args.base {
                cli.extend(["--base", base]);
            }
            if let Some(path) = &args.path {
                cli.extend(["--path", path]);
            }
            let r = cli_json(backend, &cli)?;
            let r = &r["result"];
            let get = |p: &str| {
                r.pointer(p)
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| anyhow!("herdr worktree create: reply has no {p}"))
            };
            (
                get("/workspace/workspace_id")?,
                get("/tab/tab_id")?,
                get("/root_pane/pane_id")?,
                get("/worktree/path").or_else(|_| get("/root_pane/cwd"))?,
            )
        }
        None => {
            if !dir.is_dir() {
                bail!("{} is not a folder", dir.display());
            }
            let (ws, tab, pane) = backend.create_workspace(&args.name, &dir_s)?;
            (ws, tab, pane, dir_s.clone())
        }
    };
    backend.rename_tab(&tab, "main")?;
    backend.rename_pane(&pane, Some(crate::services::RESERVED))?;
    Ok((ws, pane, folder))
}

fn start_agent(backend: &dyn Backend, name: &str, pane: &str, model: Option<&str>) -> Result<()> {
    let cfg = load_config()?;
    match &cfg.command {
        None => {
            let args = expand_args(&cfg.args, model);
            let mut cli: Vec<&str> =
                vec!["agent", "start", name, "--kind", &cfg.kind, "--pane", pane];
            if !args.is_empty() {
                cli.push("--");
                cli.extend(args.iter().map(String::as_str));
            }
            let (code, _, err) = backend.cli(&cli)?;
            if code != 0 {
                bail!("herdr agent start: {}", err.trim());
            }
        }
        Some(command) => {
            // A wrapper typed at the shell prompt, so shell functions work.
            backend.run(pane, &expand_command(command, model))?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
            loop {
                let snap = backend.snapshot()?;
                let p = snap.pane(pane);
                let ready = p
                    .map(|p| {
                        p.agent.is_some()
                            && matches!(p.agent_status.as_deref(), Some("idle" | "done"))
                    })
                    .unwrap_or(false);
                if ready {
                    break;
                }
                if std::time::Instant::now() > deadline {
                    bail!("the agent started by `{command}` was not detected within 120s");
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            let (code, _, err) = backend.cli(&["agent", "rename", pane, name])?;
            if code != 0 {
                bail!("herdr agent rename: {}", err.trim());
            }
        }
    }
    Ok(())
}

fn prompt(backend: &dyn Backend, name: &str, text: &str) -> Result<()> {
    let (code, _, err) = backend.cli(&["agent", "prompt", name, text])?;
    if code != 0 {
        bail!("herdr agent prompt: {}", err.trim());
    }
    Ok(())
}

// --------------------------------------------------------------- remove --

pub fn remove(name: &str) -> Result<()> {
    let backend = Herdr::from_env();
    edit(|file| {
        if file.get(name).is_none() {
            bail!(
                "`{name}` is not in the file (known: {})",
                file.names().join(", ")
            );
        }
        file.set_removing(name);
        Ok(())
    })?;
    match finish_removal(&backend, name)? {
        Removal::Done(how) => println!("{name}: {how}"),
        Removal::Waiting(why) => println!("{name}: marked for removal; {why}"),
    }
    Ok(())
}

pub enum Removal {
    Done(String),
    Waiting(String),
}

/// The branch's changes are already in its base (merged or squash-merged).
pub fn merged(dir: &Path, branch: &str, base: Option<&str>) -> Result<bool> {
    let git = |args: &[&str]| -> Result<(bool, String)> {
        let out = Command::new("git").arg("-C").arg(dir).args(args).output()?;
        Ok((
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ))
    };
    if !git(&[
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("refs/heads/{branch}"),
    ])?
    .0
    {
        // No branch left: nothing unmerged to lose.
        return Ok(true);
    }
    let base = match base {
        Some(b) => b.to_string(),
        None => {
            let (ok, head) = git(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])?;
            if ok && !head.is_empty() {
                head
            } else if git(&["rev-parse", "--verify", "--quiet", "refs/heads/main"])?.0 {
                "main".into()
            } else {
                "master".into()
            }
        }
    };
    if git(&["merge-base", "--is-ancestor", branch, &base])?.0 {
        return Ok(true);
    }
    // Squash merges: merging the branch into the base changes nothing.
    let (ok, merged_tree) = git(&["merge-tree", "--write-tree", &base, branch])?;
    let (_, base_tree) = git(&["rev-parse", &format!("{base}^{{tree}}")])?;
    Ok(ok && merged_tree.lines().next() == Some(base_tree.as_str()))
}

/// Try to finish a pending removal now.
pub fn finish_removal(backend: &dyn Backend, name: &str) -> Result<Removal> {
    let snap = backend.snapshot()?;
    let path = paths::workspaces_file();
    let entry = WorkspacesFile::load(&path)?
        .get(name)
        .cloned()
        .ok_or_else(|| anyhow!("`{name}` is not in the file"))?;
    let Some(ws) = snap
        .workspace_by_label(name)
        .map(|w| w.workspace_id.clone())
    else {
        drop_entry(name)?;
        return Ok(Removal::Done("not open; entry removed".into()));
    };
    if let Some(busy) = snap.panes_of(&ws).find(|p| p.busy()) {
        return Ok(Removal::Waiting(format!(
            "waiting for `{}` to go idle",
            busy.label.as_deref().unwrap_or(&busy.pane_id)
        )));
    }
    match &entry.branch {
        Some(branch) => {
            let dir = paths::expand_tilde(&entry.dir);
            if !merged(&dir, branch, entry.base.as_deref())? {
                let reason = format!("branch `{branch}` has unmerged commits; not removed");
                crate::needs::add(name, &reason, crate::needs::HELD_REMOVAL, None)?;
                return Ok(Removal::Waiting(reason));
            }
            let (code, _, err) = backend.cli(&["worktree", "remove", "--workspace", &ws])?;
            if code != 0 {
                let reason = format!("herdr worktree remove failed: {}", err.trim());
                crate::needs::add(name, &reason, crate::needs::HELD_REMOVAL, None)?;
                return Ok(Removal::Waiting(reason));
            }
            // The worktree remove closes its workspace; make sure.
            if backend.snapshot()?.workspace(&ws).is_some() {
                let _ = backend.close_workspace(&ws);
            }
            drop_entry(name)?;
            Ok(Removal::Done(format!(
                "worktree removed (branch `{branch}` merged)"
            )))
        }
        None => match backend.close_workspace(&ws) {
            Ok(()) => {
                drop_entry(name)?;
                Ok(Removal::Done("workspace closed; folder kept".into()))
            }
            Err(e) => {
                let msg = format!("{e:#}");
                let reason = if msg.contains("workspace_group_close_required") {
                    "has linked worktree workspaces; close them first (herdfile never closes a group)".to_string()
                } else {
                    format!("could not close: {msg}")
                };
                crate::needs::add(name, &reason, crate::needs::HELD_REMOVAL, None)?;
                Ok(Removal::Waiting(reason))
            }
        },
    }
}

fn drop_entry(name: &str) -> Result<()> {
    edit(|file| {
        file.remove(name);
        Ok(())
    })?;
    crate::needs::clear(|n| n.workspace == name && n.source == crate::needs::HELD_REMOVAL)?;
    Ok(())
}

// ----------------------------------------------------------------- tree --

pub fn tree() -> Result<()> {
    let backend = Herdr::from_env();
    let snap = backend.snapshot()?;
    let file = WorkspacesFile::load(&paths::workspaces_file())?;
    print!("{}", render_tree(&file, &snap));
    Ok(())
}

fn status_of(snap: &Snapshot, name: &str) -> String {
    let Some(ws) = snap.workspace_by_label(name) else {
        return "not open".into();
    };
    match snap.pane_by_label(&ws.workspace_id, crate::services::RESERVED) {
        Some(p) if p.agent.is_some() => p.agent_status.clone().unwrap_or_else(|| "unknown".into()),
        _ => match snap.panes_of(&ws.workspace_id).find(|p| p.agent.is_some()) {
            Some(p) => p.agent_status.clone().unwrap_or_else(|| "unknown".into()),
            None => "no agent".into(),
        },
    }
}

pub fn render_tree(file: &WorkspacesFile, snap: &Snapshot) -> String {
    let mut out = String::from("operator\n");
    fn walk(file: &WorkspacesFile, snap: &Snapshot, parent: &str, prefix: &str, out: &mut String) {
        let kids: Vec<&Entry> = file
            .entries
            .iter()
            .filter(|e| e.parent() == parent)
            .collect();
        for (i, e) in kids.iter().enumerate() {
            let last = i + 1 == kids.len();
            let mut flags = vec![status_of(snap, &e.name)];
            if e.unmanaged {
                flags.push("unmanaged".into());
            }
            if e.removing {
                flags.push("removing".into());
            }
            out.push_str(&format!(
                "{prefix}{} {}  [{}]{}\n",
                if last { "└─" } else { "├─" },
                e.name,
                flags.join(", "),
                e.purpose
                    .as_ref()
                    .map(|p| format!("  {p}"))
                    .unwrap_or_default()
            ));
            let next = format!("{prefix}{}", if last { "   " } else { "│  " });
            walk(file, snap, &e.name, &next, out);
        }
    }
    walk(file, snap, OPERATOR, "", &mut out);
    out
}

// --------------------------------------------------- watcher write-back --

/// What the watcher remembers about workspaces between ticks.
#[derive(Debug, Default)]
pub struct FleetState {
    pub dirty: bool,
    /// Names seen open while in the file; persisted beside the file.
    known: Option<HashSet<String>>,
    last_removal_check: Option<std::time::Instant>,
}

fn known_path() -> PathBuf {
    paths::state_dir().join(".workspaces.seen.json")
}

/// Fold hand changes to workspaces into the file and finish pending
/// removals. Only acts once the file of workspaces exists.
pub fn tick(backend: &dyn Backend, state: &mut FleetState) -> Result<()> {
    let path = paths::workspaces_file();
    if !path.exists() {
        return Ok(());
    }
    let due_removals = state
        .last_removal_check
        .map(|t| t.elapsed() > std::time::Duration::from_secs(5))
        .unwrap_or(true);
    if !state.dirty && !due_removals {
        return Ok(());
    }
    state.dirty = false;
    let known = state.known.get_or_insert_with(|| {
        std::fs::read_to_string(known_path())
            .ok()
            .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
            .map(|v| v.into_iter().collect())
            .unwrap_or_default()
    });
    let snap = backend.snapshot()?;
    let live: BTreeMap<String, String> = snap
        .workspaces
        .iter()
        .map(|w| {
            (
                w.label.clone().unwrap_or_else(|| w.workspace_id.clone()),
                w.workspace_id.clone(),
            )
        })
        .collect();
    let mut log_lines = Vec::new();
    let removing: Vec<String> = edit(|file| {
        for e in file.entries.clone() {
            let open = live.contains_key(&e.name);
            if open {
                known.insert(e.name.clone());
            } else if e.removing || e.unmanaged || known.contains(&e.name) {
                // Closed by hand (or by a finished removal): drop it.
                file.remove(&e.name);
                known.remove(&e.name);
                log_lines.push(format!(
                    "workspace {} closed; dropped from the file",
                    e.name
                ));
            }
        }
        for (label, ws) in &live {
            if file.get(label).is_none() && !known.contains(label) {
                let dir = snap
                    .panes_of(ws)
                    .find_map(|p| p.cwd.clone())
                    .unwrap_or_else(|| "~".into());
                file.add(Entry {
                    name: label.clone(),
                    dir,
                    unmanaged: true,
                    ..Entry::default()
                });
                known.insert(label.clone());
                log_lines.push(format!(
                    "workspace {label} opened outside herdfile; recorded as unmanaged"
                ));
            }
        }
        Ok(file
            .entries
            .iter()
            .filter(|e| e.removing && live.contains_key(&e.name))
            .map(|e| e.name.clone())
            .collect())
    })?;
    let _ = paths::write_atomic(
        &known_path(),
        &serde_json::to_string(&known.iter().collect::<Vec<_>>())?,
    );
    for line in log_lines {
        crate::watch::log(&line);
    }
    if due_removals {
        state.last_removal_check = Some(std::time::Instant::now());
        for name in removing {
            match finish_removal(backend, &name) {
                Ok(Removal::Done(how)) => crate::watch::log(&format!("workspace {name}: {how}")),
                Ok(Removal::Waiting(_)) => {}
                Err(e) => crate::watch::log(&format!("workspace {name}: {e:#}")),
            }
        }
    }
    Ok(())
}

/// `adopt --all`: every open workspace into the file of workspaces as
/// unmanaged, and a workspace file for each that lacks one. Closes nothing.
pub fn adopt_all() -> Result<()> {
    let backend = Herdr::from_env();
    let snap = backend.snapshot()?;
    let added = edit(|file| {
        let mut added = Vec::new();
        for w in &snap.workspaces {
            let label = w.label.clone().unwrap_or_else(|| w.workspace_id.clone());
            if file.get(&label).is_some() {
                continue;
            }
            let dir = snap
                .panes_of(&w.workspace_id)
                .find_map(|p| p.cwd.clone())
                .unwrap_or_else(|| "~".into());
            file.add(Entry {
                name: label.clone(),
                dir,
                unmanaged: true,
                ..Entry::default()
            });
            added.push(label);
        }
        Ok(added)
    })?;
    println!("workspaces recorded as unmanaged: {}", added.len());
    for w in &snap.workspaces {
        if paths::workspace_file(&w.workspace_id).exists() {
            continue;
        }
        match crate::adopt::adopt_with(&backend, &w.workspace_id, false) {
            Ok(path) => println!("wrote {}", path.display()),
            Err(e) => eprintln!("herdfile: {}: {e:#}", w.workspace_id),
        }
    }
    crate::control::warn_if_no_watcher();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<WorkspacesFile> {
        WorkspacesFile::parse(text, Path::new("/s/workspaces.toml"))
    }

    #[test]
    fn names_follow_herdr() {
        assert!(valid_name("review-pr-312"));
        assert!(!valid_name("Review PR 312"));
        assert!(!valid_name("1abc"));
        assert!(!valid_name(&"a".repeat(33)));
    }

    #[test]
    fn parents_must_exist_and_not_cycle() {
        assert!(parse("[a]\ndir = \"~\"\nparent = \"nope\"\n").is_err());
        let err = parse("[a]\ndir = \"~\"\nparent = \"b\"\n[b]\ndir = \"~\"\nparent = \"a\"\n")
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("cycle"), "{err}");
        parse("[a]\ndir = \"~\"\n[b]\ndir = \"~\"\nparent = \"a\"\n").unwrap();
    }

    #[test]
    fn remove_reparents_children() {
        let mut f = parse(
            "[orch]\ndir = \"~\"\n[land-prs]\ndir = \"~\"\nparent = \"orch\"\n[review]\ndir = \"~\"\nparent = \"land-prs\"\n",
        )
        .unwrap();
        f.remove("land-prs");
        assert_eq!(f.get("review").unwrap().parent(), "orch");
        assert!(f
            .doc
            .to_string()
            .contains("[review]\ndir = \"~\"\nparent = \"orch\""));
    }

    #[test]
    fn model_expansion() {
        let args = vec!["--model".to_string(), "{model}".to_string()];
        assert_eq!(expand_args(&args, Some("opus")), ["--model", "opus"]);
        assert!(expand_args(&args, None).is_empty());
        assert_eq!(expand_command("cl --{model}", Some("opus")), "cl --opus");
        assert_eq!(expand_command("cl --{model}", None), "cl");
    }
}
