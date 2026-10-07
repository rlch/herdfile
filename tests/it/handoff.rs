//! Handoff (an agent replaces itself in place) and a spinoff round trip,
//! end to end with the fake agent.

use crate::harness::{TestServer, SERVICES};

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

/// Run a herdfile command as the agent in `pane` would: inside its
/// workspace and pane.
fn as_agent(t: &TestServer, ws: &str, pane: &str, args: &[&str]) -> std::process::Output {
    t.herdfile_cmd(Some(ws), args)
        .env("HERDR_PANE_ID", pane)
        .output()
        .unwrap()
}

fn agent_pane_named(t: &TestServer, name: &str) -> Option<String> {
    let out = t.try_herdr(&["agent", "get", name]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    v.pointer("/result/agent/pane_id")?
        .as_str()
        .map(str::to_string)
}

#[test]
fn handoff_replaces_the_agent_in_place() {
    let mut t = TestServer::start();
    decoy(&t);
    t.start_watcher();
    let repo = t.repo();
    t.ok(
        None,
        &[
            "ws",
            "add",
            "solo",
            "--dir",
            repo.to_str().unwrap(),
            "--model",
            "opus",
        ],
    );
    let ws = t.ws_id("solo").unwrap();
    let old = t.pane_id(&ws, "agent");
    t.settle();
    let file = t.read_ws_file(&ws);
    let log_before = t.log();

    let out = as_agent(
        &t,
        &ws,
        &old,
        &["handoff", "--brief", "next.md", "--model", "opus"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("handed off to"));

    assert!(t.snapshot()["panes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|p| p["pane_id"] != old.as_str()));
    let new = t.pane_id(&ws, "agent");
    assert_ne!(new, old);
    assert_eq!(
        agent_pane_named(&t, "solo").as_deref(),
        Some(new.as_str()),
        "name moved over"
    );
    t.eventually("brief typed", || {
        t.read_pane(&new).contains("Read next.md and follow it.")
    });
    assert!(t.read_pane(&new).contains("fake claude --model opus"));
    assert_eq!(t.tab_labels(&ws, "main"), ["agent"]);
    t.settle();
    // The watcher saw nothing to fix or record.
    assert_eq!(t.read_ws_file(&ws), file);
    let log = t.log();
    let after = &log[log_before.len()..];
    for word in ["recorded", "dropped", "layout taken", "opened", "closed"] {
        assert!(!after.contains(word), "{after}");
    }
}

#[test]
fn handoff_keeps_the_agents_place_among_other_panes() {
    let mut t = TestServer::start();
    decoy(&t);
    t.write_services(SERVICES);
    t.start_watcher();
    let repo = t.repo();
    t.ok(
        None,
        &[
            "ws",
            "add",
            "solo",
            "--dir",
            repo.to_str().unwrap(),
            "--model",
            "opus",
        ],
    );
    let ws = t.ws_id("solo").unwrap();
    t.ok(
        Some(&ws),
        &["place", "test", "--right-of", "agent", "--size", "30"],
    );
    t.ok(Some(&ws), &["place", "dev", "--left-of", "agent"]);
    let old = t.pane_id(&ws, "agent");
    let test_term = t.pane(&ws, "test").unwrap()["terminal_id"].clone();
    let ratios = |t: &TestServer| -> Vec<i64> {
        t.splits_of(&ws, "agent")
            .iter()
            .map(|s| (s["ratio"].as_f64().unwrap() * 100.0).round() as i64)
            .collect()
    };
    let before = ratios(&t);
    t.settle();
    let file = t.read_ws_file(&ws);

    let out = as_agent(&t, &ws, &old, &["handoff", "--brief", "next.md"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(t.tab_labels(&ws, "main"), ["dev", "agent", "test"]);
    assert_eq!(t.pane(&ws, "test").unwrap()["terminal_id"], test_term);
    assert_eq!(ratios(&t), before, "a divider moved");
    t.settle();
    assert_eq!(t.read_ws_file(&ws), file);
    assert_eq!(t.tab_labels(&ws, "main"), ["dev", "agent", "test"]);
}

#[test]
fn failed_successor_leaves_the_old_agent() {
    let mut t = TestServer::start();
    decoy(&t);
    t.start_watcher();
    let repo = t.repo();
    t.ok(
        None,
        &[
            "ws",
            "add",
            "solo",
            "--dir",
            repo.to_str().unwrap(),
            "--model",
            "opus",
        ],
    );
    let ws = t.ws_id("solo").unwrap();
    let old = t.pane_id(&ws, "agent");
    // An agent kind herdr does not know: the successor cannot start.
    std::fs::create_dir_all(t.dir.join("herdfile")).unwrap();
    std::fs::write(
        t.dir.join("herdfile/config.toml"),
        "[agent]\nkind = \"nope\"\n",
    )
    .unwrap();

    let out = as_agent(&t, &ws, &old, &["handoff", "--brief", "next.md"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("nothing was handed off"));
    assert_eq!(t.pane_id(&ws, "agent"), old);
    assert_eq!(agent_pane_named(&t, "solo").as_deref(), Some(old.as_str()));
    assert_eq!(t.tab_labels(&ws, "main"), ["agent"]);
}

#[test]
fn handoff_from_an_agent_herdfile_did_not_start() {
    let mut t = TestServer::start();
    decoy(&t);
    let r = t.herdr(&[
        "workspace",
        "create",
        "--label",
        "byhand",
        "--cwd",
        "/tmp",
        "--no-focus",
    ]);
    let ws = r["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let old = r["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_string();
    t.herdr(&["pane", "run", &old, "claude"]);
    t.eventually("agent detected", || {
        t.snapshot()["panes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["pane_id"] == old.as_str() && p["agent_status"] == "idle")
    });
    t.start_watcher();
    t.eventually("recorded", || t.read_ws_file(&ws).contains("agent"));

    let out = as_agent(&t, &ws, &old, &["handoff", "--brief", "next.md"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let new = t.pane_id(&ws, "agent");
    assert_eq!(
        agent_pane_named(&t, "byhand").as_deref(),
        Some(new.as_str())
    );
    t.eventually("brief typed", || {
        t.read_pane(&new).contains("Read next.md and follow it.")
    });
}

#[test]
fn spinoff_round_trip() {
    let mut t = TestServer::start();
    decoy(&t);
    t.start_watcher();
    let repo = t.repo();
    let r = repo.to_str().unwrap();
    t.ok(
        None,
        &[
            "ws",
            "add",
            "orch",
            "--dir",
            r,
            "--purpose",
            "orchestrate",
            "--model",
            "opus",
        ],
    );
    let orch = t.ws_id("orch").unwrap();
    let orch_pane = t.pane_id(&orch, "agent");
    // The orchestrator spins off a child: its parent is the caller.
    let out = as_agent(
        &t,
        &orch,
        &orch_pane,
        &[
            "ws",
            "add",
            "child",
            "--dir",
            r,
            "--purpose",
            "do one thing",
            "--brief",
            "c.md",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let child = t.ws_id("child").unwrap();
    let child_pane = t.pane_id(&child, "agent");
    t.eventually("child got its brief", || {
        t.read_pane(&child_pane).contains("Read c.md")
    });
    let tree = t.ok(None, &["tree"]);
    assert!(tree.contains("─ orch  [idle]  orchestrate"), "{tree}");
    assert!(tree.contains("│  └─ child"), "{tree}");
    // The child reports back.
    let out = as_agent(&t, &child, &child_pane, &["tell", "parent", "done, merged"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    t.eventually("parent heard", || {
        t.read_pane(&orch_pane).contains("From child: done, merged")
    });
    // The orchestrator cleans up once the child is idle.
    let out = as_agent(&t, &orch, &orch_pane, &["ws", "remove", "child"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    t.eventually("child closed", || t.ws_id("child").is_none());
    t.eventually("entry dropped", || !t.workspaces_file().contains("[child]"));
    assert!(t.ws_id("orch").is_some());
}

#[test]
fn a_shell_cannot_be_handed_off() {
    let t = TestServer::start();
    decoy(&t);
    let r = t.herdr(&[
        "workspace",
        "create",
        "--label",
        "plain",
        "--cwd",
        "/tmp",
        "--no-focus",
    ]);
    let ws = r["result"]["workspace"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let pane = r["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_string();
    let out = as_agent(&t, &ws, &pane, &["handoff", "--brief", "next.md"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not running an agent"));
    assert_eq!(
        t.snapshot()["panes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["workspace_id"] == ws.as_str())
            .count(),
        1
    );
}

#[test]
fn an_agent_hands_itself_off_from_its_own_pane() {
    let mut t = TestServer::start();
    decoy(&t);
    t.start_watcher();
    let repo = t.repo();
    t.ok(
        None,
        &[
            "ws",
            "add",
            "solo",
            "--dir",
            repo.to_str().unwrap(),
            "--model",
            "opus",
        ],
    );
    let ws = t.ws_id("solo").unwrap();
    let old = t.pane_id(&ws, "agent");
    let bin = env!("CARGO_BIN_EXE_herdfile");
    let log = t.dir.join("handoff.out");
    // The agent runs the command itself; closing its pane ends it mid-run.
    t.herdr(&[
        "agent",
        "prompt",
        &old,
        &format!("!{bin} handoff --brief next.md > {} 2>&1", log.display()),
    ]);
    t.eventually("old pane closed", || {
        t.snapshot()["panes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["pane_id"] != old.as_str())
    });
    let new = t.pane_id(&ws, "agent");
    assert_eq!(agent_pane_named(&t, "solo").as_deref(), Some(new.as_str()));
    t.eventually("brief typed", || {
        t.read_pane(&new).contains("Read next.md and follow it.")
    });
    assert!(std::fs::read_to_string(&log)
        .unwrap()
        .contains("handed off to"));
    // The lock died with the old agent: commands work at once.
    let out = t.ok(Some(&ws), &["show"]);
    assert!(out.contains("[tab.main]"), "{out}");
    t.settle();
    assert_eq!(t.tab_labels(&ws, "main"), ["agent"]);
}
