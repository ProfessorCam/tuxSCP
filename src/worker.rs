//! Background worker thread that owns the transfer backend.
//!
//! `ssh2::Session` (and the FTP stream) are not `Send`, so every operation runs
//! on one dedicated thread per connection. The UI talks to it exclusively over
//! channels.
//!
//! The worker is protocol-agnostic: it drives a boxed [`Backend`] trait object
//! whose concrete implementation is chosen at connect time (SFTP, SCP, FTP or
//! FTPS). Directory transfers and deletions are orchestrated here so every
//! backend gets recursion "for free".

use crate::models::{AuthMethod, ConnectionParams, FileEntry, Protocol};
use anyhow::{anyhow, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod ftp_backend;
mod scp_backend;
mod sftp_backend;

pub use ftp_backend::FtpBackend;
pub use scp_backend::ScpBackend;
pub use sftp_backend::SftpBackend;

// ── Commands sent from UI → Worker ──────────────────────────────────────────

#[derive(Debug)]
pub enum WorkerCmd {
    Connect(ConnectionParams),
    Disconnect,
    ListDir(String),
    Download {
        transfer_id: String,
        remote_path: String,
        local_path: PathBuf,
        is_dir: bool,
    },
    Upload {
        transfer_id: String,
        local_path: PathBuf,
        remote_path: String,
        is_dir: bool,
    },
    Delete {
        path: String,
        is_dir: bool,
    },
    Rename {
        from: String,
        to: String,
    },
    Mkdir(String),
    Chmod {
        path: String,
        mode: u32,
    },
    CancelTransfer(String),
    Quit,
}

// ── Events sent from Worker → UI ─────────────────────────────────────────────

#[derive(Debug)]
pub enum WorkerEvent {
    Connected {
        host: String,
        username: String,
        home_dir: String,
        listing: Vec<FileEntry>,
    },
    ConnectionFailed(String),
    Disconnected,
    DirListing {
        path: String,
        entries: Vec<FileEntry>,
    },
    DirError {
        path: String,
        error: String,
    },
    TransferProgress {
        id: String,
        transferred: u64,
        total: u64,
        speed_bps: f64,
    },
    TransferComplete {
        id: String,
    },
    TransferFailed {
        id: String,
        error: String,
    },
    OperationComplete {
        op: String,
    },
    OperationFailed {
        op: String,
        error: String,
    },
}

// ── Public handle used by the UI ──────────────────────────────────────────────

pub struct WorkerHandle {
    pub cmd_tx: Sender<WorkerCmd>,
    pub event_rx: Receiver<WorkerEvent>,
    cancel_current: Arc<AtomicBool>,
}

impl WorkerHandle {
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = bounded::<WorkerCmd>(64);
        let (event_tx, event_rx) = bounded::<WorkerEvent>(256);
        let cancel_current = Arc::new(AtomicBool::new(false));
        let cancel_clone = cancel_current.clone();

        std::thread::spawn(move || {
            worker_thread(cmd_rx, event_tx, cancel_clone);
        });

        WorkerHandle { cmd_tx, event_rx, cancel_current }
    }

    pub fn send(&self, cmd: WorkerCmd) {
        let _ = self.cmd_tx.send(cmd);
    }

    pub fn cancel_current_transfer(&self) {
        self.cancel_current.store(true, Ordering::Relaxed);
    }

    /// Drain all pending events, returning them.
    pub fn drain_events(&self) -> Vec<WorkerEvent> {
        let mut events = Vec::new();
        while let Ok(e) = self.event_rx.try_recv() {
            events.push(e);
        }
        events
    }
}

// ── Backend abstraction ───────────────────────────────────────────────────────

