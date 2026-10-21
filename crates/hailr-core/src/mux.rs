use std::process::Command;
use std::time::Duration;

use crate::protocol::PaneId;

/// 端末マルチプレクサへの依存をここに閉じる。外部コマンドを呼ぶだけなので同期にしている
pub trait Mux: Send + Sync {
    fn list_panes(&self) -> Result<Vec<PaneInfo>, MuxError>;
    /// ANSI のエスケープを含む、表示中の画面
    fn read_pane(&self, pane: &PaneId) -> Result<String, MuxError>;
    fn attach_command(&self, pane: &PaneId) -> Command;
    fn notify(&self, title: &str, body: &str) -> Result<(), MuxError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneInfo {
    pub id: PaneId,
    pub status: AgentStatus,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Other(String),
}

impl From<&str> for AgentStatus {
    fn from(s: &str) -> Self {
        match s {
            "idle" => Self::Idle,
            "working" => Self::Working,
            "blocked" => Self::Blocked,
            other => Self::Other(other.to_owned()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MuxError {
    #[error("failed to start `{command}`: {source}")]
    Spawn {
        command: String,
        source: std::io::Error,
    },
    #[error("failed to wait for `{command}`: {source}")]
    Wait {
        command: String,
        source: std::io::Error,
    },
    #[error("`{command}` did not finish within {timeout:?}")]
    Timeout { command: String, timeout: Duration },
    #[error("`{command}` failed ({status}): {stderr}")]
    Failed {
        command: String,
        status: String,
        stderr: String,
    },
    #[error("cannot parse the output of `{command}`: {message}")]
    Parse { command: String, message: String },
    #[error("invalid pane id: {0:?}")]
    InvalidPane(String),
}
