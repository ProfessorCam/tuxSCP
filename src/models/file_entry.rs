use chrono::{DateTime, Local};

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub modified: Option<DateTime<Local>>,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub permissions: Option<u32>,
    pub owner: Option<String>,
    pub group: Option<String>,
    pub link_target: Option<String>,
}

impl FileEntry {
    pub fn is_dotdot(&self) -> bool {
        self.name == ".."
    }

    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.') && self.name != ".."
    }

    pub fn size_display(&self) -> String {
        if self.is_dir {
            return String::from("<DIR>");
        }
        human_bytes::human_bytes(self.size as f64)
    }

    pub fn modified_display(&self) -> String {
        match &self.modified {
            Some(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
            None => String::from(""),
        }
    }

    pub fn permissions_display(&self) -> String {
        match self.permissions {
            Some(p) => format_permissions(p),
            None => String::from(""),
        }
    }

    pub fn icon(&self) -> &'static str {
        if self.name == ".." {
            return "↩";
        }
        if self.is_dir {
            return "📁";
        }
        if self.is_symlink {
            return "🔗";
        }
        // Extension-based icons
        match extension(&self.name) {
            "rs" => "🦀",
            "toml" | "yaml" | "yml" | "json" => "⚙",
            "sh" | "bash" | "zsh" => "📜",
            "txt" | "md" | "rst" => "📄",
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "webp" => "🖼",
            "mp4" | "mkv" | "avi" | "mov" | "webm" => "🎬",
            "mp3" | "flac" | "ogg" | "wav" => "🎵",
            "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" => "📦",
            "pdf" => "📕",
            "deb" | "rpm" | "appimage" => "📦",
            "py" => "🐍",
            "js" | "ts" => "📜",
            "html" | "css" => "🌐",
            _ => "📄",
        }
    }
}

fn extension(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or("")
}

fn format_permissions(mode: u32) -> String {
    let file_type = if mode & 0o170000 == 0o040000 { 'd' } else { '-' };
    let r = |bit: u32, c: char| if mode & bit != 0 { c } else { '-' };
    format!(
        "{}{}{}{}{}{}{}{}{}{}",
        file_type,
        r(0o400, 'r'),
        r(0o200, 'w'),
        r(0o100, 'x'),
        r(0o040, 'r'),
        r(0o020, 'w'),
        r(0o010, 'x'),
        r(0o004, 'r'),
        r(0o002, 'w'),
        r(0o001, 'x'),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, mode: u32) -> FileEntry {
        FileEntry {
            name: name.into(),
            size: 0,
            modified: None,
            is_dir: mode & 0o170000 == 0o040000,
            is_symlink: false,
            permissions: Some(mode),
            owner: None,
            group: None,
            link_target: None,
        }
    }

    #[test]
    fn permissions_render_like_ls() {
        assert_eq!(file("f", 0o100644).permissions_display(), "-rw-r--r--");
        assert_eq!(file("d", 0o040755).permissions_display(), "drwxr-xr-x");
        assert_eq!(file("x", 0o100755).permissions_display(), "-rwxr-xr-x");
    }

    #[test]
    fn hidden_detection_excludes_dotdot() {
        assert!(file(".bashrc", 0o100644).is_hidden());
        assert!(!file("..", 0o040755).is_hidden());
        assert!(!file("visible", 0o100644).is_hidden());
    }

    #[test]
    fn extension_lookup() {
        assert_eq!(extension("photo.JPG".to_lowercase().as_str()), "jpg");
        assert_eq!(extension("archive.tar.gz"), "gz");
        assert_eq!(extension("Makefile"), "Makefile");
    }

    #[test]
    fn dir_size_display() {
        assert_eq!(file("d", 0o040755).size_display(), "<DIR>");
    }
}