/// A remote filesystem backend. One concrete impl per protocol.
///
/// Implementations only need to handle *single* files/dirs; recursion across a
/// directory tree is orchestrated by the worker (see [`download_tree`],
/// [`upload_tree`], [`delete_tree`]).
pub trait Backend {
    /// The directory the session lands in (server home / login dir).
    fn home_dir(&mut self) -> Result<String>;
    /// Raw directory contents (no synthetic ".." entry, unsorted).
    fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>>;
    /// Size in bytes of a single remote file.
    fn remote_size(&mut self, path: &str) -> Result<u64>;

    fn download_file(&mut self, remote: &str, local: &Path, prog: &mut Progress) -> Result<()>;
    fn upload_file(&mut self, local: &Path, remote: &str, prog: &mut Progress) -> Result<()>;

    fn delete_file(&mut self, path: &str) -> Result<()>;
    fn delete_dir(&mut self, path: &str) -> Result<()>;
    fn rename(&mut self, from: &str, to: &str) -> Result<()>;
    fn mkdir(&mut self, path: &str) -> Result<()>;
    fn chmod(&mut self, path: &str, mode: u32) -> Result<()>;
}

// ── Progress reporting (with throttling) ──────────────────────────────────────

/// Accumulates transferred bytes across one (possibly multi-file) transfer and
/// emits throttled [`WorkerEvent::TransferProgress`] events — at most one every
/// [`Progress::EMIT_INTERVAL`], plus a guaranteed final one via [`Progress::finish`].
pub struct Progress<'a> {
    id: String,
    total: u64,
    transferred: u64,
    start: Instant,
    last_emit: Option<Instant>,
    tx: &'a Sender<WorkerEvent>,
    cancel: &'a AtomicBool,
}

impl<'a> Progress<'a> {
    const EMIT_INTERVAL: Duration = Duration::from_millis(80);

    fn new(id: &str, total: u64, tx: &'a Sender<WorkerEvent>, cancel: &'a AtomicBool) -> Self {
        Self {
            id: id.to_string(),
            total,
            transferred: 0,
            start: Instant::now(),
            last_emit: None,
            tx,
            cancel,
        }
    }

    /// Record `n` freshly transferred bytes. Returns `Err` if the transfer has
    /// been cancelled so the copy loop can abort promptly.
    pub fn add(&mut self, n: u64) -> Result<()> {
        self.transferred += n;
        if self.cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("Transfer cancelled"));
        }
        let now = Instant::now();
        let due = self
            .last_emit
            .map(|t| now.duration_since(t) >= Self::EMIT_INTERVAL)
            .unwrap_or(true);
        if due {
            self.emit(now);
        }
        Ok(())
    }

    fn emit(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.start).as_secs_f64();
        let speed = if elapsed > 0.05 { self.transferred as f64 / elapsed } else { 0.0 };
        let _ = self.tx.send(WorkerEvent::TransferProgress {
            id: self.id.clone(),
            transferred: self.transferred,
            total: self.total,
            speed_bps: speed,
        });
        self.last_emit = Some(now);
    }

    fn finish(&mut self) {
        self.emit(Instant::now());
    }
}

/// Copy `reader` → `writer`, reporting progress and honouring cancellation.
pub fn copy_with_progress<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    prog: &mut Progress,
) -> Result<()> {
    let mut buf = vec![0u8; 128 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        prog.add(n as u64)?;
    }
    writer.flush()?;
    Ok(())
}

// ── Worker thread ─────────────────────────────────────────────────────────────

