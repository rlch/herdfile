//! The herdr backend: newline-delimited JSON over herdr's API socket, plus the
//! `herdr` CLI for commands whose behaviour lives client-side.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

use super::{Backend, Event, Placement, Snapshot, Spawn};

/// Events the watcher listens to. Agent status is read from snapshots, since
/// herdr only offers it per pane.
const SUBSCRIPTIONS: &[&str] = &[
    "workspace.created",
    "workspace.closed",
    "workspace.renamed",
    "tab.created",
    "tab.closed",
    "tab.renamed",
    "tab.moved",
    "tab.focused",
    "pane.created",
    "pane.closed",
    "pane.moved",
    "pane.exited",
    "pane.agent_detected",
    "layout.updated",
];

pub struct Herdr {
    pub socket: PathBuf,
    pub bin: PathBuf,
}

/// The socket a plain `herdr` CLI call would use from this environment.
pub fn default_socket() -> PathBuf {
    if let Some(path) = std::env::var_os("HERDR_SOCKET_PATH").filter(|v| !v.is_empty()) {
        return PathBuf::from(path);
    }
    let config = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"),
    };
    match std::env::var("HERDR_SESSION")
        .ok()
        .filter(|s| !s.is_empty() && s != "default")
    {
        Some(name) => config.join("herdr/sessions").join(name).join("herdr.sock"),
        None => config.join("herdr/herdr.sock"),
    }
}

impl Herdr {
    pub fn from_env() -> Herdr {
        let bin = std::env::var_os("HERDR_BIN_PATH")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("herdr"));
        Herdr {
            socket: default_socket(),
            bin,
        }
    }

    pub fn with_socket(socket: &Path) -> Herdr {
        let mut h = Herdr::from_env();
        h.socket = socket.to_path_buf();
        h
    }

    fn connect(&self) -> Result<UnixStream> {
        UnixStream::connect(&self.socket)
            .with_context(|| format!("cannot reach herdr at {}", self.socket.display()))
    }

    pub fn request(&self, method: &str, params: Value) -> Result<Value> {
        let mut stream = self.connect()?;
        let req = json!({ "id": format!("herdfile:{method}"), "method": method, "params": params });
        writeln!(stream, "{req}")?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        let value: Value = serde_json::from_str(&line)
            .with_context(|| format!("herdr sent an unreadable reply to {method}: {line}"))?;
        if let Some(err) = value.get("error") {
            let code = err.get("code").and_then(Value::as_str).unwrap_or("error");
            let message = err.get("message").and_then(Value::as_str).unwrap_or("");
            bail!("herdr {method}: {code}: {message}");
        }
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    }
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> Result<&'a str> {
    let mut cur = v;
    for key in path {
        cur = cur
            .get(key)
            .ok_or_else(|| anyhow!("herdr reply has no {}", path.join(".")))?;
    }
    cur.as_str()
        .ok_or_else(|| anyhow!("herdr reply: {} is not a string", path.join(".")))
}

fn env_map(spawn: &Spawn) -> Value {
    let map: serde_json::Map<String, Value> = spawn
        .env
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    Value::Object(map)
}

impl Backend for Herdr {
    fn snapshot(&self) -> Result<Snapshot> {
        let result = self.request("session.snapshot", json!({}))?;
        let snap = result
            .get("snapshot")
            .cloned()
            .ok_or_else(|| anyhow!("herdr snapshot reply has no snapshot"))?;
        Ok(serde_json::from_value(snap)?)
    }

