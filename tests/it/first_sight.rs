//! First sight: the watcher records a workspace it has no file for, exactly
//! as it is on screen, and changes nothing.

use crate::harness::{TestServer, SERVICES};

fn panes_of(t: &TestServer, ws: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = t.snapshot()["panes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["workspace_id"] == ws)
        .map(|p| {
            (
                p["pane_id"].as_str().unwrap().to_string(),
                p["terminal_id"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    v.sort();
    v
}

fn root_pane(t: &TestServer, ws: &str) -> String {
    t.snapshot()["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["workspace_id"] == ws)
        .unwrap()["pane_id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn first_sight_records_the_workspace_as_it_is() {
    let mut t = TestServer::start();
    t.write_services(SERVICES);
    let ws = t.workspace("demo");
    let root = root_pane(&t, &ws);
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
    let before = panes_of(&t, &ws);
    let ratio = t.snapshot()["layouts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["workspace_id"] == ws.as_str())
        .unwrap()["splits"][0]["ratio"]
        .as_f64()
        .unwrap();

    t.start_watcher();
    t.eventually("file written", || t.read_ws_file(&ws).contains("[tab.1]"));
    let file = t.read_ws_file(&ws);
    assert!(file.contains(r#"{ pane = "agent", size = 65 }"#), "{file}");
    assert!(
        file.contains(r#"{ pane = "shell-1", mark = "unmanaged""#),
        "{file}"
    );
    // Labels are set in herdr too; nothing closed, moved, or resized.
    assert!(t.pane(&ws, "agent").is_some() && t.pane(&ws, "shell-1").is_some());
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert_eq!(panes_of(&t, &ws), before);
    assert_eq!(
        t.splits_of(&ws, "agent")[0]["ratio"].as_f64().unwrap(),
        ratio
    );
    let out = t.ok(None, &["apply", "-w", &ws]);
    for change in ["opened", "closed", "moved", "resized"] {
        assert!(!out.contains(change), "{out}");
    }
}

#[test]
fn services_are_recorded_as_managed() {
    let mut t = TestServer::start();
    t.write_services(SERVICES);
    let ws = t.workspace("demo");
    let root = root_pane(&t, &ws);
    t.herdr(&["pane", "rename", &root, "dev"]);
    t.start_watcher();
    t.eventually("file written", || {
        t.read_ws_file(&ws).contains("row = [\"dev\"]")
    });
}

#[test]
fn scope_limits_what_the_watcher_touches() {
    let mut t = TestServer::start();
    let ws = t.workspace("demo");
    let decoy = t.ws_id("decoy").unwrap();
    std::fs::create_dir_all(t.dir.join("herdfile")).unwrap();
    std::fs::write(
        t.dir.join("herdfile/config.toml"),
        "[watch]\nworkspaces = [\"demo\"]\n",
    )
    .unwrap();
    t.start_watcher();
    t.eventually("demo recorded", || t.ws_file(&ws).exists());
    std::thread::sleep(std::time::Duration::from_millis(1200));
    assert!(!t.ws_file(&decoy).exists());
    assert!(
        t.pane(&decoy, "shell-1").is_none(),
        "out-of-scope pane was renamed"
    );
    let wsfile = t.workspaces_file();
    assert!(
        wsfile.contains("[demo]") && !wsfile.contains("[decoy]"),
        "{wsfile}"
    );
}
