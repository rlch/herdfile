//! The hidden watcher: one per server, detached, reported by status.

use std::time::Duration;

use crate::harness::TestServer;

#[test]
fn detach_twice_leaves_one_watcher() {
    let t = TestServer::start();
    let first = t.ok(None, &["watch", "--detach"]);
    assert!(first.contains("watcher started"), "{first}");
    t.eventually("watcher up", || {
        String::from_utf8_lossy(&t.herdfile(None, &["status"]).stdout).contains("running (pid")
    });
    let second = t.ok(None, &["watch", "--detach"]);
    assert!(second.contains("already running"), "{second}");
    // No pane or tab was created for it.
    assert_eq!(t.snapshot()["panes"].as_array().unwrap().len(), 0);
    let pid: i32 = std::fs::read_to_string(t.state().join("watch.lock"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // A second foreground watcher also refuses.
    let out = t.ok(None, &["watch"]);
    assert!(out.contains("already running"));
    unsafe {
        libc_kill(pid);
    }
    t.eventually("watcher gone", || {
        String::from_utf8_lossy(&t.herdfile(None, &["status"]).stdout).contains("not running")
    });
}

unsafe fn libc_kill(pid: i32) {
    std::process::Command::new("kill")
        .arg(pid.to_string())
        .status()
        .unwrap();
}

#[test]
fn path_warns_when_watcher_is_down() {
    let t = TestServer::start();
    let out = t.herdfile(Some("w3"), &["path"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim(), t.state().join("w3.toml").to_str().unwrap());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not running"));
    std::thread::sleep(Duration::from_millis(10));
}

#[test]
fn watchdog_runs_alongside_its_server() {
    let mut t = TestServer::start();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(t.watchdog_alive(), "watchdog exited while its test runs");
}