    fn subscribe(&self, on_event: &mut dyn FnMut(Event) -> bool) -> Result<()> {
        let mut stream = self.connect()?;
        let subs: Vec<Value> = SUBSCRIPTIONS.iter().map(|t| json!({ "type": t })).collect();
        let req = json!({ "id": "herdfile:subscribe", "method": "events.subscribe",
                          "params": { "subscriptions": subs } });
        writeln!(stream, "{req}")?;
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            let line = line?;
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(err) = value.get("error") {
                bail!("herdr events.subscribe: {err}");
            }
            if value.pointer("/result/type").and_then(Value::as_str) == Some("subscription_started")
            {
                let started = Event {
                    kind: "subscribed".to_string(),
                    data: Value::Null,
                };
                if !on_event(started) {
                    break;
                }
                continue;
            }
            let Some(kind) = value.get("event").and_then(Value::as_str) else {
                continue;
            };
            let event = Event {
                kind: kind.to_string(),
                data: value.get("data").cloned().unwrap_or(Value::Null),
            };
            if !on_event(event) {
                break;
            }
        }
        Ok(())
    }

    fn create_workspace(&self, label: &str, cwd: &str) -> Result<(String, String, String)> {
        let r = self.request(
            "workspace.create",
            json!({ "label": label, "cwd": cwd, "focus": false }),
        )?;
        Ok((
            str_at(&r, &["workspace", "workspace_id"])?.to_string(),
            str_at(&r, &["tab", "tab_id"])?.to_string(),
            str_at(&r, &["root_pane", "pane_id"])?.to_string(),
        ))
    }

    fn close_workspace(&self, workspace: &str) -> Result<()> {
        // Never `close_group`: a primary workspace with linked worktrees stays.
        self.request("workspace.close", json!({ "workspace_id": workspace }))?;
        Ok(())
    }

    fn create_tab(&self, workspace: &str, label: &str, spawn: &Spawn) -> Result<(String, String)> {
        let r = self.request(
            "tab.create",
            json!({ "workspace_id": workspace, "label": label, "cwd": spawn.cwd,
                    "env": env_map(spawn), "focus": false }),
        )?;
        Ok((
            str_at(&r, &["tab", "tab_id"])?.to_string(),
            str_at(&r, &["root_pane", "pane_id"])?.to_string(),
        ))
    }

    fn rename_tab(&self, tab: &str, label: &str) -> Result<()> {
        self.request("tab.rename", json!({ "tab_id": tab, "label": label }))?;
        Ok(())
    }

    fn move_tab(&self, tab: &str, index: usize) -> Result<()> {
        self.request("tab.move", json!({ "tab_id": tab, "insert_index": index }))?;
        Ok(())
    }

    fn split(&self, at: &Placement, spawn: &Spawn) -> Result<String> {
        let r = self.request(
            "pane.split",
            json!({ "target_pane_id": at.target_pane, "direction": at.dir.split(),
                    "ratio": at.ratio, "cwd": spawn.cwd, "env": env_map(spawn), "focus": false }),
        )?;
        Ok(str_at(&r, &["pane", "pane_id"])?.to_string())
    }

    fn move_pane(&self, pane: &str, tab: &str, at: &Placement) -> Result<String> {
        let r = self.request(
            "pane.move",
            json!({ "pane_id": pane, "focus": false,
                    "destination": { "type": "tab", "tab_id": tab, "split": at.dir.split(),
                                     "target_pane_id": at.target_pane, "ratio": at.ratio } }),
        )?;
        if r.pointer("/move_result/changed") == Some(&Value::Bool(false)) {
            let reason = r
                .pointer("/move_result/reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            bail!("herdr did not move {pane}: {reason}");
        }
        Ok(str_at(&r, &["move_result", "pane", "pane_id"])?.to_string())
    }

    fn move_pane_new_tab(
        &self,
        pane: &str,
        workspace: &str,
        label: &str,
    ) -> Result<(String, String)> {
        let r = self.request(
            "pane.move",
            json!({ "pane_id": pane, "focus": false,
                    "destination": { "type": "new_tab", "workspace_id": workspace, "label": label } }),
        )?;
        let pane_id = str_at(&r, &["move_result", "pane", "pane_id"])?.to_string();
        let tab_id = str_at(&r, &["move_result", "pane", "tab_id"])?.to_string();
        Ok((pane_id, tab_id))
    }

    fn swap(&self, a: &str, b: &str) -> Result<()> {
        self.request(
            "pane.swap",
            json!({ "source_pane_id": a, "target_pane_id": b }),
        )?;
        Ok(())
    }

    fn close_pane(&self, pane: &str) -> Result<()> {
        self.request("pane.close", json!({ "pane_id": pane }))?;
        Ok(())
    }

    fn rename_pane(&self, pane: &str, label: Option<&str>) -> Result<()> {
        self.request("pane.rename", json!({ "pane_id": pane, "label": label }))?;
        Ok(())
    }

    fn run(&self, pane: &str, command: &str) -> Result<()> {
        self.request(
            "pane.send_input",
            json!({ "pane_id": pane, "text": command, "keys": ["Enter"] }),
        )?;
        Ok(())
    }

    fn set_ratio(&self, tab: &str, path: &[bool], ratio: f64) -> Result<()> {
        self.request(
            "layout.set_split_ratio",
            json!({ "tab_id": tab, "path": path, "ratio": ratio }),
        )?;
        Ok(())
    }

    fn cli(&self, args: &[&str]) -> Result<(i32, String, String)> {
        let out = Command::new(&self.bin)
            .args(args)
            .env("HERDR_SOCKET_PATH", &self.socket)
            .env_remove("HERDR_SESSION")
            .output()
            .with_context(|| format!("cannot run {}", self.bin.display()))?;
        Ok((
            out.status.code().unwrap_or(1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }
}
