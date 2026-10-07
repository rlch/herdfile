//! `herdfile plan`: what a pass would do, with nothing changed.

use crate::harness::{TestServer, SERVICES};

#[test]
fn plan_shows_first_sight_and_changes_nothing() {
    let t = TestServer::start();
    let ws = t.workspace("demo");
    let root = t.snapshot()["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["workspace_id"] == ws.as_str())
        .unwrap()["pane_id"]
        .as_str()
        .unwrap()
        .to_string();
    t.report_agent(&root, "idle");
    t.herdr(&["pane", "split", &root, "--direction", "right", "--no-focus"]);
    let before = t.snapshot();
    let out = t.ok(None, &["plan"]);
    assert!(out.contains(&format!("{ws} demo:")), "{out}");
    assert!(out.contains("record: agent, shell-1"), "{out}");
    assert!(out.contains("would only be labelled and recorded"), "{out}");
    // Nothing changed: no labels in herdr, no files.
    assert_eq!(t.snapshot()["panes"], before["panes"]);
    assert!(!t.ws_file(&ws).exists());
    assert!(!t.state().join("workspaces.toml").exists());
}

#[test]
fn plan_shows_opens_and_closes() {
    let t = TestServer::start();
    t.write_services(SERVICES);
    let ws = t.workspace("demo");
    let snap = t.snapshot();
    let root = snap["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["workspace_id"] == ws.as_str())
        .unwrap()["pane_id"]
        .as_str()
        .unwrap()
        .to_string();
    t.herdr(&["pane", "rename", &root, "agent"]);
    let tab = t.tab_id(&ws, "1");
    t.herdr(&["tab", "rename", &tab, "main"]);
    t.write_ws_file(
        &ws,
        &format!(
            "dir = \"{}\"\n\n[tab.main]\nrow = [\"agent\", \"test\"]\n",
            t.repo().display()
        ),
    );
    let out = t.ok(None, &["plan", &ws]);
    assert!(out.contains("open test in tab main"), "{out}");
    assert!(out.contains("1 would see panes opened"), "{out}");
    assert!(t.pane(&ws, "test").is_none());
}
