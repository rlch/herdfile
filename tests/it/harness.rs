//! A throwaway herdr server for integration tests.
//!
//! Every test runs against its own named session under a fresh
//! `XDG_CONFIG_HOME` in /tmp, never the operator's server. The harness
//! refuses to run if the socket it would use is the default one or the one
//! this process inherited. It is also built not to bother the operator:
//!
//! - at most [`max_servers`] servers run at once (`HERDFILE_TEST_SERVERS`),
//!   so a plain `cargo test` needs no `--test-threads`;
//! - a watchdog stops a test's server if the test process dies (Ctrl-C);
//! - servers run with sounds and pop-ups off, panes run a plain shell, and
//!   git ignores the operator's global config (no signing prompts);
//! - agents are a fake `claude`, checked before any test starts one.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

const SESSION: &str = "hftest";

/// Env vars that would point a child at the operator's server or pane.
const SCRUB: &[&str] = &[
    "HERDR_SOCKET_PATH",
    "HERDR_SESSION",
    "HERDR_PANE_ID",
    "HERDR_TAB_ID",
    "HERDR_WORKSPACE_ID",
    "HERDR_ENV",
    "HERDR_CONFIG_PATH",
    "HERDR_PLUGIN_ID",
];

pub struct TestServer {
    pub dir: PathBuf,
    pub socket: PathBuf,
    server: Child,
    watchdog: Child,
    watcher: Option<Child>,
    _permit: Permit,
}

/// herdr settings for test servers: no sounds, pop-ups, or onboarding.
const QUIET_CONFIG: &str =
    "onboarding = false\n\n[ui.toast]\ndelivery = \"off\"\n\n[ui.sound]\nenabled = false\n";

fn max_servers() -> usize {
    std::env::var("HERDFILE_TEST_SERVERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(3)
}

static RUNNING: Mutex<usize> = Mutex::new(0);
static FREED: Condvar = Condvar::new();

/// A slot among the servers allowed to run at once.
struct Permit;

impl Permit {
    fn take() -> Permit {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        while *running >= max_servers() {
            running = FREED.wait(running).unwrap_or_else(|e| e.into_inner());
        }
        *running += 1;
        Permit
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        *RUNNING.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        FREED.notify_one();
    }
}

/// Stop the test session and remove its folder once this test process is
/// gone, however it went. It only ever targets the test session.
fn spawn_watchdog(dir: &Path) -> Child {
    let script = format!(
        r#"while kill -0 {parent} 2>/dev/null; do sleep 1; done
herdr session stop {SESSION} >/dev/null 2>&1
s=$(cat '{dir}/server.pid' 2>/dev/null)
if [ -n "$s" ] && ps -p "$s" -o command= 2>/dev/null | grep -q -- '--session {SESSION} server'; then kill "$s"; fi
w=$(cat '{dir}/state/herdfile/watch.lock' 2>/dev/null)
if [ -n "$w" ] && ps -p "$w" -o command= 2>/dev/null | grep -q 'herdfile watch'; then kill "$w"; fi
sleep 1
rm -rf '{dir}'"#,
        parent = std::process::id(),
        dir = dir.display(),
    );
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", &script]);
    TestServer::scrub(&mut cmd, dir);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start watchdog")
}

fn live_sockets() -> Vec<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut out = vec![home.join(".config/herdr/herdr.sock")];
    if let Some(p) = std::env::var_os("HERDR_SOCKET_PATH") {
        out.push(PathBuf::from(p));
    }
    if let Some(c) = std::env::var_os("XDG_CONFIG_HOME") {
        out.push(PathBuf::from(c).join("herdr/herdr.sock"));
    }
    out
}

