//! The hidden watcher: one per herdr server, started detached by the plugin's
//! startup hook. It applies workspace files when they or herdr change.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};

use crate::apply::{apply, converged, load_checked, Target};
use crate::backend::herdr::Herdr;
use crate::backend::{Backend, Event};
use crate::control::{Reply, Request};
use crate::lock::FileLock;
use crate::paths;
use crate::writeback::WsState;

/// Quiet time after the last event or edit before a pass runs.
const DEBOUNCE: Duration = Duration::from_millis(150);
const FILE_POLL: Duration = Duration::from_millis(400);
const STATUS_POLL: Duration = Duration::from_secs(1);
/// How long to wait for herdr to come back before giving up.
const HERDR_GONE: Duration = Duration::from_secs(60);
/// Passes in a row that change herdr with no outside trigger before the
/// watcher stops chasing its own tail.
const MAX_SELF_PASSES: u32 = 6;

enum Msg {
    Event(Event),
    Control(Request, UnixStream),
    HerdrGone,
}

pub fn log(line: &str) {
    eprintln!("{} {line}", crate::time::rfc3339(crate::time::now_secs()));
}

/// `herdfile watch --detach`: start a background watcher and return.
pub fn detach() -> Result<()> {
    paths::ensure_state_dir()?;
    if crate::lock::is_held(&paths::watch_lock()) {
        println!("herdfile: a watcher is already running");
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths::watch_log())?;
    let mut cmd = Command::new(exe);
    cmd.arg("watch")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // Herdr's plugin env names the socket; pin it so the watcher keeps
    // talking to this server.
    cmd.env("HERDR_SOCKET_PATH", crate::backend::herdr::default_socket());
    // Never inherit a pane's identity: the watcher belongs to no pane.
    for var in ["HERDR_PANE_ID", "HERDR_TAB_ID", "HERDR_WORKSPACE_ID"] {
        cmd.env_remove(var);
    }
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            // SAFETY: setsid is async-signal-safe.
            libc::setsid();
            Ok(())
        });
    }
    let child = cmd.spawn().context("cannot start the watcher")?;
    println!("herdfile: watcher started (pid {})", child.id());
    Ok(())
}

struct Watcher {
    backend: Herdr,
    states: HashMap<String, WsState>,
    /// Workspaces due a pass, and when.
    due: HashMap<String, Instant>,
    /// Whether the pending pass was asked for from outside (edit, command,
    /// hand change) rather than caused only by our own operations.
    external: HashMap<String, bool>,
    self_passes: HashMap<String, u32>,
    waiting: HashMap<String, Vec<UnixStream>>,
    mtimes: HashMap<PathBuf, SystemTime>,
    /// What each workspace looked like after our last pass.
    signatures: HashMap<String, String>,
    fleet: crate::workspaces::FleetState,
}

/// A cheap fingerprint of a workspace's layout: pane ids and labels per tab,
/// and split ratios.
fn signature(snap: &crate::backend::Snapshot, ws: &str) -> String {
    let mut parts: Vec<String> = snap
        .panes_of(ws)
        .map(|p| {
            format!(
                "{}={}@{}",
                p.pane_id,
                p.label.as_deref().unwrap_or(""),
                p.tab_id
            )
        })
        .collect();
    for layout in snap.layouts.iter().filter(|l| l.workspace_id == ws) {
        for s in &layout.splits {
            parts.push(format!("{}:{}:{:.3}", layout.tab_id, s.id, s.ratio));
        }
    }
    for t in snap.tabs_of(ws) {
        parts.push(format!(
            "{}={}#{}",
            t.tab_id,
            t.label.as_deref().unwrap_or(""),
            t.number
        ));
    }
    parts.sort();
    parts.join(";")
}

