//! `herdfile tell`: a thin wrapper over `herdr agent prompt` that adds the
//! sender's name and resolves `parent`.

use anyhow::{bail, Result};

use crate::backend::herdr::Herdr;
use crate::backend::Backend;
use crate::paths;
use crate::workspaces::{caller_name, WorkspacesFile, OPERATOR};

pub struct TellArgs {
    pub target: String,
    pub text: String,
    pub wait: bool,
    pub until: Vec<String>,
    pub timeout: Option<String>,
}

/// Returns herdr's exit code, so errors pass through unchanged.
pub fn tell(args: TellArgs) -> Result<i32> {
    let backend = Herdr::from_env();
    let snap = backend.snapshot()?;
    let file = WorkspacesFile::load(&paths::workspaces_file())?;
    let sender = caller_name(&snap).unwrap_or_else(|| OPERATOR.to_string());
    let target = if args.target == "parent" {
        let me = file.get(&sender).ok_or_else(|| {
            anyhow::anyhow!("`{sender}` is not in the file of workspaces, so it has no parent")
        })?;
        let parent = me.parent().to_string();
        if parent == OPERATOR {
            bail!("your parent is the operator; use `herdfile ask \"...\"` to reach them");
        }
        parent
    } else {
        args.target.clone()
    };
    if file.get(&target).is_none() {
        let names = file.names();
        bail!(
            "`{target}` is not in the file of workspaces (known: {})",
            if names.is_empty() {
                "none".to_string()
            } else {
                names.join(", ")
            }
        );
    }
    // Reach the workspace's agent by its pane, so agents herdfile did not
    // start (and so carry no herdr agent name) are reachable too.
    let pane = agent_pane(&snap, &target)?;
    let text = format!("From {sender}: {}", args.text);
    let before = if args.wait {
        read(&backend, &pane)
    } else {
        String::new()
    };
    if !args.wait && args.until.is_empty() && args.timeout.is_none() {
        // The same send as a brief's: submitted, never left in the input box.
        crate::workspaces::prompt(&backend, &pane, &pane, &text)?;
        println!("sent to {target}");
        return Ok(0);
    }
    let mut cli: Vec<&str> = vec!["agent", "prompt", &pane, &text];
    if args.wait {
        cli.push("--wait");
    }
    for u in &args.until {
        cli.extend(["--until", u]);
    }
    if let Some(t) = &args.timeout {
        cli.extend(["--timeout", t]);
    }
    let (code, _, err) = backend.cli(&cli)?;
    if code != 0 {
        // herdr's error, unchanged (agent_blocked and the rest).
        eprint!("{err}");
        return Ok(code);
    }
    if !args.wait {
        println!("sent to {target}");
        return Ok(0);
    }
    // herdr does not return the reply; read the screen and print what came
    // after the message.
    let after = read(&backend, &pane);
    println!("{}", reply_after(&before, &after, &text));
    Ok(0)
}

/// The pane running a workspace's agent: the one labelled `agent`, else the
/// first pane hosting an agent.
fn agent_pane(snap: &crate::backend::Snapshot, name: &str) -> Result<String> {
    let ws = snap
        .workspace_by_label(name)
        .ok_or_else(|| anyhow::anyhow!("workspace `{name}` is not open"))?;
    let id = &ws.workspace_id;
    snap.pane_by_label(id, crate::services::RESERVED)
        .filter(|p| p.agent.is_some())
        .or_else(|| snap.panes_of(id).find(|p| p.agent.is_some()))
        .map(|p| p.pane_id.clone())
        .ok_or_else(|| anyhow::anyhow!("no agent is running in `{name}`"))
}

fn read(backend: &dyn Backend, pane: &str) -> String {
    backend
        .cli(&[
            "agent",
            "read",
            pane,
            "--source",
            "recent-unwrapped",
            "--lines",
            "400",
        ])
        .map(|(_, out, _)| out)
        .unwrap_or_default()
}

/// What the agent wrote after our message: of the lines new since before,
/// those after the first one that shows the message (its echo).
pub fn reply_after(before: &str, after: &str, sent: &str) -> String {
    let head: String = sent.chars().take(40).collect();
    let old: std::collections::HashSet<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().filter(|l| !old.contains(l)).collect();
    let start = new
        .iter()
        .position(|l| l.contains(head.trim()))
        .map(|i| i + 1)
        .unwrap_or(0);
    new[start..].join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_is_what_follows_the_message() {
        let after = "old line\n> From a: is it approved?\nYes, approved.\nReady to merge.\n";
        assert_eq!(
            reply_after("old line\n", after, "From a: is it approved?"),
            "Yes, approved.\nReady to merge."
        );
        assert_eq!(reply_after("x\n", "x\nnew\n", "From a: gone"), "new");
        // A reply that quotes the message is still the reply.
        assert_eq!(
            reply_after("", "From a: hi\nreply to: From a: hi\n", "From a: hi"),
            "reply to: From a: hi"
        );
    }

    #[test]
    fn agent_pane_finds_unnamed_agents() {
        let snap: crate::backend::Snapshot =
            serde_json::from_str(include_str!("../tests/fixtures/snapshot.json")).unwrap();
        assert_eq!(agent_pane(&snap, "demo").unwrap(), "w1:p1");
        assert!(agent_pane(&snap, "nope").is_err());
    }
}
