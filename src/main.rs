//! herdfile: declarative workspaces for herdr.

mod adopt;
mod apply;
mod backend;
mod commands;
mod control;
mod layout;
mod live;
mod lock;
mod needs;
mod paths;
mod services;
mod tell;
mod time;
mod watch;
mod workspaces;
mod writeback;
mod wsfile;

use clap::{Args, Parser, Subcommand};

use crate::commands::{resolve_workspace, Anchor, Wait};
use crate::layout::Mark;

#[derive(Parser)]
#[command(name = "herdfile", version, about = "Declarative workspaces for herdr")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Args)]
struct WsArg {
    /// herdr workspace id (default: $HERDR_WORKSPACE_ID)
    #[arg(long, short = 'w')]
    workspace: Option<String>,
}

#[derive(Args)]
struct WaitArg {
    /// Return once the file is written, without waiting for herdr
    #[arg(long)]
    no_wait: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the watcher (in the foreground unless --detach)
    Watch {
        /// Start it in the background and return
        #[arg(long)]
        detach: bool,
    },
    /// Is the watcher running?
    Status,
    /// Print the workspace file's path
    Path {
        /// herdr workspace id (default: $HERDR_WORKSPACE_ID)
        workspace: Option<String>,
    },
    /// Print the workspace file
    Show {
        #[command(flatten)]
        ws: WsArg,
    },
    /// Add a pane to the file, or move it if already there
    Place {
        /// Service name, or `agent`
        name: String,
        /// Tab to put it in (created if missing)
        #[arg(long)]
        tab: Option<String>,
        #[arg(long, value_name = "NAME", group = "anchor")]
        right_of: Option<String>,
        #[arg(long, value_name = "NAME", group = "anchor")]
        left_of: Option<String>,
        #[arg(long, value_name = "NAME", group = "anchor")]
        above: Option<String>,
        #[arg(long, value_name = "NAME", group = "anchor")]
        below: Option<String>,
        /// Next after NAME in its row or column
        #[arg(long, value_name = "NAME", group = "anchor")]
        after: Option<String>,
        /// Just before NAME in its row or column
        #[arg(long, value_name = "NAME", group = "anchor")]
        before: Option<String>,
        /// Percent of its row or column
        #[arg(long, value_parser = clap::value_parser!(u8).range(1..=99))]
        size: Option<u8>,
        #[command(flatten)]
        ws: WsArg,
        #[command(flatten)]
        wait: WaitArg,
    },
    /// Take a pane out of the file (it is closed; busy agents once idle)
    Remove {
        name: String,
        #[command(flatten)]
        ws: WsArg,
        #[command(flatten)]
        wait: WaitArg,
    },
    /// Replace a tab's whole layout, e.g. 'row = ["agent", { column = ["test", "dev"] }]'
    Set {
        tab: String,
        tree: String,
        #[command(flatten)]
        ws: WsArg,
        #[command(flatten)]
        wait: WaitArg,
    },
    /// Change a pane's ownership mark
    Mark {
        name: String,
        #[arg(value_parser = ["managed", "mine", "unmanaged"])]
        mark: String,
        #[command(flatten)]
        ws: WsArg,
        #[command(flatten)]
        wait: WaitArg,
    },
    /// Apply the workspace file now
    Apply {
        #[command(flatten)]
        ws: WsArg,
    },
    /// Write a workspace file from what is on screen
    Adopt {
        /// herdr workspace id (default: $HERDR_WORKSPACE_ID)
        workspace: Option<String>,
        /// Overwrite an existing file
        #[arg(long)]
        force: bool,
        /// Every open workspace: into the file of workspaces as unmanaged,
        /// and a workspace file for each that has none. Closes nothing.
        #[arg(long, conflicts_with_all = ["workspace", "force"])]
        all: bool,
    },
    /// The file of workspaces: add or remove a workspace
    #[command(subcommand)]
    Ws(WsCmd),
    /// Print the workspaces as a tree, with live agent status
    Tree,
    /// Prompt another workspace's agent, prefixed with your name
    Tell {
        /// Workspace name, or `parent`
        target: String,
        text: String,
        /// Wait for the agent to settle, then print its reply
        #[arg(long)]
        wait: bool,
        /// Passed to `herdr agent prompt --until`
        #[arg(long)]
        until: Vec<String>,
        /// Milliseconds, passed to `herdr agent prompt --timeout`
        #[arg(long)]
        timeout: Option<String>,
    },
    /// Things waiting on the operator
    Needs {
        #[command(subcommand)]
        cmd: Option<NeedsCmd>,
    },
    /// Put a question on the "needs you" list
    Ask { question: String },
}

