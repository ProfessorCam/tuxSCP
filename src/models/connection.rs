use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Protocol {
    Sftp,
    Scp,
    Ftp,
    Ftps,
}

impl Protocol {
    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Sftp | Protocol::Scp => 22,
            Protocol::Ftp => 21,
            Protocol::Ftps => 990,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Protocol::Sftp => "SFTP",
            Protocol::Scp => "SCP",
            Protocol::Ftp => "FTP",
            Protocol::Ftps => "FTPS",
        }
    }

    pub fn all() -> &'static [Protocol] {
        &[Protocol::Sftp, Protocol::Scp, Protocol::Ftp, Protocol::Ftps]
    }
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthMethod {
    Password,
    PublicKey { key_path: PathBuf },
    Agent,
    KeyboardInteractive,
}

impl AuthMethod {
    pub fn label(&self) -> &'static str {
        match self {
            AuthMethod::Password => "Password",
            AuthMethod::PublicKey { .. } => "Public Key",
            AuthMethod::Agent => "SSH Agent",
            AuthMethod::KeyboardInteractive => "Keyboard Interactive",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionParams {
    pub host: String,
    pub port: u16,
    pub username: String,
    // Passwords are never written to disk — skip during serialization.
    #[serde(skip)]
    pub password: String,
    pub auth_method: AuthMethod,
    pub protocol: Protocol,
    pub initial_remote_dir: String,
    pub initial_local_dir: Option<PathBuf>,
    pub timeout_secs: u64,
}

impl Default for ConnectionParams {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 22,
            username: String::from(""),
            password: String::new(),
            auth_method: AuthMethod::Password,
            protocol: Protocol::Sftp,
            initial_remote_dir: String::from("/"),
            initial_local_dir: None,
            timeout_secs: 30,
        }
    }
}