pub fn run() -> Result<()> {
    paths::ensure_state_dir()?;
    let Some(lock) = FileLock::try_acquire(&paths::watch_lock())? else {
        println!("herdfile: a watcher is already running");
        return Ok(());
    };
    {
        let mut f = lock.file();
        f.set_len(0)?;
        writeln!(f, "{}", std::process::id())?;
    }
    let backend = Herdr::from_env();
    log(&format!(
        "watcher {} started for {}",
        std::process::id(),
        backend.socket.display()
    ));

    let (tx, rx) = channel::<Msg>();
    spawn_events(backend.socket.clone(), tx.clone());
    // Answer commands only once events flow, so no change slips by unseen.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Msg::Event(e)) if e.kind == "subscribed" => break,
            Ok(Msg::HerdrGone) => {
                log("herdr is not running; watcher exiting");
                return Ok(());
            }
            _ if Instant::now() > deadline => {
                log("no event stream from herdr yet; continuing with polling");
                break;
            }
            _ => {}
        }
    }

    let socket = paths::watch_socket();
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("cannot listen on {}", socket.display()))?;
    spawn_control(listener, tx.clone());

    let mut w = Watcher {
        backend,
        states: HashMap::new(),
        due: HashMap::new(),
        external: HashMap::new(),
        self_passes: HashMap::new(),
        waiting: HashMap::new(),
        mtimes: HashMap::new(),
        signatures: HashMap::new(),
        fleet: Default::default(),
    };
    // Apply every existing file once at start.
    w.scan_files(true);
    let mut last_file_poll = Instant::now();
    let mut last_status_poll = Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Msg::Event(e)) => w.on_event(e),
            Ok(Msg::Control(req, stream)) => w.on_control(req, stream),
            Ok(Msg::HerdrGone) => {
                log("herdr is gone; watcher exiting");
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if last_file_poll.elapsed() >= FILE_POLL {
            w.scan_files(false);
            last_file_poll = Instant::now();
        }
        if last_status_poll.elapsed() >= STATUS_POLL {
            w.poll_status();
            last_status_poll = Instant::now();
        }
        w.run_due();
    }
    let _ = std::fs::remove_file(&socket);
    drop(lock);
    Ok(())
}

fn spawn_control(listener: UnixListener, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut line = String::new();
                let Ok(clone) = stream.try_clone() else {
                    return;
                };
                if BufReader::new(clone).read_line(&mut line).is_err() {
                    return;
                }
                match serde_json::from_str::<Request>(&line) {
                    Ok(req) => {
                        let _ = tx.send(Msg::Control(req, stream));
                    }
                    Err(e) => reply(stream, &Reply::error(format!("bad request: {e}"))),
                }
            });
        }
    });
}

