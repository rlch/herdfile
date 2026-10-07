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

#[test]
fn two_sessions_keep_separate_state_and_watchers() {
    let t = TestServer::start();
    // A second herdr server beside the test's own, in the same folders.
    let mut other = std::process::Command::new("herdr");
    other.args(["--session", "other", "server"]);
    for var in [
        "HERDR_SOCKET_PATH",
        "HERDR_SESSION",
        "HERDR_PANE_ID",
        "HERDR_WORKSPACE_ID",
    ] {
        other.env_remove(var);
    }
    other
        .env("XDG_CONFIG_HOME", &t.dir)
        .env("XDG_STATE_HOME", t.dir.join("state"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut other = other.spawn().unwrap();
    let other_sock = t.dir.join("herdr/sessions/other/herdr.sock");
    t.eventually("other server up", || other_sock.exists());
    // One watcher per server: the second is not refused by the first's lock.
    t.ok(None, &["watch", "--detach"]);
    let out = t
        .herdfile_cmd(None, &["watch", "--detach"])
        .env("HERDR_SOCKET_PATH", &other_sock)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("watcher started"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let base = t.dir.join("state/herdfile/sessions");
    t.eventually("both watchers hold their own lock", || {
        base.join("hftest/watch.lock").exists() && base.join("other/watch.lock").exists()
    });
    let path = t
        .herdfile_cmd(Some("w1"), &["path"])
        .env("HERDR_SOCKET_PATH", &other_sock)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&path.stdout).trim(),
        base.join("other/w1.toml").to_str().unwrap()
    );
    for lock in ["hftest", "other"] {
        let pid = std::fs::read_to_string(base.join(lock).join("watch.lock")).unwrap();
        std::process::Command::new("kill")
            .arg(pid.trim())
            .status()
            .unwrap();
    }
    let _ = other.kill();
    let _ = other.wait();
}
