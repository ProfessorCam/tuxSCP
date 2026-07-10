use crate::models::ConnectionParams;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSession {
    pub id: String,
    pub name: String,
    pub params: ConnectionParams,
    pub last_used: Option<chrono::DateTime<chrono::Local>>,
}

impl SavedSession {
    pub fn new(name: impl Into<String>, params: ConnectionParams) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            params,
            last_used: None,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SessionStore {
    pub sessions: Vec<SavedSession>,
}

impl SessionStore {
    fn config_path() -> PathBuf {
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("tuxscp").join("sessions.json")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if let Ok(data) = std::fs::read_to_string(&path) {
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            Self::default()
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, data)?;
        Ok(())
    }

    pub fn add_or_update(&mut self, session: SavedSession) {
        if let Some(existing) = self.sessions.iter_mut().find(|s| s.id == session.id) {
            *existing = session;
        } else {
            self.sessions.push(session);
        }
    }

    pub fn remove(&mut self, id: &str) {
        self.sessions.retain(|s| s.id != id);
    }
}
