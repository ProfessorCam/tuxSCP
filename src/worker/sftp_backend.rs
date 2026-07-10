//! SFTP backend — the default, most capable protocol. Uses the SFTP subsystem
//! of an SSH session via `ssh2`.

use super::{copy_with_progress, ssh_session, Backend, Progress};
use crate::models::{ConnectionParams, FileEntry};
use anyhow::Result;
use std::path::Path;

pub struct SftpBackend {
    _session: ssh2::Session, // keep the SSH session alive for the sftp handle
    sftp: ssh2::Sftp,
}

impl SftpBackend {
    pub fn connect(params: &ConnectionParams) -> Result<Self> {
        let session = ssh_session(params)?;
        let sftp = session.sftp()?;
        Ok(Self { _session: session, sftp })
    }
}

impl Backend for SftpBackend {
    fn home_dir(&mut self) -> Result<String> {
        let path = self.sftp.realpath(Path::new("."))?;
        Ok(path.to_string_lossy().to_string())
    }

    fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>> {
        let entries = self.sftp.readdir(Path::new(path))?;
        let files = entries
            .into_iter()
            .filter_map(|(p, stat)| {
                let name = p.file_name()?.to_string_lossy().to_string();
                if name == "." || name == ".." {
                    return None;
                }
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
        Ok(files)
    }

    fn remote_size(&mut self, path: &str) -> Result<u64> {
        let stat = self.sftp.stat(Path::new(path))?;
        Ok(stat.size.unwrap_or(0))
    }

    fn download_file(&mut self, remote: &str, local: &Path, prog: &mut Progress) -> Result<()> {
        let mut remote_file = self.sftp.open(Path::new(remote))?;
        let mut local_file = std::fs::File::create(local)?;
        copy_with_progress(&mut remote_file, &mut local_file, prog)?;
        Ok(())
    }

    fn upload_file(&mut self, local: &Path, remote: &str, prog: &mut Progress) -> Result<()> {
        let mut local_file = std::fs::File::open(local)?;
        let mut remote_file = self.sftp.create(Path::new(remote))?;
        copy_with_progress(&mut local_file, &mut remote_file, prog)?;
        Ok(())
    }

    fn delete_file(&mut self, path: &str) -> Result<()> {
        self.sftp.unlink(Path::new(path))?;
        Ok(())
    }

    fn delete_dir(&mut self, path: &str) -> Result<()> {
        self.sftp.rmdir(Path::new(path))?;
        Ok(())
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        // Overwrite/Atomic flags let rename replace an existing target where the
        // server supports it (matches typical "move" semantics).
        self.sftp.rename(
            Path::new(from),
            Path::new(to),
            Some(ssh2::RenameFlags::OVERWRITE | ssh2::RenameFlags::ATOMIC),
        )?;
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