fn spawn_events(socket: PathBuf, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let backend = Herdr::with_socket(&socket);
        let mut gone_since: Option<Instant> = None;
        loop {
            let result = backend.subscribe(&mut |e| tx.send(Msg::Event(e)).is_ok());
            match result {
                Ok(()) => {
                    gone_since = None;
                    // A dropped stream may mean a restart or live handoff: resync.
                    let _ = tx.send(Msg::Event(Event {
                        kind: "resync".into(),
                        data: serde_json::Value::Null,
                    }));
                }
                Err(e) => {
                    let since = *gone_since.get_or_insert_with(Instant::now);
                    if since.elapsed() > HERDR_GONE {
                        let _ = tx.send(Msg::HerdrGone);
                        return;
                    }
                    if since.elapsed() < Duration::from_secs(1) {
                        log(&format!("event stream: {e}; reconnecting"));
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

fn reply(mut stream: UnixStream, reply: &Reply) {
    if let Ok(text) = serde_json::to_string(reply) {
        let _ = writeln!(stream, "{text}");
    }
}

impl Watcher {
    fn managed(&self, ws: &str) -> bool {
        paths::workspace_file(ws).exists()
    }

    fn mark(&mut self, ws: &str, delay: Duration, external: bool) {
        let at = Instant::now() + delay;
        self.due.insert(ws.to_string(), at);
        let e = self.external.entry(ws.to_string()).or_insert(false);
        *e |= external;
    }

    fn mark_all(&mut self, external: bool) {
        let ids: Vec<String> = self.managed_ids();
        for ws in ids {
            self.mark(&ws, DEBOUNCE, external);
        }
    }

    fn managed_ids(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(paths::state_dir()) else {
            return Vec::new();
        };
        entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let id = name.strip_suffix(".toml")?.to_string();
                (id != "workspaces").then_some(id)
            })
            .collect()
    }

    fn on_event(&mut self, e: Event) {
        self.fleet.dirty = true;
        if e.kind == "subscribed" {
            return;
        }
        match e.workspace_id() {
            // Our own operations land here too; they are told apart from
            // hand changes by state, not by the event (events carry no
            // originator).
            Some(ws) if self.managed(&ws) => self.mark(&ws, DEBOUNCE, true),
            Some(_) => {}
            None => self.mark_all(true),
        }
    }

    fn on_control(&mut self, req: Request, stream: UnixStream) {
        match req {
            Request::Ping => reply(
                stream,
                &Reply {
                    ok: true,
                    pid: Some(std::process::id()),
                    ..Reply::default()
                },
            ),
            Request::Apply { workspace } => {
                self.mark(&workspace, Duration::ZERO, true);
                self.self_passes.remove(&workspace);
                self.waiting.entry(workspace).or_default().push(stream);
            }
            Request::Workspaces => {
                self.fleet.dirty = true;
                self.fleet_tick();
                reply(
                    stream,
                    &Reply {
                        ok: true,
                        ..Reply::default()
                    },
                );
            }
        }
    }

    /// Notice direct edits and new or removed files.
    fn scan_files(&mut self, initial: bool) {
        for ws in self.managed_ids() {
            let path = paths::workspace_file(&ws);
            let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
                continue;
            };
            let changed = self.mtimes.get(&path) != Some(&mtime);
            self.mtimes.insert(path, mtime);
            if changed {
                let delay = if initial {
                    Duration::ZERO
                } else {
                    DEBOUNCE * 2
                };
                self.self_passes.remove(&ws);
                self.mark(&ws, delay, true);
            }
        }
        self.mtimes.retain(|p, _| p.exists());
        if let Ok(m) = std::fs::metadata(paths::workspaces_file()).and_then(|m| m.modified()) {
            let path = paths::workspaces_file();
            if self.mtimes.get(&path) != Some(&m) {
                self.mtimes.insert(path, m);
                self.fleet.dirty = true;
            }
        }
    }

    /// Agent status is not an event herdr broadcasts, so poll it: close
    /// panes waiting for idle, finish removals.
    fn poll_status(&mut self) {
        let Ok(snap) = self.backend.snapshot() else {
            return;
        };
        // Backstop for a missed event: any change in a managed workspace's
        // panes, tabs, or splits since our last pass gets a pass.
        for ws in self.managed_ids() {
            let sig = signature(&snap, &ws);
            if self.signatures.get(&ws) != Some(&sig) && !self.due.contains_key(&ws) {
                if self.signatures.contains_key(&ws) {
                    self.mark(&ws, Duration::ZERO, true);
                }
                self.signatures.insert(ws, sig);
            }
        }
        let ids: Vec<String> = self
            .states
            .iter()
            .filter(|(_, s)| !s.pending.is_empty())
            .map(|(ws, _)| ws.clone())
            .collect();
        for ws in ids {
            let ready = self.states[&ws].pending.iter().any(|label| {
                snap.pane_by_label(&ws, label)
                    .map(|p| !p.busy())
                    .unwrap_or(true)
            });
            if ready {
                self.mark(&ws, Duration::ZERO, true);
            }
        }
        self.fleet_tick();
    }

    fn fleet_tick(&mut self) {
        if let Err(e) = crate::workspaces::tick(&self.backend, &mut self.fleet) {
            log(&format!("workspaces: {e:#}"));
        }
    }

    fn run_due(&mut self) {
        let now = Instant::now();
        let ready: Vec<String> = self
            .due
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(ws, _)| ws.clone())
            .collect();
        for ws in ready {
            self.due.remove(&ws);
            let external = self.external.remove(&ws).unwrap_or(false);
            self.pass(&ws, external);
        }
    }

    fn pass(&mut self, ws: &str, external: bool) {
        let waiters = self.waiting.remove(ws).unwrap_or_default();
        let path = paths::workspace_file(ws);
        if !path.exists() {
            for s in waiters {
                reply(s, &Reply::error(format!("no workspace file for {ws}")));
            }
            return;
        }
        if !external {
            let n = self.self_passes.entry(ws.to_string()).or_insert(0);
            if *n >= MAX_SELF_PASSES {
                return;
            }
        }
        let lock = match FileLock::acquire(&paths::lock_for(&path)) {
            Ok(l) => l,
            Err(e) => {
                log(&format!("{ws}: cannot lock: {e}"));
                return;
            }
        };
        let state = self
            .states
            .entry(ws.to_string())
            .or_insert_with(|| WsState::load(ws));
        let target = Target {
            backend: &self.backend,
            ws,
            file_path: path.clone(),
        };
        let result = apply(&target, state);
        let mut problems = Vec::new();
        let r = match result {
            Ok(report) => {
                if let Ok(snap) = self.backend.snapshot() {
                    if let Ok((file, services)) = load_checked(&path, &snap, ws) {
                        problems = converged(&self.backend, ws, &file, &services)
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|p| {
                                !report
                                    .deferred
                                    .iter()
                                    .any(|d| p.contains(&format!("`{d}`")))
                            })
                            .collect();
                    }
                }
                if report.ops() > 0 || !report.writeback.is_empty() {
                    log(&format!("{ws}: {}", report.summary().join("; ")));
                }
                let n = self.self_passes.entry(ws.to_string()).or_insert(0);
                if report.ops() > 0 && !external {
                    *n += 1;
                    if *n >= MAX_SELF_PASSES {
                        log(&format!(
                            "{ws}: still changing after {n} passes; waiting for an edit"
                        ));
                    }
                } else if report.ops() == 0 {
                    *n = 0;
                }
                Reply {
                    ok: true,
                    report: Some(report),
                    problems,
                    ..Reply::default()
                }
            }
            Err(e) => {
                let message = format!("{e:#}");
                log(&format!("{ws}: {message}"));
                Reply::error(message)
            }
        };
        drop(lock);
        if let Ok(snap) = self.backend.snapshot() {
            self.signatures.insert(ws.to_string(), signature(&snap, ws));
        }
        if let Ok(m) = std::fs::metadata(&path).and_then(|m| m.modified()) {
            // Our own write-back edits are not a new edit to react to.
            self.mtimes.insert(path, m);
        }
        for s in waiters {
            let copy = Reply {
                ok: r.ok,
                error: r.error.clone(),
                pid: None,
                report: r.report.clone(),
                problems: r.problems.clone(),
            };
            reply(s, &copy);
        }
    }
}

/// Run apply once in this process (no watcher). Used by `herdfile apply`
/// when no watcher is running.
pub fn apply_once(backend: &dyn Backend, ws: &str) -> Result<Reply> {
    let path = paths::workspace_file(ws);
    let _lock = FileLock::acquire(&paths::lock_for(&path))?;
    let mut state = WsState::load(ws);
    let report = apply(
        &Target {
            backend,
            ws,
            file_path: path.clone(),
        },
        &mut state,
    )?;
    let snap = backend.snapshot()?;
    let (file, services) = load_checked(&path, &snap, ws)?;
    let problems = converged(backend, ws, &file, &services)?;
    Ok(Reply {
        ok: true,
        report: Some(report),
        problems,
        ..Reply::default()
    })
}
