//! Write-back: hand changes are folded into the file, never fought.

use std::time::Duration;

use crate::harness::{TestServer, SERVICES};

fn setup(t: &mut TestServer, file_body: &str) -> String {
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
        &format!("dir = \"{}\"\n{file_body}", t.repo().display()),
    );
    t.start_watcher();
    t.ok(Some(&ws), &["apply"]);
    ws
}

fn settle() {
    std::thread::sleep(Duration::from_millis(1500));
}

#[test]
fn hand_close_is_written_back_and_stays_closed() {
    let mut t = TestServer::start();
    let ws = setup(
        &mut t,
        "\n# the operator's layout\n[tab.main]\nrow = [\"agent\", \"test\"]  # keep this comment\n\n[tab.services]\nrow = [\"dev\", \"logs\"]\n",
    );
    let before = t.read_ws_file(&ws);
    let test = t.pane_id(&ws, "test");
    t.herdr(&["pane", "close", &test]);
    t.eventually("test dropped from file", || {
        !t.read_ws_file(&ws).contains("\"test\"")
    });
    settle();
    assert!(t.pane(&ws, "test").is_none(), "test was reopened");
    // Only the row line of [tab.main] changed.
    let after = t.read_ws_file(&ws);
    let changed: Vec<_> = before
        .lines()
        .zip(after.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(changed.len(), 1, "{after}");
    assert!(
        after.contains("row = [\"agent\"]  # keep this comment"),
        "{after}"
    );
    assert!(after.contains("# the operator's layout"));
}

#[test]
fn own_close_is_not_written_back() {
    let mut t = TestServer::start();
    let ws = setup(&mut t, "\n[tab.main]\nrow = [\"agent\", \"test\"]\n");
    t.ok(Some(&ws), &["remove", "test"]);
    let after_remove = t.read_ws_file(&ws);
    settle();
    assert_eq!(t.read_ws_file(&ws), after_remove);
    assert!(!t.log().contains("dropped"), "{}", t.log());
}

#[test]
fn hand_opened_shell_is_recorded_in_place() {
    let mut t = TestServer::start();
    let ws = setup(&mut t, "\n[tab.main]\nrow = [\"agent\", \"test\"]\n");
    let agent = t.pane_id(&ws, "agent");
    t.herdr(&[
        "pane",
        "split",
        &agent,
        "--direction",
        "right",
        "--no-focus",
    ]);
    t.eventually("shell recorded", || t.read_ws_file(&ws).contains("shell-1"));
    let file = t.read_ws_file(&ws);
    // In its place, with the sizes on screen, so no divider moves.
    assert!(
        file.contains(r#"row = [{ pane = "agent", size = 25 }, { pane = "shell-1", size = 25, mark = "unmanaged""#),
        "{file}"
    );
    let ratios = t.splits_of(&ws, "agent");
    settle();
    assert!(t.pane(&ws, "shell-1").is_some());
    assert_eq!(t.tab_labels(&ws, "main"), ["agent", "shell-1", "test"]);
    assert_eq!(t.splits_of(&ws, "agent"), ratios, "a divider moved");
}

#[test]
fn dragged_size_is_written_back_rounded() {
    let mut t = TestServer::start();
    let ws = setup(
        &mut t,
        "\n[tab.main]\nrow = [{ pane = \"agent\", size = 60 }, \"test\"]\n",
    );
    let tab = t.pane(&ws, "agent").unwrap()["tab_id"]
        .as_str()
        .unwrap()
        .to_string();
    // A small drag rounds to the same size: no change.
    let before = t.read_ws_file(&ws);
    t.set_ratio(&tab, &[], 0.61);
    settle();
    assert_eq!(t.read_ws_file(&ws), before);
    // A real drag is written back, rounded to 5%, and not undone.
    t.set_ratio(&tab, &[], 0.73);
    t.eventually("size written back", || {
        t.read_ws_file(&ws).contains("size = 75")
    });
    settle();
    let root = t
        .splits_of(&ws, "agent")
        .into_iter()
        .find(|s| s["id"].as_str().unwrap().ends_with("_root"))
        .unwrap();
    assert!(
        (root["ratio"].as_f64().unwrap() - 0.73).abs() < 0.005,
        "{root}"
    );
}

#[test]
fn hand_close_beats_a_queued_command() {
    let mut t = TestServer::start();
    let ws = setup(
        &mut t,
        "\n[tab.main]\nrow = [\"agent\"]\n\n[tab.services]\nrow = [\"dev\"]\n",
    );
    let watcher: i32 = std::fs::read_to_string(t.state().join("watch.lock"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let signal = |sig: &str| {
        std::process::Command::new("kill")
            .args([sig, &watcher.to_string()])
            .status()
            .unwrap();
    };
    // The watcher is busy: the command's edit lands but apply has not run...
    signal("-STOP");
    let place = t
        .herdfile_cmd(Some(&ws), &["place", "dev", "--tab", "main"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    t.eventually("edit written", || {
        t.read_ws_file(&ws).contains("row = [\"agent\", \"dev\"]")
    });
    // ...when the operator closes dev by hand.
    let dev = t.pane_id(&ws, "dev");
    t.herdr(&["pane", "close", &dev]);
    signal("-CONT");
    let out = place.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(err.contains("dropped") && err.contains("`dev`"), "{err}");
    assert!(!t.read_ws_file(&ws).contains("dev"));
    settle();
    assert!(t.pane(&ws, "dev").is_none());
}

#[test]
fn status_change_does_not_rewrite_the_file() {
    let mut t = TestServer::start();
    let ws = setup(&mut t, "\n[tab.main]\nrow = [\"agent\"]\n");
    let path = t.ws_file(&ws);
    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    let agent = t.pane_id(&ws, "agent");
    t.report_agent(&agent, "working");
    settle();
    t.report_agent(&agent, "idle");
    settle();
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), mtime);
}

#[test]
fn tab_renamed_outside_is_written_back_not_rebuilt() {
    let mut t = TestServer::start();
    let ws = setup(&mut t, "\n[tab.main]\nrow = [\"agent\", \"test\"]\n");
    let tab = t.pane(&ws, "agent").unwrap()["tab_id"]
        .as_str()
        .unwrap()
        .to_string();
    let terms: Vec<_> = ["agent", "test"]
        .iter()
        .map(|l| t.pane(&ws, l).unwrap()["pane_id"].clone())
        .collect();
    t.herdr(&["tab", "rename", &tab, "my title"]);
    t.eventually("rename written back", || {
        t.read_ws_file(&ws).contains("[tab.\"my title\"]")
    });
    settle();
    assert_eq!(t.tab_names(&ws), ["my title"]);
    let after: Vec<_> = ["agent", "test"]
        .iter()
        .map(|l| t.pane(&ws, l).unwrap()["pane_id"].clone())
        .collect();
    assert_eq!(after, terms, "panes were moved");
    assert!(!t.log().contains("moved"), "{}", t.log());
}

#[test]
fn pane_moved_to_another_tab_is_written_back() {
    let mut t = TestServer::start();
    let ws =
        t.demo("\n[tab.main]\nrow = [\"agent\", \"test\"]\n\n[tab.services]\nrow = [\"dev\"]\n");
    let services = t.tab_id(&ws, "services");
    let dev = t.pane_id(&ws, "dev");
    let test = t.pane_id(&ws, "test");
    t.herdr(&[
        "pane",
        "move",
        &test,
        "--tab",
        &services,
        "--split",
        "right",
        "--target-pane",
        &dev,
        "--no-focus",
    ]);
    t.eventually("move written back", || {
        let f = t.read_ws_file(&ws);
        f.contains("[tab.main]\nrow = [\"agent\"]") && f.contains("row = [\"dev\", \"test\"]")
    });
    t.settle();
    assert_eq!(t.tab_labels(&ws, "services"), ["dev", "test"], "moved back");
    assert_eq!(t.tab_labels(&ws, "main"), ["agent"]);
}

#[test]
fn swap_by_hand_is_written_back() {
    let mut t = TestServer::start();
    let ws = t.demo("\n[tab.main]\nrow = [\"agent\", \"test\"]\n");
    let agent = t.pane_id(&ws, "agent");
    let test = t.pane_id(&ws, "test");
    t.herdr(&[
        "pane",
        "swap",
        "--source-pane",
        &agent,
        "--target-pane",
        &test,
    ]);
    t.eventually("swap written back", || {
        t.read_ws_file(&ws).contains("row = [\"test\", \"agent\"]")
    });
    t.settle();
    assert_eq!(t.tab_labels(&ws, "main"), ["test", "agent"], "swapped back");
}

#[test]
fn hand_move_beats_a_queued_command() {
    let mut t = TestServer::start();
    let ws =
        t.demo("\n[tab.main]\nrow = [\"agent\", \"test\"]\n\n[tab.services]\nrow = [\"dev\"]\n");
    t.pause_watcher();
    let place = t
        .herdfile_cmd(Some(&ws), &["place", "test", "--tab", "services"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    t.eventually("edit written", || {
        t.read_ws_file(&ws).contains("row = [\"dev\", \"test\"]")
    });
    let agent = t.pane_id(&ws, "agent");
    let test = t.pane_id(&ws, "test");
    t.herdr(&[
        "pane",
        "swap",
        "--source-pane",
        &agent,
        "--target-pane",
        &test,
    ]);
    t.resume_watcher();
    let out = place.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        err.contains("moved by hand") && err.contains("`test`"),
        "{err}"
    );
    t.settle();
    assert_eq!(t.tab_labels(&ws, "main"), ["test", "agent"]);
    assert!(t.read_ws_file(&ws).contains("row = [\"test\", \"agent\"]"));
}

#[test]
fn queued_place_survives_a_hand_split_of_its_tab() {
    let mut t = TestServer::start();
    let ws = t.demo("\n[tab.main]\nrow = [\"agent\", \"test\"]\n");
    t.pause_watcher();
    let place = t
        .herdfile_cmd(Some(&ws), &["place", "logs", "--after", "agent"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    t.eventually("edit written", || t.read_ws_file(&ws).contains("\"logs\""));
    let test = t.pane_id(&ws, "test");
    t.herdr(&["pane", "split", &test, "--direction", "down", "--no-focus"]);
    t.resume_watcher();
    let out = place.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    t.settle();
    let labels = t.tab_labels(&ws, "main");
    assert!(labels.contains(&"logs".to_string()), "{labels:?}");
    assert!(labels.contains(&"shell-1".to_string()), "{labels:?}");
    let f = t.read_ws_file(&ws);
    assert!(f.contains("\"logs\"") && f.contains("shell-1"), "{f}");
}

#[test]
fn hand_close_while_watcher_is_down_is_written_back() {
    let mut t = TestServer::start();
    let ws = t.demo("\n[tab.main]\nrow = [\"agent\", \"test\"]\n");
    t.stop_watcher();
    let test = t.pane_id(&ws, "test");
    t.herdr(&["pane", "close", &test]);
    t.start_watcher();
    t.eventually("dropped on restart", || {
        !t.read_ws_file(&ws).contains("\"test\"")
    });
    t.settle();
    assert!(t.pane(&ws, "test").is_none(), "reopened after restart");
}

#[test]
fn tab_titles_set_by_agents_are_never_renamed() {
    let mut t = TestServer::start();
    let ws = t.workspace("demo");
    let first = t.tab_id(&ws, "1");
    t.herdr(&["tab", "rename", &first, "Fix the login bug"]);
    let r = t.herdr(&[
        "tab",
        "create",
        "--workspace",
        &ws,
        "--label",
        "Fix the login bug",
        "--no-focus",
    ]);
    let second = r["result"]["tab"]["tab_id"].as_str().unwrap().to_string();
    t.start_watcher();
    t.eventually("recorded", || {
        t.read_ws_file(&ws)
            .contains("[tab.\"Fix the login bug (2)\"]")
    });
    t.settle();
    // herdr still shows the agent's titles, unchanged.
    assert_eq!(t.tab_names(&ws), ["Fix the login bug", "Fix the login bug"]);
    let out = t.ok(None, &["apply", "-w", &ws]);
    for change in ["opened", "closed", "moved", "resized"] {
        assert!(
            !out.contains(change),
            "{out}\nlog: {}\ntabs: {:?}\nfile: {}",
            t.log(),
            t.tab_names(&ws),
            t.read_ws_file(&ws)
        );
    }
    // The agent retitles the second tab: the file follows, herdr is untouched.
    t.herdr(&["tab", "rename", &second, "Review PR 312"]);
    t.eventually("retitle written back", || {
        t.read_ws_file(&ws).contains("[tab.\"Review PR 312\"]")
    });
    t.settle();
    assert_eq!(t.tab_names(&ws), ["Fix the login bug", "Review PR 312"]);
}
