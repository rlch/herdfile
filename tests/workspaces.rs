//! The file of workspaces, tell, and the "needs you" list.

mod common;

use common::TestServer;

/// A decoy workspace keeps focus away from the ones under test. Its pane
/// also proves agents started here would be the fake.
fn decoy(t: &TestServer) {
    let r = t.herdr(&[
        "workspace",
        "create",
        "--label",
        "decoy",
        "--cwd",
        "/tmp",
        "--no-focus",
    ]);
    let pane = r["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_string();
    t.require_fake_agents(&pane);
}

fn focused(t: &TestServer) -> String {
    t.snapshot()["focused_workspace_id"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

#[test]
fn add_opens_workspace_with_agent_and_brief() {
    let mut t = TestServer::start();
    decoy(&t);
    t.start_watcher();
    let before = focused(&t);
    let repo = t.repo();
    let out = t.ok(
        None,
        &[
            "ws",
            "add",
            "review-pr-312",
            "--dir",
            repo.to_str().unwrap(),
            "--purpose",
            "review PR 312",
            "--brief",
            "briefs/review-312.md",
            "--model",
            "opus",
        ],
    );
    assert!(out.contains("brief sent"), "{out}");
    let ws = t.ws_id("review-pr-312").expect("workspace opened");
    assert_eq!(focused(&t), before, "focus moved");
    assert_eq!(t.tab_names(&ws), ["main"]);
    assert_eq!(t.tab_labels(&ws, "main"), ["agent"]);
    assert!(t
        .read_ws_file(&ws)
        .contains("[tab.main]\nrow = [\"agent\"]"));
    let file = t.workspaces_file();
    assert!(file.contains("[review-pr-312]"), "{file}");
    assert!(file.contains("parent = \"operator\""), "{file}");
    assert!(
        file.contains(r#"agent = { brief = "briefs/review-312.md", model = "opus" }"#),
        "{file}"
    );
    // The agent carries the workspace's name, so plain herdr reaches it.
    let agent = t.herdr(&["agent", "get", "review-pr-312"]);
    assert_eq!(agent["result"]["agent"]["name"], "review-pr-312");
    let pane = t.pane_id(&ws, "agent");
    t.eventually("brief typed into the agent", || {
        t.read_pane(&pane)
            .contains("Read briefs/review-312.md and follow it.")
    });
    // Default start: herdr's agent start with the model filled in.
    assert!(t.read_pane(&pane).contains("fake claude --model opus"));
}

#[test]
fn add_through_a_wrapper_command() {
    let mut t = TestServer::start();
    decoy(&t);
    t.use_wrapper_command();
    t.start_watcher();
    let repo = t.repo();
    t.ok(
        None,
        &[
            "ws",
            "add",
            "wrapped",
            "--dir",
            repo.to_str().unwrap(),
            "--brief",
            "b.md",
            "--model",
            "opus",
        ],
    );
    let ws = t.ws_id("wrapped").unwrap();
    let pane = t.pane_id(&ws, "agent");
    let screen = t.read_pane(&pane);
    assert!(screen.contains("WRAPPED=1 claude --opus"), "{screen}");
    assert_eq!(
        t.herdr(&["agent", "get", "wrapped"])["result"]["agent"]["name"],
        "wrapped"
    );
    t.eventually("brief typed", || {
        t.read_pane(&pane).contains("Read b.md and follow it.")
    });
}

#[test]
fn invalid_or_taken_names_create_nothing() {
    let t = TestServer::start();
    decoy(&t);
    let repo = t.repo();
    let out = t.herdfile(
        None,
        &[
            "ws",
            "add",
            "Review PR 312",
            "--dir",
            repo.to_str().unwrap(),
        ],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("[a-z][a-z0-9_-]{0,31}"));
    t.ok(None, &["ws", "add", "one", "--dir", repo.to_str().unwrap()]);
    let n = t.snapshot()["workspaces"].as_array().unwrap().len();
    let out = t.herdfile(None, &["ws", "add", "one", "--dir", repo.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("already"));
    assert_eq!(t.snapshot()["workspaces"].as_array().unwrap().len(), n);
}

#[test]
fn worktree_removed_only_when_merged() {
    let t = TestServer::start();
    decoy(&t);
    let repo = t.git_repo();
    let wt = t.dir.join("wt-feat");
    t.ok(
        None,
        &[
            "ws",
            "add",
            "feat",
            "--dir",
            repo.to_str().unwrap(),
            "--branch",
            "feat",
            "--path",
            wt.to_str().unwrap(),
        ],
    );
    let ws = t.ws_id("feat").unwrap();
    assert!(t.read_ws_file(&ws).contains(wt.to_str().unwrap()));
    // An unmerged commit holds the removal and asks the operator.
    std::fs::write(wt.join("work.txt"), "work\n").unwrap();
    t.git(&wt, &["add", "work.txt"]);
    t.git(&wt, &["commit", "-q", "-m", "work"]);
    let out = t.ok(None, &["ws", "remove", "feat"]);
    assert!(out.contains("unmerged"), "{out}");
    assert!(t.ws_id("feat").is_some());
    assert!(t.ok(None, &["needs"]).contains("unmerged"));
    assert!(t.workspaces_file().contains("removing = true"));
    // Squash-merge it into main: now it goes, and the need clears.
    t.git(&repo, &["merge", "-q", "--squash", "feat"]);
    t.git(&repo, &["commit", "-q", "-m", "squash feat"]);
    let out = t.ok(None, &["ws", "remove", "feat"]);
    assert!(out.contains("worktree removed"), "{out}");
    t.eventually("workspace closed", || t.ws_id("feat").is_none());
    assert!(!wt.exists());
    assert!(!t.workspaces_file().contains("[feat]"));
    assert!(t.ok(None, &["needs"]).contains("nothing needs you"));
}

#[test]
fn removal_waits_for_idle_and_reparents() {
    let mut t = TestServer::start();
    decoy(&t);
    t.start_watcher();
    let repo = t.repo();
    let r = repo.to_str().unwrap();
    t.ok(
        None,
        &["ws", "add", "land-prs", "--dir", r, "--purpose", "land PRs"],
    );
    t.ok(
        None,
        &["ws", "add", "review", "--dir", r, "--parent", "land-prs"],
    );
    let tree = t.ok(None, &["tree"]);
    assert!(tree.contains("└─ land-prs  [no agent]  land PRs"), "{tree}");
    assert!(tree.contains("   └─ review  [no agent]"), "{tree}");

    let ws = t.ws_id("land-prs").unwrap();
    let agent = t.pane_id(&ws, "agent");
    t.report_agent(&agent, "working");
    let out = t.ok(None, &["ws", "remove", "land-prs"]);
    assert!(out.contains("waiting for `agent` to go idle"), "{out}");
    assert!(t.ws_id("land-prs").is_some());
    t.report_agent(&agent, "idle");
    t.eventually("removed once idle", || t.ws_id("land-prs").is_none());
    t.eventually("entry dropped", || {
        !t.workspaces_file().contains("[land-prs]")
    });
    assert!(t.workspaces_file().contains("[review]\ndir"));
    assert!(t.workspaces_file().contains("parent = \"operator\""));
    // The folder is never deleted.
    assert!(repo.is_dir());
}

#[test]
fn hand_changes_to_workspaces_are_written_back() {
    let mut t = TestServer::start();
    decoy(&t);
    let repo = t.repo();
    t.ok(
        None,
        &["ws", "add", "mine", "--dir", repo.to_str().unwrap()],
    );
    t.start_watcher();
    // Opened outside herdfile: recorded as unmanaged.
    t.herdr(&[
        "workspace",
        "create",
        "--label",
        "elsewhere",
        "--cwd",
        "/tmp",
        "--no-focus",
    ]);
    t.eventually("recorded", || t.workspaces_file().contains("[elsewhere]"));
    assert!(t.workspaces_file().contains("unmanaged = true"));
    // Closed by hand: dropped, not reopened.
    let ws = t.ws_id("mine").unwrap();
    t.herdr(&["workspace", "close", &ws]);
    t.eventually("dropped", || !t.workspaces_file().contains("[mine]"));
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert!(t.ws_id("mine").is_none());
}

#[test]
fn first_sight_records_every_workspace_and_closes_nothing() {
    let mut t = TestServer::start();
    decoy(&t);
    t.herdr(&[
        "workspace",
        "create",
        "--label",
        "other",
        "--cwd",
        "/tmp",
        "--no-focus",
    ]);
    let panes = t.snapshot()["panes"].as_array().unwrap().len();
    t.start_watcher();
    t.eventually("workspaces recorded", || {
        let f = t.workspaces_file();
        f.contains("[decoy]") && f.contains("[other]")
    });
    assert!(t.workspaces_file().contains("unmanaged = true"));
    t.eventually("workspace files written", || {
        t.ws_file("w1").exists() && t.ws_file("w2").exists()
    });
    std::thread::sleep(std::time::Duration::from_millis(1200));
    assert_eq!(t.snapshot()["panes"].as_array().unwrap().len(), panes);
}

#[test]
fn tell_parent_and_wait_for_reply() {
    let mut t = TestServer::start();
    decoy(&t);
    t.start_watcher();
    let r = t.repo();
    let r = r.to_str().unwrap();
    t.ok(
        None,
        &["ws", "add", "land-prs", "--dir", r, "--model", "opus"],
    );
    t.ok(
        None,
        &[
            "ws",
            "add",
            "review-pr-312",
            "--dir",
            r,
            "--parent",
            "land-prs",
            "--model",
            "opus",
        ],
    );
    let child = t.ws_id("review-pr-312").unwrap();
    let parent = t.ws_id("land-prs").unwrap();

    t.ok(
        Some(&child),
        &["tell", "parent", "approved, ready to merge"],
    );
    let pane = t.pane_id(&parent, "agent");
    t.eventually("parent got it", || {
        t.read_pane(&pane)
            .contains("From review-pr-312: approved, ready to merge")
    });

    let out = t.ok(
        Some(&parent),
        &[
            "tell",
            "review-pr-312",
            "--wait",
            "--timeout",
            "20000",
            "is it approved?",
        ],
    );
    assert!(
        out.contains("reply to: From land-prs: is it approved?"),
        "{out}"
    );

    // Unknown names list the known ones.
    let out = t.herdfile(Some(&parent), &["tell", "nobody", "hi"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("land-prs") && err.contains("review-pr-312"),
        "{err}"
    );
}

#[test]
fn blocked_agent_refuses_and_lands_on_needs() {
    let mut t = TestServer::start();
    decoy(&t);
    t.start_watcher();
    let r = t.repo();
    t.ok(
        None,
        &["ws", "add", "land-prs", "--dir", r.to_str().unwrap()],
    );
    let ws = t.ws_id("land-prs").unwrap();
    let pane = t.pane_id(&ws, "agent");
    t.report_agent(&pane, "blocked");
    t.herdr(&["agent", "rename", &pane, "land-prs"]);
    let out = t.herdfile(None, &["tell", "land-prs", "hello"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("agent_blocked"));
    assert!(!t.read_pane(&pane).contains("hello"));
    t.eventually("blocked agent on needs", || {
        t.ok(None, &["needs"]).contains("land-prs")
    });
    t.report_agent(&pane, "idle");
    t.eventually("cleared once answered", || {
        t.ok(None, &["needs"]).contains("nothing needs you")
    });
}

#[test]
fn ask_and_done() {
    let t = TestServer::start();
    let out = t.ok(None, &["ask", "ship the migration tonight?"]);
    let id = out.split(['(', ')']).nth(1).unwrap().to_string();
    let list = t.ok(None, &["needs"]);
    assert!(
        list.contains("ship the migration tonight?") && list.contains(&id),
        "{list}"
    );
    let line = std::fs::read_to_string(t.state().join("needs.jsonl")).unwrap();
    let v: serde_json::Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
    assert_eq!(v["source"], "ask");
    assert!(v["time"].as_str().unwrap().ends_with('Z'));
    t.ok(None, &["needs", "done", &id]);
    assert!(t.ok(None, &["needs"]).contains("nothing needs you"));
}

#[test]
fn primary_with_linked_worktrees_is_never_group_closed() {
    let t = TestServer::start();
    decoy(&t);
    let repo = t.git_repo();
    t.ok(
        None,
        &["ws", "add", "primary", "--dir", repo.to_str().unwrap()],
    );
    let wt = t.dir.join("wt-side");
    t.herdr(&[
        "worktree",
        "create",
        "--cwd",
        repo.to_str().unwrap(),
        "--branch",
        "side",
        "--path",
        wt.to_str().unwrap(),
        "--label",
        "side",
        "--no-focus",
    ]);
    let out = t.ok(None, &["ws", "remove", "primary"]);
    assert!(out.contains("linked worktree"), "{out}");
    assert!(t.ws_id("primary").is_some(), "primary was closed");
    assert!(t.ws_id("side").is_some(), "the linked worktree was closed");
    assert!(t.ok(None, &["needs"]).contains("linked worktree"));
}
