//! SCP backend.
//!
//! SCP is only a file-copy protocol — it has no notion of listing directories
//! or manipulating metadata. So this backend does the actual byte transfers
//! with the real SCP protocol (`scp_recv` / `scp_send`) and satisfies every
//! other operation by running ordinary shell commands (`ls`, `mv`, `rm`,
//! `mkdir`, `chmod`) over an SSH exec channel — the same approach WinSCP's SCP
//! mode uses. It therefore works against servers that expose SCP + a shell but
//! no SFTP subsystem.

use super::{copy_with_progress, ssh_session, Backend, Progress};
use crate::models::{ConnectionParams, FileEntry};
use anyhow::{anyhow, Result};
use std::io::Read;
use std::path::Path;

pub struct ScpBackend {
    session: ssh2::Session,
}

impl ScpBackend {
    pub fn connect(params: &ConnectionParams) -> Result<Self> {
        Ok(Self { session: ssh_session(params)? })
    }

    /// Run a command and return its stdout, erroring on a non-zero exit code.
    fn exec(&self, cmd: &str) -> Result<String> {
        let mut ch = self.session.channel_session()?;
        ch.exec(cmd)?;
        let mut out = String::new();
        ch.read_to_string(&mut out)?;
        let mut err = String::new();
        // stderr is best-effort — used only to build a nicer error message.
        let _ = ch.stderr().read_to_string(&mut err);
        ch.wait_close().ok();
        let code = ch.exit_status().unwrap_or(-1);
        if code != 0 {
            let msg = err.trim();
            return Err(anyhow!(
                "remote command failed (exit {code}): {}",
                if msg.is_empty() { cmd } else { msg }
            ));
        }
        Ok(out)
    }
}

impl Backend for ScpBackend {
    fn home_dir(&mut self) -> Result<String> {
        let out = self.exec("pwd")?;
        let dir = out.trim();
        Ok(if dir.is_empty() { "/".into() } else { dir.to_string() })
    }

    fn list_dir(&mut self, path: &str) -> Result<Vec<FileEntry>> {
        let cmd = format!("ls -la --time-style=long-iso -- {}", shell_quote(path));
        let out = self.exec(&cmd)?;
        Ok(out.lines().filter_map(parse_ls_line).collect())
    }

    fn remote_size(&mut self, path: &str) -> Result<u64> {
        let out = self.exec(&format!("stat -c %s -- {}", shell_quote(path)))?;
        out.trim()
            .parse::<u64>()
            .map_err(|_| anyhow!("could not read remote file size"))
    }

    fn download_file(&mut self, remote: &str, local: &Path, prog: &mut Progress) -> Result<()> {
        let (mut channel, _stat) = self.session.scp_recv(Path::new(remote))?;
        let mut local_file = std::fs::File::create(local)?;
        copy_with_progress(&mut channel, &mut local_file, prog)?;
        // Cleanly tear down the SCP channel.
        channel.send_eof().ok();
        channel.wait_eof().ok();
        channel.close().ok();
        channel.wait_close().ok();
        Ok(())
    }

    fn upload_file(&mut self, local: &Path, remote: &str, prog: &mut Progress) -> Result<()> {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(local)?;
        let size = meta.len();
        let mode = (meta.mode() & 0o777) as i32;
        let mut local_file = std::fs::File::open(local)?;
        let mut channel = self.session.scp_send(Path::new(remote), mode, size, None)?;
        copy_with_progress(&mut local_file, &mut channel, prog)?;
        channel.send_eof()?;
        channel.wait_eof()?;
        channel.close()?;
        channel.wait_close()?;
        Ok(())
    }

    fn delete_file(&mut self, path: &str) -> Result<()> {
        self.exec(&format!("rm -f -- {}", shell_quote(path))).map(|_| ())
    }

