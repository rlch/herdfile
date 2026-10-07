//! Where herdfile keeps its files.

use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// `$XDG_STATE_HOME/herdfile`, defaulting to `~/.local/state/herdfile`.
pub fn state_dir() -> PathBuf {
    match std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("herdfile"),
        None => home().join(".local/state/herdfile"),
    }
}

/// `$HERDFILE_CONFIG`, else `$XDG_CONFIG_HOME/herdfile/config.toml`, defaulting to
/// `~/.config/herdfile/config.toml`.
pub fn config_file() -> PathBuf {
    if let Some(path) = std::env::var_os("HERDFILE_CONFIG").filter(|v| !v.is_empty()) {
        return PathBuf::from(path);
    }
    let base = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => home().join(".config"),
    };
    base.join("herdfile/config.toml")
}

pub fn workspace_file(workspace_id: &str) -> PathBuf {
    state_dir().join(format!("{workspace_id}.toml"))
}

pub fn workspaces_file() -> PathBuf {
    state_dir().join("workspaces.toml")
}

pub fn needs_file() -> PathBuf {
    state_dir().join("needs.jsonl")
}

pub fn watch_lock() -> PathBuf {
    state_dir().join("watch.lock")
}

pub fn watch_log() -> PathBuf {
    state_dir().join("watch.log")
}

pub fn watch_socket() -> PathBuf {
    state_dir().join("watch.sock")
}

/// Lock file guarding one workspace file (or the workspaces file).
pub fn lock_for(file: &Path) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    file.with_file_name(name)
}

/// Expand a leading `~` the way a shell would.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        home()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(path)
    }
}

pub fn ensure_state_dir() -> std::io::Result<PathBuf> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Write a file atomically: write a sibling temp file, then rename over.
pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(format!(".tmp{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}