fn worker_thread(
    cmd_rx: Receiver<WorkerCmd>,
    event_tx: Sender<WorkerEvent>,
    cancel: Arc<AtomicBool>,
) {
    let mut state: Option<Box<dyn Backend>> = None;

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            WorkerCmd::Quit => break,

            WorkerCmd::Connect(params) => {
                state = None;
                match do_connect(&params) {
                    Ok(mut backend) => {
                        // Land in the requested directory if one was supplied,
                        // otherwise the server's default/home directory.
                        let home = backend.home_dir().unwrap_or_else(|_| "/".into());
                        let want = params.initial_remote_dir.trim();
                        let start = if !want.is_empty() && want != "/" {
                            match backend.list_dir(want) {
                                Ok(_) => want.to_string(),
                                Err(_) => home.clone(),
                            }
                        } else {
                            home.clone()
                        };
                        let listing = backend
                            .list_dir(&start)
                            .map(|e| finalize_listing(&start, e))
                            .unwrap_or_default();
                        let host = params.host.clone();
                        let username = params.username.clone();
                        state = Some(backend);
                        let _ = event_tx.send(WorkerEvent::Connected {
                            host,
                            username,
                            home_dir: start,
                            listing,
                        });
                    }
                    Err(e) => {
                        let _ = event_tx.send(WorkerEvent::ConnectionFailed(e.to_string()));
                    }
                }
            }

            WorkerCmd::Disconnect => {
                state = None;
                let _ = event_tx.send(WorkerEvent::Disconnected);
            }

            WorkerCmd::ListDir(path) => {
                if let Some(b) = &mut state {
                    match b.list_dir(&path) {
                        Ok(entries) => {
                            let entries = finalize_listing(&path, entries);
                            let _ = event_tx.send(WorkerEvent::DirListing { path, entries });
                        }
                        Err(e) => {
                            let _ = event_tx.send(WorkerEvent::DirError {
                                path,
                                error: e.to_string(),
                            });
                        }
                    }
                }
            }

            WorkerCmd::Download { transfer_id, remote_path, local_path, is_dir } => {
                cancel.store(false, Ordering::Relaxed);
                if let Some(b) = &mut state {
                    let result = if is_dir {
                        download_tree(b.as_mut(), &remote_path, &local_path, &transfer_id, &event_tx, &cancel)
                    } else {
                        download_one(b.as_mut(), &remote_path, &local_path, &transfer_id, &event_tx, &cancel)
                    };
                    match result {
                        Ok(()) => {
                            let _ = event_tx.send(WorkerEvent::TransferComplete { id: transfer_id });
                        }
                        Err(e) => {
                            let _ = event_tx.send(WorkerEvent::TransferFailed {
                                id: transfer_id,
                                error: e.to_string(),
                            });
                        }
                    }
                }
            }

            WorkerCmd::Upload { transfer_id, local_path, remote_path, is_dir } => {
                cancel.store(false, Ordering::Relaxed);
                if let Some(b) = &mut state {
                    let result = if is_dir {
                        upload_tree(b.as_mut(), &local_path, &remote_path, &transfer_id, &event_tx, &cancel)
                    } else {
                        upload_one(b.as_mut(), &local_path, &remote_path, &transfer_id, &event_tx, &cancel)
                    };
                    match result {
                        Ok(()) => {
                            let _ = event_tx.send(WorkerEvent::TransferComplete { id: transfer_id });
                        }
                        Err(e) => {
                            let _ = event_tx.send(WorkerEvent::TransferFailed {
                                id: transfer_id,
                                error: e.to_string(),
                            });
                        }
                    }
                }
            }

            WorkerCmd::Delete { path, is_dir } => {
                if let Some(b) = &mut state {
                    let result = delete_tree(b.as_mut(), &path, is_dir);
                    match result {
                        Ok(_) => {
                            let _ = event_tx.send(WorkerEvent::OperationComplete {
                                op: format!("Deleted {path}"),
                            });
                        }
                        Err(e) => {
                            let _ = event_tx.send(WorkerEvent::OperationFailed {
                                op: format!("Delete {path}"),
                                error: e.to_string(),
                            });
                        }
                    }
                }
            }

            WorkerCmd::Rename { from, to } => {
                if let Some(b) = &mut state {
                    match b.rename(&from, &to) {
                        Ok(_) => {
                            let _ = event_tx.send(WorkerEvent::OperationComplete {
                                op: format!("Renamed {from} → {to}"),
                            });
                        }
                        Err(e) => {
                            let _ = event_tx.send(WorkerEvent::OperationFailed {
                                op: format!("Rename {from}"),
                                error: e.to_string(),
                            });
                        }
                    }
                }
            }

            WorkerCmd::Mkdir(path) => {
                if let Some(b) = &mut state {
                    match b.mkdir(&path) {
                        Ok(_) => {
                            let _ = event_tx.send(WorkerEvent::OperationComplete {
                                op: format!("Created directory {path}"),
                            });
                        }
                        Err(e) => {
                            let _ = event_tx.send(WorkerEvent::OperationFailed {
                                op: format!("Mkdir {path}"),
                                error: e.to_string(),
                            });
                        }
                    }
                }
            }

            WorkerCmd::Chmod { path, mode } => {
                if let Some(b) = &mut state {
                    match b.chmod(&path, mode) {
                        Ok(_) => {
                            let _ = event_tx.send(WorkerEvent::OperationComplete {
                                op: format!("chmod {mode:o} {path}"),
                            });
                        }
                        Err(e) => {
                            let _ = event_tx.send(WorkerEvent::OperationFailed {
                                op: format!("chmod {path}"),
                                error: e.to_string(),
                            });
                        }
                    }
                }
            }

            WorkerCmd::CancelTransfer(_id) => {
                cancel.store(true, Ordering::Relaxed);
            }
        }
    }
}

