use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDirection {
    Download, // Remote -> Local
    Upload,   // Local -> Remote
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferStatus {
    Queued,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

impl TransferStatus {
    pub fn label(self) -> &'static str {
        match self {
            TransferStatus::Queued => "Queued",
            TransferStatus::InProgress => "Transferring",
            TransferStatus::Completed => "Done",
            TransferStatus::Failed => "Failed",
            TransferStatus::Cancelled => "Cancelled",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Transfer {
    pub id: String,
    pub direction: TransferDirection,
    pub local_path: PathBuf,
    pub remote_path: String,
    pub filename: String,
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    pub status: TransferStatus,
    pub error: Option<String>,
    pub speed_bps: f64,
    pub started_at: Option<std::time::Instant>,
}

impl Transfer {
    pub fn new_download(remote_path: String, local_path: PathBuf, total_bytes: u64) -> Self {
        let filename = remote_path.rsplit('/').next().unwrap_or(&remote_path).to_string();
        Self {
            id: Uuid::new_v4().to_string(),
            direction: TransferDirection::Download,
            local_path,
            remote_path,
            filename,
            total_bytes,
            transferred_bytes: 0,
            status: TransferStatus::Queued,
            error: None,
            speed_bps: 0.0,
            started_at: None,
        }
    }

    pub fn new_upload(local_path: PathBuf, remote_path: String) -> Self {
        let filename = local_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let total_bytes = std::fs::metadata(&local_path).map(|m| m.len()).unwrap_or(0);
        Self {
            id: Uuid::new_v4().to_string(),
            direction: TransferDirection::Upload,
            local_path,
            remote_path,
            filename,
            total_bytes,
            transferred_bytes: 0,
            status: TransferStatus::Queued,
            error: None,
            speed_bps: 0.0,
            started_at: None,
        }
    }

    pub fn progress(&self) -> f32 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        (self.transferred_bytes as f64 / self.total_bytes as f64) as f32
    }

    pub fn speed_display(&self) -> String {
        if self.speed_bps < 1.0 {
            return String::from("-");
        }
        format!("{}/s", human_bytes::human_bytes(self.speed_bps))
    }

    pub fn eta_display(&self) -> String {
        if self.speed_bps < 1.0 || self.total_bytes == 0 {
            return String::from("-");
        }
        let remaining = self.total_bytes.saturating_sub(self.transferred_bytes);
        let secs = remaining as f64 / self.speed_bps;
        if secs < 60.0 {
            format!("{:.0}s", secs)
        } else if secs < 3600.0 {
            format!("{:.0}m {:.0}s", secs / 60.0, secs % 60.0)
        } else {
            format!("{:.0}h {:.0}m", secs / 3600.0, (secs % 3600.0) / 60.0)
        }
    }
}
