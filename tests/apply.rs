//! Apply: herdr converges to the workspace file. One test per spec scenario,
//! each against its own throwaway herdr server.

mod common;

use common::{TestServer, SERVICES};

/// A workspace with tab `main` holding the agent pane, and a file saying so.
fn setup(t: &mut TestServer) -> String {
    t.write_services(SERVICES);
    let ws = t.workspace("demo");
    let snap = t.snapshot();
    let tab = snap["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["workspace_id"] == ws.as_str())
        .unwrap()["tab_id"]
        .as_str()
        .unwrap()
        .to_string();
    let root = snap["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["workspace_id"] == ws.as_str())
        .unwrap()["pane_id"]
        .as_str()
        .unwrap()
        .to_string();
    t.herdr(&["tab", "rename", &tab, "main"]);
    t.herdr(&["pane", "rename", &root, "agent"]);
    t.write_ws_file(
        &ws,
        &format!(
            "dir = \"{}\"\n\n[tab.main]\nrow = [\"agent\"]\n",
            t.repo().display()
        ),
    );
    t.start_watcher();
    ws
}

fn focused_tab(t: &TestServer) -> String {
    t.snapshot()["focused_tab_id"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

#[test]
fn place_and_wait_opens_in_background_tab() {
    let mut t = TestServer::start();
    let ws = setup(&mut t);
    let before = focused_tab(&t);
    let out = t.ok(Some(&ws), &["place", "dev", "--tab", "services"]);
    assert!(out.contains("opened: dev"), "{out}");
    assert!(out.contains("dev is ready"), "{out}");
    assert_eq!(t.tab_labels(&ws, "services"), ["dev"]);
    assert_eq!(t.tab_names(&ws), ["main", "services"]);
    // Missing pane is opened in the service's cwd and its cmd runs.
    let dev = t.pane(&ws, "dev").unwrap();
    assert_eq!(dev["cwd"].as_str().unwrap(), t.repo().to_str().unwrap());
    // Never take focus.
    assert_eq!(focused_tab(&t), before);
    assert!(t
        .read_ws_file(&ws)
        .contains("[tab.services]\nrow = [\"dev\"]"));
}

#[test]
fn replace_a_pane() {
    let mut t = TestServer::start();
    let ws = setup(&mut t);
    t.ok(Some(&ws), &["set", "main", r#"row = ["agent", "test"]"#]);
    assert_eq!(t.tab_labels(&ws, "main"), ["agent", "test"]);
    let out = t.ok(Some(&ws), &["set", "main", r#"row = ["agent", "logs"]"#]);
    assert!(
        out.contains("closed: test") && out.contains("opened: logs"),
        "{out}"
    );
    assert_eq!(t.tab_labels(&ws, "main"), ["agent", "logs"]);
    assert!(t.pane(&ws, "test").is_none());
}

#[test]
fn move_between_tabs_keeps_process() {
    let mut t = TestServer::start();
    let ws = setup(&mut t);
    t.ok(Some(&ws), &["place", "dev", "--tab", "services"]);
    let term = t.pane(&ws, "dev").unwrap()["terminal_id"].clone();
    let out = t.ok(Some(&ws), &["place", "dev", "--tab", "main"]);
    assert!(out.contains("moved: dev"), "{out}");
    assert_eq!(t.tab_labels(&ws, "main"), ["agent", "dev"]);
    assert_eq!(t.pane(&ws, "dev").unwrap()["terminal_id"], term);
    // The emptied tab is gone, and the file no longer lists it.
    assert_eq!(t.tab_names(&ws), ["main"]);
    assert!(!t.read_ws_file(&ws).contains("services"));
}

#[test]
fn stack_two_panes_on_the_right_and_resize() {
    let mut t = TestServer::start();
    let ws = setup(&mut t);
    t.ok(Some(&ws), &["place", "test", "--right-of", "agent"]);
    let agent_term = t.pane(&ws, "agent").unwrap()["terminal_id"].clone();
    let test_term = t.pane(&ws, "test").unwrap()["terminal_id"].clone();
    t.ok(Some(&ws), &["place", "dev", "--below", "test"]);
    assert_eq!(t.tab_labels(&ws, "main"), ["agent", "test", "dev"]);
    assert!(t
        .read_ws_file(&ws)
        .contains(r#"row = ["agent", { column = ["test", "dev"] }]"#));
    let splits = t.splits_of(&ws, "dev");
    assert!(splits.iter().any(|s| s["direction"] == "down"));
    assert_eq!(t.pane(&ws, "agent").unwrap()["terminal_id"], agent_term);
    assert_eq!(t.pane(&ws, "test").unwrap()["terminal_id"], test_term);

    // Resize: agent to 70% of the row, nothing restarted.
    t.ok(
        Some(&ws),
        &[
            "set",
            "main",
            r#"row = [{ pane = "agent", size = 70 }, { column = ["test", "dev"] }]"#,
        ],
    );
    let root = t
        .splits_of(&ws, "agent")
        .into_iter()
        .find(|s| s["id"].as_str().unwrap().ends_with("_root"))
        .unwrap();
    assert!(
        (root["ratio"].as_f64().unwrap() - 0.7).abs() < 0.01,
        "{root}"
    );
    assert_eq!(t.pane(&ws, "agent").unwrap()["terminal_id"], agent_term);
}

#[test]
fn reorder_by_swapping() {
    let mut t = TestServer::start();
    let ws = setup(&mut t);
    t.ok(
        Some(&ws),
        &["set", "main", r#"row = ["agent", "test", "dev"]"#],
    );
    let dev_term = t.pane(&ws, "dev").unwrap()["terminal_id"].clone();
    t.ok(
        Some(&ws),
        &["set", "main", r#"row = ["agent", "dev", "test"]"#],
    );
    assert_eq!(t.tab_labels(&ws, "main"), ["agent", "dev", "test"]);
    assert_eq!(t.pane(&ws, "dev").unwrap()["terminal_id"], dev_term);
}

#[test]
fn rebuild_into_a_new_shape() {
    let mut t = TestServer::start();
    let ws = setup(&mut t);
    t.ok(
        Some(&ws),
        &["set", "main", r#"row = ["agent", "test", "dev"]"#],
    );
    let terms: Vec<_> = ["agent", "test", "dev"]
        .iter()
        .map(|l| t.pane(&ws, l).unwrap()["terminal_id"].clone())
        .collect();
    t.ok(
        Some(&ws),
        &[
            "set",
            "main",
            r#"column = [{ row = ["dev", "agent"], size = 70 }, "test"]"#,
        ],
    );
    assert_eq!(t.tab_labels(&ws, "main"), ["dev", "agent", "test"]);
    for (l, term) in ["agent", "test", "dev"].iter().zip(terms) {
        assert_eq!(
            t.pane(&ws, l).unwrap()["terminal_id"],
            term,
            "{l} restarted"
        );
    }
    // The staging tab is gone.
    assert_eq!(t.tab_names(&ws), ["main"]);
}

#[test]
fn remove_a_service() {
    let mut t = TestServer::start();
    let ws = setup(&mut t);
    t.ok(Some(&ws), &["place", "test", "--right-of", "agent"]);
    let out = t.ok(Some(&ws), &["remove", "test"]);
    assert!(out.contains("closed: test"), "{out}");
    assert!(t.pane(&ws, "test").is_none());
    assert_eq!(t.tab_labels(&ws, "main"), ["agent"]);
}

#[test]
fn busy_agent_waits_for_idle() {
    let mut t = TestServer::start();
    let ws = setup(&mut t);
    let agent = t.pane_id(&ws, "agent");
    t.ok(Some(&ws), &["place", "test", "--right-of", "agent"]);
    t.report_agent(&agent, "working");
    let out = t.ok(Some(&ws), &["remove", "agent"]);
    assert!(out.contains("marked for removal once idle: agent"), "{out}");
    assert!(t.pane(&ws, "agent").is_some());
    t.report_agent(&agent, "idle");
    t.eventually("agent closed once idle", || t.pane(&ws, "agent").is_none());
}

#[test]
fn readded_before_idle_cancels_removal() {
    let mut t = TestServer::start();
    let ws = setup(&mut t);
    let agent = t.pane_id(&ws, "agent");
    t.ok(Some(&ws), &["place", "test", "--right-of", "agent"]);
    t.report_agent(&agent, "working");
    t.ok(Some(&ws), &["remove", "agent"]);
    t.ok(Some(&ws), &["place", "agent", "--left-of", "test"]);
    t.report_agent(&agent, "idle");
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert!(t.pane(&ws, "agent").is_some());
}

#[test]
fn unmanaged_pane_is_never_closed() {
    let mut t = TestServer::start();
    let ws = setup(&mut t);
    let agent = t.pane_id(&ws, "agent");
    // An outside split, recorded as unmanaged.
    t.herdr(&[
        "pane",
        "split",
        &agent,
        "--direction",
        "right",
        "--no-focus",
    ]);
    t.eventually("shell recorded", || t.read_ws_file(&ws).contains("shell-1"));
    assert!(t.read_ws_file(&ws).contains(r#"mark = "unmanaged""#));
    t.ok(Some(&ws), &["remove", "shell-1"]);
    assert!(
        t.pane(&ws, "shell-1").is_some(),
        "unmanaged pane was closed"
    );
    assert!(!t.read_ws_file(&ws).contains("shell-1"));
}

#[test]
fn mine_pane_is_never_closed() {
    let mut t = TestServer::start();
    let ws = setup(&mut t);
    let agent = t.pane_id(&ws, "agent");
    t.herdr(&["pane", "split", &agent, "--direction", "down", "--no-focus"]);
    t.eventually("shell recorded", || t.read_ws_file(&ws).contains("shell-1"));
    t.ok(Some(&ws), &["mark", "shell-1", "mine"]);
    assert!(t.read_ws_file(&ws).contains(r#"mark = "mine""#));
    t.ok(Some(&ws), &["set", "main", r#"row = ["agent"]"#]);
    assert!(t.pane(&ws, "shell-1").is_some());
}

#[test]
fn invalid_file_changes_nothing() {
    let mut t = TestServer::start();
    let ws = setup(&mut t);
    t.ok(Some(&ws), &["place", "test", "--right-of", "agent"]);
    let path = t.ws_file(&ws);
    std::fs::write(&path, "[tab.main]\nrow = [\"agent\"\n").unwrap();
    let out = t.herdfile(Some(&ws), &["apply"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(path.to_str().unwrap()) && err.contains("line 2"),
        "{err}"
    );
    std::thread::sleep(std::time::Duration::from_millis(800));
    assert!(t.pane(&ws, "test").is_some());
    // Commands refuse to write an invalid result.
    let out = t.herdfile(Some(&ws), &["place", "nope"]);
    assert!(!out.status.success());
}

#[test]
fn unknown_name_rejected_by_place() {
    let mut t = TestServer::start();
    let ws = setup(&mut t);
    let out = t.herdfile(Some(&ws), &["place", "nope", "--tab", "x"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("`nope`"));
    assert!(!t.read_ws_file(&ws).contains("nope"));
}

#[test]
fn viewed_tab_defers_moves_but_opens() {
    let mut t = TestServer::start();
    let ws = setup(&mut t);
    t.ok(Some(&ws), &["set", "main", r#"row = ["agent", "test"]"#]);
    let main = t.pane(&ws, "agent").unwrap()["tab_id"]
        .as_str()
        .unwrap()
        .to_string();
    t.herdr(&["tab", "focus", &main]);
    let out = t.ok(
        Some(&ws),
        &["set", "main", r#"row = ["test", "agent", "dev"]"#],
    );
    assert!(out.contains("deferred"), "{out}");
    assert!(t.pane(&ws, "dev").is_some(), "open still happens");
    let labels = t.tab_labels(&ws, "main");
    let pos = |l: &str| labels.iter().position(|x| x == l).unwrap();
    assert!(pos("agent") < pos("test"), "moved while viewed: {labels:?}");
    // Leaving the tab lets the move happen.
    let decoy = t.snapshot()["tabs"][0]["tab_id"]
        .as_str()
        .unwrap()
        .to_string();
    t.herdr(&["tab", "focus", &decoy]);
    t.eventually("reorder after leaving", || {
        t.tab_labels(&ws, "main") == ["test", "agent", "dev"]
    });
}

#[test]
fn concurrent_edits_create_once() {
    let mut t = TestServer::start();
    let ws = setup(&mut t);
    let a = t
        .herdfile_cmd(Some(&ws), &["place", "dev", "--tab", "services"])
        .spawn()
        .unwrap();
    let b = t
        .herdfile_cmd(Some(&ws), &["place", "dev", "--tab", "services"])
        .spawn()
        .unwrap();
    for mut c in [a, b] {
        c.wait().unwrap();
    }
    std::thread::sleep(std::time::Duration::from_millis(800));
    let n = t.snapshot()["panes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["label"] == "dev")
        .count();
    assert_eq!(n, 1);
}