// ── Transfer orchestration (recursion lives here, not in the backends) ─────────

fn download_one(
    b: &mut dyn Backend,
    remote: &str,
    local: &Path,
    id: &str,
    tx: &Sender<WorkerEvent>,
    cancel: &AtomicBool,
) -> Result<()> {
    let total = b.remote_size(remote).unwrap_or(0);
    let mut prog = Progress::new(id, total, tx, cancel);
    if let Err(e) = b.download_file(remote, local, &mut prog) {
        // Remove the partial local file so a cancelled/failed download doesn't
        // leave a truncated file behind.
        let _ = std::fs::remove_file(local);
        return Err(e);
    }
    prog.finish();
    Ok(())
}

fn download_tree(
    b: &mut dyn Backend,
    remote_dir: &str,
    local_dir: &Path,
    id: &str,
    tx: &Sender<WorkerEvent>,
    cancel: &AtomicBool,
) -> Result<()> {
    // First pass: enumerate every file in the tree and its size.
    let mut files: Vec<(String, PathBuf, u64)> = Vec::new();
    collect_remote_files(b, remote_dir, local_dir, &mut files)?;
    let total: u64 = files.iter().map(|(_, _, s)| *s).sum();

    let mut prog = Progress::new(id, total, tx, cancel);
    std::fs::create_dir_all(local_dir)?;
    for (rpath, lpath, _size) in &files {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("Transfer cancelled"));
        }
        if let Some(parent) = lpath.parent() {
            std::fs::create_dir_all(parent)?;
        }
        b.download_file(rpath, lpath, &mut prog)?;
    }
    prog.finish();
    Ok(())
}

fn collect_remote_files(
    b: &mut dyn Backend,
    remote_dir: &str,
    local_dir: &Path,
    out: &mut Vec<(String, PathBuf, u64)>,
) -> Result<()> {
    for entry in b.list_dir(remote_dir)? {
        if entry.name == ".." || entry.name == "." {
            continue;
        }
        let rpath = join_remote(remote_dir, &entry.name);
        let lpath = local_dir.join(&entry.name);
        if entry.is_dir {
            collect_remote_files(b, &rpath, &lpath, out)?;
        } else {
            out.push((rpath, lpath, entry.size));
        }
    }
    Ok(())
}

fn upload_one(
    b: &mut dyn Backend,
    local: &Path,
    remote: &str,
    id: &str,
    tx: &Sender<WorkerEvent>,
    cancel: &AtomicBool,
) -> Result<()> {
    let total = std::fs::metadata(local).map(|m| m.len()).unwrap_or(0);
    let mut prog = Progress::new(id, total, tx, cancel);
    b.upload_file(local, remote, &mut prog)?;
    prog.finish();
    Ok(())
}