impl TestServer {
    pub fn start() -> TestServer {
        let permit = Permit::take();
        // Unix socket paths must be short: keep the dir directly under /tmp.
        let dir = tempfile::Builder::new()
            .prefix("hf")
            .tempdir_in("/tmp")
            .expect("tempdir")
            .keep();
        let dir = dir.canonicalize().unwrap();
        // First, so that even a test killed this instant leaves nothing.
        let watchdog = spawn_watchdog(&dir);
        let socket = dir.join(format!("herdr/sessions/{SESSION}/herdr.sock"));
        for live in live_sockets() {
            assert_ne!(
                socket, live,
                "refusing to test against the operator's herdr server"
            );
        }
        assert!(socket.starts_with(&dir));
        std::fs::create_dir_all(dir.join("state")).unwrap();
        std::fs::create_dir_all(dir.join("repo/.herdr")).unwrap();
        std::fs::create_dir_all(dir.join("herdr")).unwrap();
        std::fs::write(dir.join("herdr/config.toml"), QUIET_CONFIG).unwrap();

        let mut cmd = Command::new("herdr");
        cmd.args(["--session", SESSION, "server"]);
        Self::scrub(&mut cmd, &dir);
        let server = cmd
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start herdr test server");
        std::fs::write(dir.join("server.pid"), server.id().to_string()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !socket.exists() {
            assert!(Instant::now() < deadline, "test server did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        let t = TestServer {
            dir,
            socket,
            server,
            watchdog,
            watcher: None,
            _permit: permit,
        };
        // Wait for the API to answer.
        let deadline = Instant::now() + Duration::from_secs(10);
        while t.try_herdr(&["api", "snapshot"]).status.code() != Some(0) {
            assert!(Instant::now() < deadline, "test server not answering");
            std::thread::sleep(Duration::from_millis(50));
        }
        t
    }

    fn scrub(cmd: &mut Command, dir: &Path) {
        for var in SCRUB {
            cmd.env_remove(var);
        }
        let path = format!(
            "{}:{}",
            fake_bin_dir().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        cmd.env("XDG_CONFIG_HOME", dir)
            .env("PATH", path)
            .env("XDG_STATE_HOME", dir.join("state"))
            // Plain shells start fast and predictably, with the fake agent
            // CLIs first on PATH.
            .env("SHELL", fake_bin_dir().join("test-shell"))
            .env("ENV", "/dev/null")
            // Git, here and in herdr's worktree commands, ignores the
            // operator's config: no hooks, no signing prompts.
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "herdfile test")
            .env("GIT_AUTHOR_EMAIL", "test@herdfile.invalid")
            .env("GIT_COMMITTER_NAME", "herdfile test")
            .env("GIT_COMMITTER_EMAIL", "test@herdfile.invalid");
    }

    fn env(&self, cmd: &mut Command) {
        Self::scrub(cmd, &self.dir);
        cmd.env("HERDR_SOCKET_PATH", &self.socket);
    }

    pub fn try_herdr(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new("herdr");
        cmd.args(args);
        self.env(&mut cmd);
        cmd.output().expect("run herdr")
    }

    /// Run a herdr CLI command on the test server and parse its JSON.
    pub fn herdr(&self, args: &[&str]) -> Value {
        let out = self.try_herdr(args);
        assert!(
            out.status.success(),
            "herdr {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }

    pub fn snapshot(&self) -> Value {
        self.herdr(&["api", "snapshot"])["result"]["snapshot"].clone()
    }

    pub fn herdfile_cmd(&self, ws: Option<&str>, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_herdfile"));
        cmd.args(args);
        self.env(&mut cmd);
        if let Some(ws) = ws {
            cmd.env("HERDR_WORKSPACE_ID", ws);
        }
        cmd
    }

    pub fn herdfile(&self, ws: Option<&str>, args: &[&str]) -> Output {
        self.herdfile_cmd(ws, args).output().expect("run herdfile")
    }

    /// Run herdfile and require success; returns stdout.
    pub fn ok(&self, ws: Option<&str>, args: &[&str]) -> String {
        let out = self.herdfile(ws, args);
        assert!(
            out.status.success(),
            "herdfile {args:?} failed:\nstdout: {}\nstderr: {}\nlog: {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
            self.log()
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    pub fn start_watcher(&mut self) {
        let child = self
            .herdfile_cmd(None, &["watch"])
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(self.dir.join("watch.err")).unwrap())
            .spawn()
            .expect("start watcher");
        self.watcher = Some(child);
        let deadline = Instant::now() + Duration::from_secs(10);
        while !String::from_utf8_lossy(&self.herdfile(None, &["status"]).stdout)
            .contains("running (pid")
        {
            assert!(
                Instant::now() < deadline,
                "watcher did not start: {}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn watchdog_alive(&mut self) -> bool {
        match self.watchdog.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                eprintln!("watchdog exited: {status}");
                false
            }
            Err(_) => false,
        }
    }

    /// Freeze the watcher, as if it were busy: edits land, nothing applies.
    pub fn pause_watcher(&self) {
        self.signal_watcher("-STOP");
    }

    pub fn resume_watcher(&self) {
        self.signal_watcher("-CONT");
    }

    fn signal_watcher(&self, sig: &str) {
        let pid = self.watcher.as_ref().expect("watcher running").id();
        Command::new("kill")
            .args([sig, &pid.to_string()])
            .status()
            .unwrap();
    }

    /// Workspace `demo` in the repo folder: tab `main` holding the pane
    /// labelled `agent`, the services file, a workspace file with
    /// `file_body` after its `dir`, and a watcher that has applied it.
    pub fn demo(&mut self, file_body: &str) -> String {
        self.write_services(SERVICES);
        let ws = self.workspace("demo");
        let snap = self.snapshot();
        let first = |kind: &str, key: &str| {
            snap[kind]
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["workspace_id"] == ws.as_str())
                .unwrap()[key]
                .as_str()
                .unwrap()
                .to_string()
        };
        self.herdr(&["tab", "rename", &first("tabs", "tab_id"), "main"]);
        self.herdr(&["pane", "rename", &first("panes", "pane_id"), "agent"]);
        self.write_ws_file(
            &ws,
            &format!("dir = \"{}\"\n{file_body}", self.repo().display()),
        );
        self.start_watcher();
        self.ok(None, &["apply", "-w", &ws]);
        ws
    }

    /// Give the watcher time to see events and finish its passes.
    pub fn settle(&self) {
        std::thread::sleep(Duration::from_millis(1500));
    }

    pub fn tab_id(&self, ws: &str, label: &str) -> String {
        self.snapshot()["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["workspace_id"] == ws && t["label"] == label)
            .unwrap_or_else(|| panic!("no tab {label}"))["tab_id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    pub fn stop_watcher(&mut self) {
        if let Some(mut w) = self.watcher.take() {
            let _ = w.kill();
            let _ = w.wait();
        }
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("watch.err")).unwrap_or_default()
    }

    pub fn state(&self) -> PathBuf {
        self.dir.join("state/herdfile")
    }

    pub fn repo(&self) -> PathBuf {
        self.dir.join("repo")
    }

    pub fn write_services(&self, text: &str) {
        std::fs::write(self.repo().join(".herdr/services.toml"), text).unwrap();
    }

    pub fn ws_file(&self, ws: &str) -> PathBuf {
        self.state().join(format!("{ws}.toml"))
    }

    pub fn read_ws_file(&self, ws: &str) -> String {
        std::fs::read_to_string(self.ws_file(ws)).unwrap_or_default()
    }

    pub fn write_ws_file(&self, ws: &str, text: &str) {
        std::fs::create_dir_all(self.state()).unwrap();
        std::fs::write(self.ws_file(ws), text).unwrap();
    }

    /// A workspace in the repo folder. A decoy workspace is created first so
    /// that this one is not the one the "operator" is viewing.
    pub fn workspace(&self, label: &str) -> String {
        if self.snapshot()["workspaces"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true)
        {
            self.herdr(&[
                "workspace",
                "create",
                "--label",
                "decoy",
                "--cwd",
                "/tmp",
                "--no-focus",
            ]);
        }
        let repo = self.repo();
        let r = self.herdr(&[
            "workspace",
            "create",
            "--label",
            label,
            "--cwd",
            repo.to_str().unwrap(),
            "--no-focus",
        ]);
        r["result"]["workspace"]["workspace_id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Labels of panes in a workspace's tab, depth-first.
    pub fn tab_labels(&self, ws: &str, tab_label: &str) -> Vec<String> {
        let snap = self.snapshot();
        let Some(tab) = snap["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["workspace_id"] == ws && t["label"] == tab_label)
        else {
            return Vec::new();
        };
        let layout = snap["layouts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["tab_id"] == tab["tab_id"])
            .unwrap();
        layout["panes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let id = &p["pane_id"];
                snap["panes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|q| &q["pane_id"] == id)
                    .and_then(|q| q["label"].as_str())
                    .unwrap_or("?")
                    .to_string()
            })
            .collect()
    }

    pub fn tab_names(&self, ws: &str) -> Vec<String> {
        let snap = self.snapshot();
        let mut tabs: Vec<&Value> = snap["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["workspace_id"] == ws)
            .collect();
        tabs.sort_by_key(|t| t["number"].as_u64());
        tabs.iter()
            .map(|t| t["label"].as_str().unwrap_or("").to_string())
            .collect()
    }

    pub fn pane(&self, ws: &str, label: &str) -> Option<Value> {
        self.snapshot()["panes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["workspace_id"] == ws && p["label"] == label)
            .cloned()
    }

    pub fn pane_id(&self, ws: &str, label: &str) -> String {
        self.pane(ws, label)
            .unwrap_or_else(|| panic!("no pane {label}"))["pane_id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// The splits of the tab holding `label`.
    pub fn splits_of(&self, ws: &str, label: &str) -> Vec<Value> {
        let snap = self.snapshot();
        let pane = self.pane(ws, label).unwrap();
        snap["layouts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["tab_id"] == pane["tab_id"])
            .unwrap()["splits"]
            .as_array()
            .unwrap()
            .clone()
    }

    /// Mark a pane as hosting an agent in some state.
    pub fn report_agent(&self, pane: &str, state: &str) {
        self.herdr(&[
            "pane",
            "report-agent",
            pane,
            "--source",
            "hftest",
            "--agent",
            "claude",
            "--state",
            state,
        ]);
    }

    /// Wait until `f` holds, polling.
    pub fn eventually(&self, what: &str, f: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !f() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for: {what}\nlog: {}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Drag a divider: set a split's ratio over the raw socket, as a hand
    /// resize would.
    pub fn set_ratio(&self, tab: &str, path: &[bool], ratio: f64) {
        use std::io::{BufRead, BufReader, Write};
        let mut s = std::os::unix::net::UnixStream::connect(&self.socket).unwrap();
        let req = serde_json::json!({"id": "t", "method": "layout.set_split_ratio",
            "params": {"tab_id": tab, "path": path, "ratio": ratio}});
        writeln!(s, "{req}").unwrap();
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).unwrap();
        assert!(line.contains("result"), "{line}");
    }

    pub fn wait_output(&self, pane: &str, text: &str) {
        self.herdr(&[
            "pane",
            "wait-output",
            pane,
            "--match",
            text,
            "--timeout",
            "15000",
        ]);
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // The watchdog is only for when this process dies without dropping.
        let _ = self.watchdog.kill();
        let _ = self.watchdog.wait();
        self.stop_watcher();
        let mut stop = Command::new("herdr");
        stop.args(["session", "stop", SESSION]);
        Self::scrub(&mut stop, &self.dir);
        let _ = stop.stdout(Stdio::null()).stderr(Stdio::null()).status();
        // Only our own child: never the operator's server.
        let _ = self.server.kill();
        let _ = self.server.wait();
        if std::env::var_os("HERDFILE_KEEP_TEST_DIR").is_none() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// The fake `claude` (tests/fixtures/fake_claude.rs), compiled once.
fn fake_bin_dir() -> &'static PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fakebin");
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("claude");
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_claude.rs");
        let lock = dir.join(format!("build-{}", std::process::id()));
        let fresh = std::fs::metadata(&bin)
            .and_then(|b| Ok(b.modified()? >= std::fs::metadata(&src)?.modified()?))
            .unwrap_or(false);
        if !fresh {
            let tmp = lock.with_extension("bin");
            let out = Command::new("rustc")
                .args(["-O", "--edition", "2021", "-o"])
                .arg(&tmp)
                .arg(&src)
                .output()
                .expect("rustc");
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            std::fs::rename(&tmp, &bin).unwrap();
        }
        // Pane shells: a login shell would re-sort PATH and find the real
        // agent CLIs, so panes run a plain shell with the fakes first.
        let shell = dir.join("test-shell");
        std::fs::write(
            &shell,
            format!(
                "#!/bin/sh\nPATH=\"{}:$PATH\"\nexport PATH\nexec /bin/sh \"$@\"\n",
                dir.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        dir
    })
}

impl TestServer {
    /// Start agents through a wrapper typed at the shell, as the operator's
    /// `cl --{model}` does.
    pub fn use_wrapper_command(&self) {
        let cfg = self.dir.join("herdfile");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(
            cfg.join("config.toml"),
            "[agent]\ncommand = \"WRAPPED=1 claude --{model}\"\n",
        )
        .unwrap();
    }

    /// Refuse to go on unless `claude` in a pane of `ws` is the fake: a test
    /// must never start a real agent.
    pub fn require_fake_agents(&self, pane: &str) {
        let out = self.dir.join("which-claude");
        let _ = std::fs::remove_file(&out);
        self.herdr(&[
            "pane",
            "run",
            pane,
            &format!("command -v claude > {}", out.display()),
        ]);
        self.eventually("claude resolved in a pane", || {
            std::fs::read_to_string(&out)
                .map(|s| !s.is_empty())
                .unwrap_or(false)
        });
        let found = std::fs::read_to_string(&out).unwrap();
        assert_eq!(
            PathBuf::from(found.trim()),
            fake_bin_dir().join("claude"),
            "panes would start a real claude; refusing to run"
        );
    }

    /// A git repo with one commit on main.
    pub fn git_repo(&self) -> PathBuf {
        let repo = self.dir.join("git");
        std::fs::create_dir_all(&repo).unwrap();
        self.git(&repo, &["init", "-q", "-b", "main"]);
        self.git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);
        repo
    }

    /// Run git in `dir`, isolated from the operator's git config.
    pub fn git(&self, dir: &Path, args: &[&str]) -> String {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(dir).args(args);
        Self::scrub(&mut cmd, &self.dir);
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    pub fn workspaces_file(&self) -> String {
        std::fs::read_to_string(self.state().join("workspaces.toml")).unwrap_or_default()
    }

    pub fn ws_id(&self, label: &str) -> Option<String> {
        self.snapshot()["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["label"] == label)
            .map(|w| w["workspace_id"].as_str().unwrap().to_string())
    }

    pub fn read_pane(&self, pane: &str) -> String {
        let out = self.try_herdr(&[
            "pane",
            "read",
            pane,
            "--source",
            "recent-unwrapped",
            "--lines",
            "200",
        ]);
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

pub const SERVICES: &str = r#"
[dev]
cmd = "echo dev-up; sleep 100000"
ready = "dev-up"

[test]
cmd = "echo test-up; sleep 100000"

[logs]
cmd = "echo logs-up; sleep 100000"
"#;
