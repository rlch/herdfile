//! The command surface: anchors, waiting, show/status/path, apply without a
//! watcher, marks.

use crate::harness::TestServer;

#[test]
fn place_anchors() {
    let mut t = TestServer::start();
    let ws = t.demo("\n[tab.main]\nrow = [\"agent\"]\n");
    t.ok(Some(&ws), &["place", "test", "--left-of", "agent"]);
    assert_eq!(t.tab_labels(&ws, "main"), ["test", "agent"]);
    t.ok(Some(&ws), &["place", "dev", "--above", "agent"]);
    assert!(t
        .read_ws_file(&ws)
        .contains(r#"row = ["test", { column = ["dev", "agent"] }]"#));
    t.ok(Some(&ws), &["place", "logs", "--before", "test"]);
    assert_eq!(t.tab_labels(&ws, "main"), ["logs", "test", "dev", "agent"]);
    t.ok(
        Some(&ws),
        &["place", "logs", "--after", "test", "--size", "20"],
    );
    assert!(t
        .read_ws_file(&ws)
        .contains(r#"{ pane = "logs", size = 20 }"#));
    let out = t.herdfile(Some(&ws), &["place", "agent", "--right-of", "agent"]);
    assert!(!out.status.success());
}

#[test]
fn no_wait_returns_after_the_edit() {
    let mut t = TestServer::start();
    let ws = t.demo("\n[tab.main]\nrow = [\"agent\"]\n");
    t.pause_watcher();
    let out = t.ok(
        Some(&ws),
        &["place", "test", "--right-of", "agent", "--no-wait"],
    );
    assert!(out.contains("edited"), "{out}");
    assert!(t.pane(&ws, "test").is_none());
    t.resume_watcher();
    t.eventually("applied later", || t.pane(&ws, "test").is_some());
}

#[test]
fn show_status_and_path() {
    let mut t = TestServer::start();
    let ws = t.demo("\n[tab.main]\nrow = [\"agent\"]\n");
    assert!(t.ok(Some(&ws), &["show"]).contains("[tab.main]"));
    let status = t.ok(None, &["status"]);
    assert!(status.contains("watcher running"), "{status}");
    assert!(status.contains(t.socket.to_str().unwrap()), "{status}");
    let path = t.ok(Some(&ws), &["path"]);
    assert_eq!(path.trim(), t.ws_file(&ws).to_str().unwrap());
    t.stop_watcher();
    assert!(t.ok(None, &["status"]).contains("not running"));
    let out = t.herdfile(Some(&ws), &["place", "test", "--tab", "main"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not running"));
}

#[test]
fn apply_without_a_watcher_runs_in_process() {
    let t = TestServer::start();
    t.write_services(crate::harness::SERVICES);
    let ws = t.workspace("demo");
    t.write_ws_file(
        &ws,
        &format!(
            "dir = \"{}\"\n\n[tab.services]\nrow = [\"dev\"]\n",
            t.repo().display()
        ),
    );
    let out = t.ok(Some(&ws), &["apply"]);
    assert!(out.contains("opened: dev"), "{out}");
    assert_eq!(t.tab_labels(&ws, "services"), ["dev"]);
}

#[test]
fn mark_managed_drops_cwd() {
    let mut t = TestServer::start();
    let ws = t.demo("\n[tab.main]\nrow = [\"agent\"]\n");
    let agent = t.pane_id(&ws, "agent");
    t.herdr(&[
        "pane",
        "split",
        &agent,
        "--direction",
        "right",
        "--no-focus",
    ]);
    t.eventually("recorded", || t.read_ws_file(&ws).contains("shell-1"));
    assert!(t.read_ws_file(&ws).contains("cwd ="));
    let out = t.herdfile(Some(&ws), &["mark", "shell-1", "managed"]);
    // A managed pane must be a service or the agent.
    assert!(!out.status.success());
    t.ok(Some(&ws), &["mark", "shell-1", "mine"]);
    assert!(t.read_ws_file(&ws).contains(r#"mark = "mine""#));
    let out = t.herdfile(Some(&ws), &["mark", "nope", "mine"]);
    assert!(!out.status.success());
}

#[test]
fn needs_done_rejects_unknown_ids() {
    let t = TestServer::start();
    let out = t.herdfile(None, &["needs", "done", "n-missing"]);
    assert!(!out.status.success());
    t.ok(None, &["ask", "first?"]);
    t.ok(None, &["ask", "second?"]);
    let list = t.ok(None, &["needs"]);
    assert!(list.find("first?").unwrap() < list.find("second?").unwrap());
    // The same question twice is one entry.
    t.ok(None, &["ask", "first?"]);
    assert_eq!(t.ok(None, &["needs"]).matches("first?").count(), 1);
}
