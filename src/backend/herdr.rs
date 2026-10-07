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
    socket_from(|k| std::env::var(k).ok())
}

/// herdr's own lookup: `HERDR_SOCKET_PATH`, else the session named by
/// `HERDR_SESSION`, else the default session, under `XDG_CONFIG_HOME`.
fn socket_from(var: impl Fn(&str) -> Option<String>) -> PathBuf {
    let var = |k: &str| var(k).filter(|v| !v.is_empty());
    if let Some(path) = var("HERDR_SOCKET_PATH") {
        return PathBuf::from(path);
    }
    let config = match var("XDG_CONFIG_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(var("HOME").unwrap_or_default()).join(".config"),
    };
    match var("HERDR_SESSION").filter(|s| s != "default") {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: std::collections::HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn socket_lookup_follows_herdr() {
        assert_eq!(
            socket_from(env(&[
                ("HERDR_SOCKET_PATH", "/s.sock"),
                ("HERDR_SESSION", "x")
            ])),
            PathBuf::from("/s.sock")
        );
        assert_eq!(
            socket_from(env(&[("HOME", "/h"), ("HERDR_SESSION", "t")])),
            PathBuf::from("/h/.config/herdr/sessions/t/herdr.sock")
        );
        assert_eq!(
            socket_from(env(&[
                ("XDG_CONFIG_HOME", "/c"),
                ("HERDR_SESSION", "default")
            ])),
            PathBuf::from("/c/herdr/herdr.sock")
        );
    }

    /// A one-request herdr stand-in on a temp socket.
    fn fake_herdr(replies: &'static [&'static str]) -> (tempfile::TempDir, Herdr) {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("h.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut stream = stream;
            for reply in replies {
                writeln!(stream, "{reply}").unwrap();
            }
        });
        let h = Herdr {
            socket,
            bin: PathBuf::from("herdr"),
        };
        (dir, h)
    }

    #[test]
    fn errors_carry_code_and_message() {
        let (_d, h) =
            fake_herdr(&[r#"{"id":"x","error":{"code":"agent_blocked","message":"is blocked"}}"#]);
        let err = h
            .request("agent.prompt", json!({}))
            .unwrap_err()
            .to_string();
        assert_eq!(err, "herdr agent.prompt: agent_blocked: is blocked");
    }

    #[test]
    fn same_tab_move_is_an_error() {
        let (_d, h) = fake_herdr(&[
            r#"{"id":"x","result":{"move_result":{"changed":false,"reason":"same_tab","pane":{"pane_id":"w1:p1"}}}}"#,
        ]);
        let at = Placement {
            target_pane: "w1:p2",
            dir: crate::layout::Dir::Row,
            ratio: None,
        };
        let err = h.move_pane("w1:p1", "w1:t1", &at).unwrap_err().to_string();
        assert!(err.contains("same_tab"), "{err}");
    }

    #[test]
    fn subscribe_reports_start_then_events() {
        let (_d, h) = fake_herdr(&[
            r#"{"id":"herdfile:subscribe","result":{"type":"subscription_started"}}"#,
            r#"{"event":"pane_closed","data":{"pane_id":"w3:p2"}}"#,
        ]);
        let mut seen = Vec::new();
        h.subscribe(&mut |e| {
            seen.push((e.kind.clone(), e.workspace_id()));
            seen.len() < 2
        })
        .unwrap();
        assert_eq!(seen[0].0, "subscribed");
        assert_eq!(seen[1], ("pane_closed".to_string(), Some("w3".to_string())));
    }
}