#[derive(Subcommand)]
enum WsCmd {
    /// Open a workspace (a herdr worktree with --branch) and start its agent
    Add {
        /// Workspace name, also its herdr label and agent name ([a-z][a-z0-9_-]{0,31})
        name: String,
        /// The folder (a path; `~` allowed)
        #[arg(long)]
        dir: String,
        /// Open a herdr worktree on this branch
        #[arg(long)]
        branch: Option<String>,
        /// The branch's base (default: herdr's)
        #[arg(long, requires = "branch")]
        base: Option<String>,
        /// Where to put the worktree (default: herdr's)
        #[arg(long, requires = "branch")]
        path: Option<String>,
        /// Parent workspace, or `operator` (default: the calling workspace)
        #[arg(long)]
        parent: Option<String>,
        #[arg(long)]
        purpose: Option<String>,
        /// A file the agent is told to read and follow
        #[arg(long)]
        brief: Option<String>,
        /// Fills {model} in the configured agent command
        #[arg(long)]
        model: Option<String>,
    },
    /// Remove a workspace once its agent is idle and its branch merged
    Remove { name: String },
}

#[derive(Subcommand)]
enum NeedsCmd {
    /// Clear an entry
    Done { id: String },
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli.command) {
        eprintln!("herdfile: {e:#}");
        std::process::exit(1);
    }
}

fn run(cmd: Cmd) -> anyhow::Result<()> {
    match cmd {
        Cmd::Watch { detach: true } => watch::detach(),
        Cmd::Watch { detach: false } => watch::run(),
        Cmd::Status => commands::status(),
        Cmd::Path { workspace } => commands::path(&resolve_workspace(workspace)?),
        Cmd::Show { ws } => commands::show(&resolve_workspace(ws.workspace)?),
        Cmd::Place {
            name,
            tab,
            right_of,
            left_of,
            above,
            below,
            after,
            before,
            size,
            ws,
            wait,
        } => {
            let anchor = [
                (Anchor::RightOf, right_of),
                (Anchor::LeftOf, left_of),
                (Anchor::Above, above),
                (Anchor::Below, below),
                (Anchor::After, after),
                (Anchor::Before, before),
            ]
            .into_iter()
            .find_map(|(kind, name)| name.map(|n| (kind, n)));
            commands::place(
                &resolve_workspace(ws.workspace)?,
                &name,
                tab,
                anchor,
                size,
                Wait {
                    no_wait: wait.no_wait,
                },
            )
        }
        Cmd::Remove { name, ws, wait } => commands::remove(
            &resolve_workspace(ws.workspace)?,
            &name,
            Wait {
                no_wait: wait.no_wait,
            },
        ),
        Cmd::Set {
            tab,
            tree,
            ws,
            wait,
        } => commands::set(
            &resolve_workspace(ws.workspace)?,
            &tab,
            &tree,
            Wait {
                no_wait: wait.no_wait,
            },
        ),
        Cmd::Mark {
            name,
            mark,
            ws,
            wait,
        } => commands::mark(
            &resolve_workspace(ws.workspace)?,
            &name,
            Mark::parse(&mark).expect("validated by clap"),
            Wait {
                no_wait: wait.no_wait,
            },
        ),
        Cmd::Apply { ws } => commands::apply_now(&resolve_workspace(ws.workspace)?),
        Cmd::Adopt { all: true, .. } => workspaces::adopt_all(),
        Cmd::Adopt {
            workspace, force, ..
        } => adopt::adopt(&resolve_workspace(workspace)?, force),
        Cmd::Ws(WsCmd::Add {
            name,
            dir,
            branch,
            base,
            path,
            parent,
            purpose,
            brief,
            model,
        }) => workspaces::add(workspaces::AddArgs {
            name,
            dir,
            branch,
            base,
            path,
            parent,
            purpose,
            brief,
            model,
        }),
        Cmd::Ws(WsCmd::Remove { name }) => workspaces::remove(&name),
        Cmd::Tree => workspaces::tree(),
        Cmd::Tell {
            target,
            text,
            wait,
            until,
            timeout,
        } => {
            let code = tell::tell(tell::TellArgs {
                target,
                text,
                wait,
                until,
                timeout,
            })?;
            if code != 0 {
                std::process::exit(code);
            }
            Ok(())
        }
        Cmd::Needs { cmd: None } => {
            needs::print();
            Ok(())
        }
        Cmd::Needs {
            cmd: Some(NeedsCmd::Done { id }),
        } => needs::done(&id),
        Cmd::Ask { question } => needs::ask(&question),
    }
}
