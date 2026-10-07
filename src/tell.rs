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
    let text = format!("From {sender}: {}", args.text);
    let mut cli: Vec<&str> = vec!["agent", "prompt", &target, &text];
    if args.wait {
        cli.push("--wait");
    }
    for u in &args.until {
        cli.extend(["--until", u]);
    }
    if let Some(t) = &args.timeout {
        cli.extend(["--timeout", t]);
    }
    let (code, out, err) = backend.cli(&cli)?;
    print!("{out}");
    eprint!("{err}");
    if code != 0 {
        return Ok(code);
    }
    if args.wait {
        // herdr does not return the reply; read it.
        let (code, out, err) = backend.cli(&[
            "agent",
            "read",
            &target,
            "--source",
            "recent-unwrapped",
            "--lines",
            "120",
        ])?;
        print!("{out}");
        eprint!("{err}");
        return Ok(code);
    }
    Ok(0)
}