fn upload_tree(
    b: &mut dyn Backend,
    local_dir: &Path,
    remote_dir: &str,
    id: &str,
    tx: &Sender<WorkerEvent>,
    cancel: &AtomicBool,
) -> Result<()> {
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<(PathBuf, String, u64)> = Vec::new();
    collect_local_files(local_dir, remote_dir, &mut dirs, &mut files)?;
    let total: u64 = files.iter().map(|(_, _, s)| *s).sum();

    // Create the destination directory tree first (ignore "already exists").
    let _ = b.mkdir(remote_dir);
    for d in &dirs {
        let _ = b.mkdir(d);
    }

    let mut prog = Progress::new(id, total, tx, cancel);
    for (lpath, rpath, _size) in &files {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("Transfer cancelled"));
        }
        b.upload_file(lpath, rpath, &mut prog)?;
    }
    prog.finish();
    Ok(())
}

fn collect_local_files(
    local_dir: &Path,
    remote_dir: &str,
    dirs: &mut Vec<String>,
    files: &mut Vec<(PathBuf, String, u64)>,
) -> Result<()> {
    for entry in std::fs::read_dir(local_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let lpath = entry.path();
        let rpath = join_remote(remote_dir, &name);
        // Use symlink-agnostic type first, then follow for regular files.
        let ft = entry.file_type()?;
        if ft.is_dir() {
            dirs.push(rpath.clone());
            collect_local_files(&lpath, &rpath, dirs, files)?;
        } else if ft.is_file() {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            files.push((lpath, rpath, size));
        }
        // symlinks and special files are skipped
    }
    Ok(())
}

fn delete_tree(b: &mut dyn Backend, path: &str, is_dir: bool) -> Result<()> {
    if is_dir {
        for entry in b.list_dir(path)? {
            if entry.name == ".." || entry.name == "." {
                continue;
            }
            let child = join_remote(path, &entry.name);
            delete_tree(b, &child, entry.is_dir)?;
        }
        b.delete_dir(path)
    } else {
        b.delete_file(path)
    }
}

// ── Connection factory ────────────────────────────────────────────────────────

fn do_connect(params: &ConnectionParams) -> Result<Box<dyn Backend>> {
    match params.protocol {
        Protocol::Sftp => Ok(Box::new(SftpBackend::connect(params)?)),
        Protocol::Scp => Ok(Box::new(ScpBackend::connect(params)?)),
        Protocol::Ftp | Protocol::Ftps => Ok(Box::new(FtpBackend::connect(params)?)),
    }
}

/// Shared SSH session setup for the SFTP and SCP backends.
pub(crate) fn ssh_session(params: &ConnectionParams) -> Result<ssh2::Session> {
    use ssh2::Session;
    use std::net::TcpStream;

    let addr = format!("{}:{}", params.host, params.port);
    let sock_addr = addr
        .to_socket_addrs_first()
        .ok_or_else(|| anyhow!("Could not resolve address: {addr}"))?;
    let tcp = TcpStream::connect_timeout(&sock_addr, Duration::from_secs(params.timeout_secs))?;
    tcp.set_read_timeout(Some(Duration::from_secs(60)))?;

    let mut session = Session::new()?;
    session.set_tcp_stream(tcp);
    session.handshake()?;

    match &params.auth_method {
        AuthMethod::Password => {
            session.userauth_password(&params.username, &params.password)?;
        }
        AuthMethod::PublicKey { key_path } => {
            session.userauth_pubkey_file(&params.username, None, key_path, None)?;
        }
        AuthMethod::Agent => {
            let mut agent = session.agent()?;
            agent.connect()?;
            agent.list_identities()?;
            let identities = agent.identities()?;
            let mut authed = false;
            for identity in &identities {
                if agent.userauth(&params.username, identity).is_ok() {
                    authed = true;
                    break;
                }
            }
            if !authed {
                return Err(anyhow!("SSH agent authentication failed"));
            }
        }
        AuthMethod::KeyboardInteractive => {
            let pw = params.password.clone();
            session.userauth_keyboard_interactive(&params.username, &mut KeyboardHandler(pw))?;
        }
    }

    if !session.authenticated() {
        return Err(anyhow!("Authentication failed"));
    }
    Ok(session)
}

