//! The watcher's local socket: commands ask it to apply now and wait.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::apply::Report;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Apply {
        workspace: String,
    },
    /// Re-read the file of workspaces and act on pending removals.
    Workspaces,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Reply {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub report: Option<Report>,
    /// Ways herdr still differs from the file after apply.
    #[serde(default)]
    pub problems: Vec<String>,
}

impl Reply {
    pub fn error(message: impl Into<String>) -> Reply {
        Reply {
            ok: false,
            error: Some(message.into()),
            ..Reply::default()
        }
    }
}

pub fn send(request: &Request, timeout: Duration) -> Result<Reply> {
    let path = crate::paths::watch_socket();
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("watcher not reachable at {}", path.display()))?;
    stream.set_read_timeout(Some(timeout))?;
    writeln!(stream, "{}", serde_json::to_string(request)?)?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| anyhow!("no reply from the watcher: {e}"))?;
    if line.trim().is_empty() {
        return Err(anyhow!(
            "the watcher closed the connection without replying"
        ));
    }
    Ok(serde_json::from_str(&line)?)
}

/// Is a watcher running and answering?
pub fn watcher_pid() -> Option<u32> {
    if !crate::lock::is_held(&crate::paths::watch_lock()) {
        return None;
    }
    send(&Request::Ping, Duration::from_secs(2))
        .ok()
        .and_then(|r| r.pid)
}

pub fn warn_if_no_watcher() {
    if watcher_pid().is_none() {
        eprintln!(
            "herdfile: warning: the watcher is not running, so edits will not be applied (start it with `herdfile watch --detach`)"
        );
    }
}
