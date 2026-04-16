//! Background worker thread that owns the SSH/SFTP session.
//! ssh2::Session is not Send, so all operations run on one dedicated thread.

use crate::models::{AuthMethod, ConnectionParams, FileEntry, Protocol};
use anyhow::{anyhow, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

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
    },
    Upload {
        transfer_id: String,
        local_path: PathBuf,
        remote_path: String,
    },
    Delete(String),
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

// ── Worker thread ─────────────────────────────────────────────────────────────

fn worker_thread(
    cmd_rx: Receiver<WorkerCmd>,
    event_tx: Sender<WorkerEvent>,
    cancel: Arc<AtomicBool>,
) {
    let mut state: Option<SftpState> = None;

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            WorkerCmd::Quit => break,

            WorkerCmd::Connect(params) => {
                // Drop existing connection
                state = None;
                match do_connect(&params) {
                    Ok(mut s) => {
                        let home = s.pwd().unwrap_or_else(|_| "/".into());
                        let listing = s.list_dir(&home).unwrap_or_default();
                        let host = params.host.clone();
                        let username = params.username.clone();
                        state = Some(s);
                        let _ = event_tx.send(WorkerEvent::Connected {
                            host,
                            username,
                            home_dir: home.clone(),
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
                if let Some(s) = &mut state {
                    match s.list_dir(&path) {
                        Ok(entries) => {
                            let _ = event_tx
                                .send(WorkerEvent::DirListing { path, entries });
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

            WorkerCmd::Download { transfer_id, remote_path, local_path } => {
                cancel.store(false, Ordering::Relaxed);
                if let Some(s) = &mut state {
                    let result = s.download(
                        &transfer_id,
                        &remote_path,
                        &local_path,
                        &event_tx,
                        &cancel,
                    );
                    if let Err(e) = result {
                        let _ = event_tx.send(WorkerEvent::TransferFailed {
                            id: transfer_id,
                            error: e.to_string(),
                        });
                    }
                }
            }

            WorkerCmd::Upload { transfer_id, local_path, remote_path } => {
                cancel.store(false, Ordering::Relaxed);
                if let Some(s) = &mut state {
                    let result = s.upload(
                        &transfer_id,
                        &local_path,
                        &remote_path,
                        &event_tx,
                        &cancel,
                    );
                    if let Err(e) = result {
                        let _ = event_tx.send(WorkerEvent::TransferFailed {
                            id: transfer_id,
                            error: e.to_string(),
                        });
                    }
                }
            }

            WorkerCmd::Delete(path) => {
                if let Some(s) = &mut state {
                    let result = s.delete(&path);
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
                if let Some(s) = &mut state {
                    let result = s.rename(&from, &to);
                    match result {
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
                if let Some(s) = &mut state {
                    match s.mkdir(&path) {
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
                if let Some(s) = &mut state {
                    match s.chmod(&path, mode) {
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

// ── SFTP session wrapper ──────────────────────────────────────────────────────

struct SftpState {
    _session: ssh2::Session, // keep alive
    sftp: ssh2::Sftp,
    protocol: Protocol,
}

fn do_connect(params: &ConnectionParams) -> Result<SftpState> {
    use ssh2::Session;
    use std::net::TcpStream;
    use std::time::Duration;

    let addr = format!("{}:{}", params.host, params.port);
    let tcp = TcpStream::connect_timeout(
        &addr.parse().map_err(|_| anyhow!("Invalid address: {addr}"))?,
        Duration::from_secs(params.timeout_secs),
    )?;
    tcp.set_read_timeout(Some(Duration::from_secs(30)))?;

    let mut session = Session::new()?;
    session.set_tcp_stream(tcp);
    session.handshake()?;

    // Authenticate
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

    let sftp = session.sftp()?;
    Ok(SftpState { _session: session, sftp, protocol: params.protocol })
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

impl SftpState {
    fn pwd(&mut self) -> Result<String> {
        // Attempt to get home dir via realpath(".")
        let path = self.sftp.realpath(Path::new("."))?;
        Ok(path.to_string_lossy().to_string())
    }

    fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>> {
        let entries = self.sftp.readdir(Path::new(path))?;
        let mut files: Vec<FileEntry> = entries
            .into_iter()
            .filter_map(|(p, stat)| {
                let name = p.file_name()?.to_string_lossy().to_string();
                Some(FileEntry {
                    name,
                    size: stat.size.unwrap_or(0),
                    modified: stat.mtime.and_then(|t| {
                        chrono::DateTime::from_timestamp(t as i64, 0)
                            .map(|dt| dt.with_timezone(&chrono::Local))
                    }),
                    is_dir: stat.is_dir(),
                    is_symlink: stat.file_type().is_symlink(),
                    permissions: stat.perm,
                    owner: None,
                    group: None,
                    link_target: None,
                })
            })
            .collect();

        // Add ".." entry unless we're at root
        if path != "/" {
            files.insert(
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

        // Sort: dirs first, then by name
        files[if path != "/" { 1 } else { 0 }..].sort_by(|a, b| {
            b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        Ok(files)
    }

    fn download(
        &mut self,
        id: &str,
        remote_path: &str,
        local_path: &Path,
        event_tx: &Sender<WorkerEvent>,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let stat = self.sftp.stat(Path::new(remote_path))?;
        let total = stat.size.unwrap_or(0);

        let mut remote_file = self.sftp.open(Path::new(remote_path))?;
        let mut local_file = std::fs::File::create(local_path)?;

        let mut transferred = 0u64;
        let mut buf = vec![0u8; 128 * 1024];
        let start = Instant::now();

        loop {
            if cancel.load(Ordering::Relaxed) {
                // Clean up partial file
                drop(local_file);
                let _ = std::fs::remove_file(local_path);
                return Err(anyhow!("Transfer cancelled"));
            }

            let n = remote_file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            local_file.write_all(&buf[..n])?;
            transferred += n as u64;

            let elapsed = start.elapsed().as_secs_f64();
            let speed = if elapsed > 0.1 { transferred as f64 / elapsed } else { 0.0 };

            let _ = event_tx.send(WorkerEvent::TransferProgress {
                id: id.to_string(),
                transferred,
                total,
                speed_bps: speed,
            });
        }

        let _ = event_tx.send(WorkerEvent::TransferComplete { id: id.to_string() });
        Ok(())
    }

    fn upload(
        &mut self,
        id: &str,
        local_path: &Path,
        remote_path: &str,
        event_tx: &Sender<WorkerEvent>,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let total = std::fs::metadata(local_path)?.len();
        let mut local_file = std::fs::File::open(local_path)?;
        let mut remote_file = self
            .sftp
            .create(Path::new(remote_path))?;

        let mut transferred = 0u64;
        let mut buf = vec![0u8; 128 * 1024];
        let start = Instant::now();

        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(anyhow!("Transfer cancelled"));
            }

            let n = local_file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            remote_file.write_all(&buf[..n])?;
            transferred += n as u64;

            let elapsed = start.elapsed().as_secs_f64();
            let speed = if elapsed > 0.1 { transferred as f64 / elapsed } else { 0.0 };

            let _ = event_tx.send(WorkerEvent::TransferProgress {
                id: id.to_string(),
                transferred,
                total,
                speed_bps: speed,
            });
        }

        let _ = event_tx.send(WorkerEvent::TransferComplete { id: id.to_string() });
        Ok(())
    }

    fn delete(&mut self, path: &str) -> Result<()> {
        let stat = self.sftp.stat(Path::new(path))?;
        if stat.is_dir() {
            self.sftp.rmdir(Path::new(path))?;
        } else {
            self.sftp.unlink(Path::new(path))?;
        }
        Ok(())
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        self.sftp.rename(Path::new(from), Path::new(to), None)?;
        Ok(())
    }

    fn mkdir(&mut self, path: &str) -> Result<()> {
        self.sftp.mkdir(Path::new(path), 0o755)?;
        Ok(())
    }

    fn chmod(&mut self, path: &str, mode: u32) -> Result<()> {
        let stat = ssh2::FileStat {
            size: None,
            uid: None,
            gid: None,
            perm: Some(mode),
            atime: None,
            mtime: None,
        };
        self.sftp.setstat(Path::new(path), stat)?;
        Ok(())
    }
}