/// Small helper so callers don't need to import `ToSocketAddrs` everywhere and
/// so we resolve host names (not just literal IPs) — the previous code only
/// accepted literal `IP:port` because it used `str::parse::<SocketAddr>`.
trait ResolveFirst {
    fn to_socket_addrs_first(&self) -> Option<std::net::SocketAddr>;
}
impl ResolveFirst for String {
    fn to_socket_addrs_first(&self) -> Option<std::net::SocketAddr> {
        use std::net::ToSocketAddrs;
        self.to_socket_addrs().ok().and_then(|mut it| it.next())
    }
}

struct KeyboardHandler(String);

impl ssh2::KeyboardInteractivePrompt for KeyboardHandler {
    fn prompt(
        &mut self,
        _username: &str,
        _instructions: &str,
        prompts: &[ssh2::Prompt<'_>],
    ) -> Vec<String> {
        prompts.iter().map(|_| self.0.clone()).collect()
    }
}

// ── Shared helpers ─────────────────────────────────────────────────────────────

/// Join a remote directory and a child name using POSIX separators.
pub(crate) fn join_remote(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_string()
    } else if base == "/" {
        format!("/{name}")
    } else if base.ends_with('/') {
        format!("{base}{name}")
    } else {
        format!("{base}/{name}")
    }
}

/// Turn a raw backend listing into what the UI shows: prepend a synthetic ".."
/// entry (unless we're at the root) and sort directories first, then by name.
pub(crate) fn finalize_listing(path: &str, mut entries: Vec<FileEntry>) -> Vec<FileEntry> {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    if !is_remote_root(path) {
        entries.insert(
            0,
            FileEntry {
                name: "..".to_string(),
                size: 0,
                modified: None,
                is_dir: true,
                is_symlink: false,
                permissions: None,
                owner: None,
                group: None,
                link_target: None,
            },
        );
    }
    entries
}

fn is_remote_root(path: &str) -> bool {
    path.is_empty() || path == "/"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_remote_root() {
        assert_eq!(join_remote("/", "etc"), "/etc");
    }

    #[test]
    fn join_remote_nested() {
        assert_eq!(join_remote("/home/user", "file.txt"), "/home/user/file.txt");
    }

    #[test]
    fn join_remote_trailing_slash() {
        assert_eq!(join_remote("/home/user/", "file.txt"), "/home/user/file.txt");
    }

    #[test]
    fn finalize_adds_dotdot_off_root() {
        let entries = vec![FileEntry {
            name: "b".into(),
            size: 0,
            modified: None,
            is_dir: false,
            is_symlink: false,
            permissions: None,
            owner: None,
            group: None,
            link_target: None,
        }];
        let out = finalize_listing("/home", entries);
        assert_eq!(out[0].name, "..");
    }

    #[test]
    fn finalize_no_dotdot_at_root() {
        let out = finalize_listing("/", vec![]);
        assert!(out.iter().all(|e| e.name != ".."));
    }

    #[test]
    fn finalize_sorts_dirs_first() {
        let mk = |name: &str, is_dir: bool| FileEntry {
            name: name.into(),
            size: 0,
            modified: None,
            is_dir,
            is_symlink: false,
            permissions: None,
            owner: None,
            group: None,
            link_target: None,
        };
        let out = finalize_listing("/", vec![mk("zzz", false), mk("aaa", true)]);
        assert_eq!(out[0].name, "aaa"); // dir first even though alphabetically later
        assert_eq!(out[1].name, "zzz");
    }
}
