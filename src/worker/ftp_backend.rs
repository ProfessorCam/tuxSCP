//! FTP / FTPS backend built on `suppaftp`.
//!
//! Plain FTP and TLS FTP use different concrete stream types, so the connection
//! is held in a [`Conn`] enum and every operation is dispatched over both arms
//! with the [`with_conn!`] macro (the method names are identical on both, and
//! `DataStream` implements `Read`/`Write`, so one body type-checks for both).

use super::{copy_with_progress, Backend, Progress};
use crate::models::{ConnectionParams, FileEntry, Protocol};
use anyhow::{anyhow, Result};
use std::path::Path;
use suppaftp::list::File as FtpFile;
use suppaftp::native_tls::TlsConnector as NativeTls;
use suppaftp::types::FileType;
use suppaftp::{FtpStream, NativeTlsConnector, NativeTlsFtpStream};

enum Conn {
    Plain(FtpStream),
    Tls(NativeTlsFtpStream),
}

/// Run `$body` against whichever concrete FTP stream we hold. `$s` is bound to
/// `&mut FtpStream` or `&mut NativeTlsFtpStream`; both expose the same methods.
macro_rules! with_conn {
    ($self:expr, $s:ident, $body:block) => {
        match &mut $self.conn {
            Conn::Plain($s) => $body,
            Conn::Tls($s) => $body,
        }
    };
}

pub struct FtpBackend {
    conn: Conn,
}

impl FtpBackend {
    pub fn connect(params: &ConnectionParams) -> Result<Self> {
        let addr = format!("{}:{}", params.host, params.port);

        let mut conn = if params.protocol == Protocol::Ftps {
            let connector = NativeTlsConnector::from(
                NativeTls::new().map_err(|e| anyhow!("TLS init failed: {e}"))?,
            );
            // Port 990 is conventionally implicit FTPS; anything else uses the
            // modern explicit (AUTH TLS) upgrade on the control channel.
            let stream = if params.port == 990 {
                NativeTlsFtpStream::connect_secure_implicit(&addr, connector, &params.host)
                    .map_err(|e| anyhow!("FTPS connect failed: {e}"))?
            } else {
                NativeTlsFtpStream::connect(&addr)
                    .map_err(|e| anyhow!("FTP connect failed: {e}"))?
                    .into_secure(connector, &params.host)
                    .map_err(|e| anyhow!("TLS upgrade failed: {e}"))?
            };
            Conn::Tls(stream)
        } else {
            let stream = FtpStream::connect(&addr).map_err(|e| anyhow!("FTP connect failed: {e}"))?;
            Conn::Plain(stream)
        };

        // Authenticate and switch to binary mode so transfers are byte-exact.
        match &mut conn {
            Conn::Plain(s) => {
                s.login(&params.username, &params.password)
                    .map_err(|e| anyhow!("Login failed: {e}"))?;
                let _ = s.transfer_type(FileType::Binary);
            }
            Conn::Tls(s) => {
                s.login(&params.username, &params.password)
                    .map_err(|e| anyhow!("Login failed: {e}"))?;
                let _ = s.transfer_type(FileType::Binary);
            }
        }

        Ok(Self { conn })
    }
}

impl Backend for FtpBackend {
    fn home_dir(&mut self) -> Result<String> {
        let dir = with_conn!(self, s, { s.pwd()? });
        Ok(if dir.is_empty() { "/".into() } else { dir })
    }

    fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>> {
        let lines = with_conn!(self, s, { s.list(Some(path))? });
        Ok(lines.iter().filter_map(|l| ftp_line_to_entry(l)).collect())
    }

    fn remote_size(&mut self, path: &str) -> Result<u64> {
        let n = with_conn!(self, s, { s.size(path)? });
        Ok(n as u64)
    }

    fn download_file(&mut self, remote: &str, local: &Path, prog: &mut Progress) -> Result<()> {
        let mut local_file = std::fs::File::create(local)?;
        with_conn!(self, s, {
            let mut stream = s.retr_as_stream(remote)?;
            copy_with_progress(&mut stream, &mut local_file, prog)?;
            s.finalize_retr_stream(stream)?;
        });
        Ok(())
    }

    fn upload_file(&mut self, local: &Path, remote: &str, prog: &mut Progress) -> Result<()> {
        let mut local_file = std::fs::File::open(local)?;
        with_conn!(self, s, {
            let mut stream = s.put_with_stream(remote)?;
            copy_with_progress(&mut local_file, &mut stream, prog)?;
            s.finalize_put_stream(stream)?;
        });
        Ok(())
    }

    fn delete_file(&mut self, path: &str) -> Result<()> {
        with_conn!(self, s, { s.rm(path)? });
        Ok(())
    }

    fn delete_dir(&mut self, path: &str) -> Result<()> {
        with_conn!(self, s, { s.rmdir(path)? });
        Ok(())
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        with_conn!(self, s, { s.rename(from, to)? });
        Ok(())
    }

    fn mkdir(&mut self, path: &str) -> Result<()> {
        with_conn!(self, s, { s.mkdir(path)? });
        Ok(())
    }

    fn chmod(&mut self, path: &str, mode: u32) -> Result<()> {
        // Not part of the FTP standard; many servers support SITE CHMOD.
        let cmd = format!("CHMOD {:o} {}", mode & 0o7777, path);
        with_conn!(self, s, {
            s.site(cmd.clone())
                .map_err(|e| anyhow!("SITE CHMOD not supported by server: {e}"))?
        });
        Ok(())
    }
}

/// Parse one raw FTP LIST line into a [`FileEntry`] (best-effort; unparseable
/// lines are dropped).
fn ftp_line_to_entry(line: &str) -> Option<FileEntry> {
    let f: FtpFile = line.parse().ok()?;
    let name = f.name().to_string();
    if name == "." || name == ".." || name.is_empty() {
        return None;
    }
    let modified = Some(chrono::DateTime::<chrono::Local>::from(f.modified()));
    Some(FileEntry {
        name,
        size: f.size() as u64,
        modified,
        is_dir: f.is_directory(),
        is_symlink: f.is_symlink(),
        permissions: None, // FTP permission reporting is inconsistent; omit
        owner: f.uid().map(|u| u.to_string()),
        group: f.gid().map(|g| g.to_string()),
        link_target: f.symlink().map(|p| p.to_string_lossy().to_string()),
    })
}
