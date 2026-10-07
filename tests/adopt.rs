//! Adopt: a file from what is on screen, and the first apply changes nothing.

mod common;

use common::{TestServer, SERVICES};

fn panes_of(t: &TestServer, ws: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = t.snapshot()["panes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["workspace_id"] == ws)
        .map(|p| {
            (
                p["pane_id"].as_str().unwrap().to_string(),
                p["label"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    v.sort();
    v
}

#[test]
fn adopt_agent_and_shell_then_apply_changes_nothing() {
    let t = TestServer::start();
    t.write_services(SERVICES);
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
    t.herdr(&[
        "pane",
        "split",
        &root,
        "--direction",
        "right",
        "--ratio",
        "0.65",
        "--no-focus",
    ]);

    t.ok(None, &["adopt", &ws]);
    let file = t.read_ws_file(&ws);
    assert!(file.contains("[tab.1]"), "{file}");
    assert!(file.contains(r#"{ pane = "agent", size = 65 }"#), "{file}");
    assert!(
        file.contains(r#"{ pane = "shell-1", mark = "unmanaged""#),
        "{file}"
    );
    // Unlabelled panes got their generated labels in herdr too.
    assert!(t.pane(&ws, "agent").is_some() && t.pane(&ws, "shell-1").is_some());

    let before = panes_of(&t, &ws);
    let ratio_before = t.splits_of(&ws, "agent")[0]["ratio"].as_f64().unwrap();
    let out = t.ok(None, &["apply", "-w", &ws]);
    assert!(out.contains("herdr already matches"), "{out}");
    assert_eq!(panes_of(&t, &ws), before);
    assert_eq!(
        t.splits_of(&ws, "agent")[0]["ratio"].as_f64().unwrap(),
        ratio_before
    );
    assert_eq!(t.read_ws_file(&ws), file);
}

#[test]
fn adopt_refuses_to_overwrite() {
    let t = TestServer::start();
    let ws = t.workspace("demo");
    t.ok(None, &["adopt", &ws]);
    let out = t.herdfile(None, &["adopt", &ws]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(t.ws_file(&ws).to_str().unwrap()), "{err}");
    t.ok(None, &["adopt", &ws, "--force"]);
}

#[test]
fn adopt_marks_services_managed() {
    let t = TestServer::start();
    t.write_services(SERVICES);
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
    t.herdr(&["pane", "rename", &root, "dev"]);
    t.ok(None, &["adopt", &ws]);
    assert!(t.read_ws_file(&ws).contains("row = [\"dev\"]"));
}