    fn delete_dir(&mut self, path: &str) -> Result<()> {
        self.exec(&format!("rmdir -- {}", shell_quote(path))).map(|_| ())
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<()> {
        self.exec(&format!("mv -- {} {}", shell_quote(from), shell_quote(to)))
            .map(|_| ())
    }

    fn mkdir(&mut self, path: &str) -> Result<()> {
        self.exec(&format!("mkdir -p -- {}", shell_quote(path))).map(|_| ())
    }

    fn chmod(&mut self, path: &str, mode: u32) -> Result<()> {
        self.exec(&format!("chmod {:o} -- {}", mode & 0o7777, shell_quote(path)))
            .map(|_| ())
    }
}

/// Single-quote a string for safe use as one shell argument.
pub(crate) fn shell_quote(s: &str) -> String {
    // Wrap in single quotes and escape any embedded single quote as '\''
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Parse a single line of `ls -la --time-style=long-iso` output into a
/// [`FileEntry`]. Returns `None` for the `total N` header, `.`/`..` and any line
/// that doesn't look like a directory entry.
pub(crate) fn parse_ls_line(line: &str) -> Option<FileEntry> {
    let line = line.trim_end();
    if line.is_empty() || line.starts_with("total ") {
        return None;
    }

    // Fields: perms links owner group size date time name...
    // The name (token 8+) may contain spaces, so we tokenise for the fixed
    // leading fields but recover the name from the raw line below.
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 8 {
        return None;
    }

    let perms = tokens[0];
    if perms.len() < 10 {
        return None;
    }
    let type_char = perms.chars().next()?;
    if !matches!(type_char, 'd' | '-' | 'l' | 'c' | 'b' | 's' | 'p') {
        return None;
    }

    let size = tokens[4].parse::<u64>().unwrap_or(0);
    let date = tokens[5];
    let time = tokens[6];

    // Name is everything from token 7 onward (may contain spaces).
    // Recover it from the original line to preserve internal spacing.
    let name_start = {
        // find the start of the 8th whitespace-separated field
        let mut count = 0;
        let mut idx = 0;
        let mut in_ws = true;
        for (i, ch) in line.char_indices() {
            if ch.is_whitespace() {
                in_ws = true;
            } else {
                if in_ws {
                    count += 1;
                    if count == 8 {
                        idx = i;
                        break;
                    }
                }
                in_ws = false;
            }
        }
        idx
    };
    let raw_name = &line[name_start..];

    let is_symlink = type_char == 'l';
    let (name, link_target) = if is_symlink {
        match raw_name.split_once(" -> ") {
            Some((n, t)) => (n.to_string(), Some(t.to_string())),
            None => (raw_name.to_string(), None),
        }
    } else {
        (raw_name.to_string(), None)
    };

    if name == "." || name == ".." || name.is_empty() {
        return None;
    }

    let is_dir = type_char == 'd';
    let permissions = Some(parse_mode_str(perms));

    let modified = parse_ls_datetime(date, time);

    Some(FileEntry {
        name,
        size,
        modified,
        is_dir,
        is_symlink,
        permissions,
        owner: Some(tokens[2].to_string()),
        group: Some(tokens[3].to_string()),
        link_target,
    })
}

/// Convert a `drwxr-xr-x`-style permission string into numeric mode bits
/// (including the file-type bits so `format_permissions` renders the type).
pub(crate) fn parse_mode_str(perms: &str) -> u32 {
    let chars: Vec<char> = perms.chars().collect();
    if chars.len() < 10 {
        return 0;
    }
    let mut mode: u32 = match chars[0] {
        'd' => 0o040000,
        'l' => 0o120000,
        'c' => 0o020000,
        'b' => 0o060000,
        's' => 0o140000,
        'p' => 0o010000,
        _ => 0o100000, // regular file
    };
    // rwx triplets for user/group/other
    let bits = [
        (1, 0o400),
        (2, 0o200),
        (3, 0o100),
        (4, 0o040),
        (5, 0o020),
        (6, 0o010),
        (7, 0o004),
        (8, 0o002),
        (9, 0o001),
    ];
    for (i, bit) in bits {
        match chars[i] {
            '-' => {}
            // setuid/setgid/sticky variants still imply the base permission
            _ => mode |= bit,
        }
    }
    // setuid / setgid / sticky
    if matches!(chars[3], 's' | 'S') {
        mode |= 0o4000;
    }
    if matches!(chars[6], 's' | 'S') {
        mode |= 0o2000;
    }
    if matches!(chars[9], 't' | 'T') {
        mode |= 0o1000;
    }
    mode
}

fn parse_ls_datetime(date: &str, time: &str) -> Option<chrono::DateTime<chrono::Local>> {
    use chrono::TimeZone;
    let d = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    // Time may be HH:MM or HH:MM:SS depending on --time-style resolution.
    let t = chrono::NaiveTime::parse_from_str(time, "%H:%M:%S")
        .or_else(|_| chrono::NaiveTime::parse_from_str(time, "%H:%M"))
        .ok()?;
    let ndt = d.and_time(t);
    chrono::Local.from_local_datetime(&ndt).single()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_plain() {
        assert_eq!(shell_quote("/etc/hosts"), "'/etc/hosts'");
    }

    #[test]
    fn quote_spaces_and_specials() {
        assert_eq!(shell_quote("a b;rm -rf /"), "'a b;rm -rf /'");
    }

    #[test]
    fn quote_embedded_single_quote() {
        // don't -> 'don'\''t'
        assert_eq!(shell_quote("don't"), "'don'\\''t'");
    }

    #[test]
    fn parse_regular_file() {
        let e = parse_ls_line("-rw-r--r-- 1 user group 1024 2024-01-15 12:30 file.txt").unwrap();
        assert_eq!(e.name, "file.txt");
        assert_eq!(e.size, 1024);
        assert!(!e.is_dir);
        assert!(!e.is_symlink);
        assert!(e.modified.is_some());
        assert_eq!(e.permissions.unwrap() & 0o777, 0o644);
    }

    #[test]
    fn parse_directory() {
        let e = parse_ls_line("drwxr-xr-x 2 user group 4096 2024-01-15 12:30 mydir").unwrap();
        assert_eq!(e.name, "mydir");
        assert!(e.is_dir);
        assert_eq!(e.permissions.unwrap() & 0o777, 0o755);
    }

    #[test]
    fn parse_name_with_spaces() {
        let e = parse_ls_line("-rw-r--r-- 1 u g 5 2024-01-15 12:30 my cool file.txt").unwrap();
        assert_eq!(e.name, "my cool file.txt");
    }

    #[test]
    fn parse_symlink() {
        let e = parse_ls_line("lrwxrwxrwx 1 u g 9 2024-01-15 12:30 link -> targetfile").unwrap();
        assert_eq!(e.name, "link");
        assert!(e.is_symlink);
        assert_eq!(e.link_target.as_deref(), Some("targetfile"));
    }

    #[test]
    fn skip_total_header() {
        assert!(parse_ls_line("total 24").is_none());
    }

    #[test]
    fn skip_dot_entries() {
        assert!(parse_ls_line("drwxr-xr-x 2 u g 4096 2024-01-15 12:30 .").is_none());
        assert!(parse_ls_line("drwxr-xr-x 2 u g 4096 2024-01-15 12:30 ..").is_none());
    }

    #[test]
    fn mode_string_roundtrip() {
        assert_eq!(parse_mode_str("-rwxr-xr-x") & 0o777, 0o755);
        assert_eq!(parse_mode_str("-rw-------") & 0o777, 0o600);
        assert_eq!(parse_mode_str("drwxrwxrwx") & 0o170000, 0o040000);
    }
}
