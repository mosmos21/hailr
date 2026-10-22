use std::time::Duration;

use crate::protocol::PaneId;

/// 端末マルチプレクサへの依存をここに閉じる。外部コマンドを呼ぶだけなので同期にしている
pub trait Mux: Send + Sync {
    fn list_panes(&self) -> Result<Vec<PaneInfo>, MuxError>;
    /// ANSI のエスケープを含む、表示中の画面
    fn read_pane(&self, pane: &PaneId) -> Result<String, MuxError>;
    fn focus(&self, pane: &PaneId) -> Result<(), MuxError>;
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

/// `command` にはコマンドとサブコマンドの名前だけを入れる。質問文など利用者由来の引数はログに残さない
#[derive(Debug, thiserror::Error)]
pub enum MuxError {
    #[error("failed to start `{command}`")]
    Spawn {
        command: String,
        source: std::io::Error,
    },
    #[error("failed to wait for `{command}`")]
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
    #[error("invalid pane id")]
    InvalidPane,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_status_from_herdr() {
        assert_eq!(AgentStatus::from("idle"), AgentStatus::Idle);
        assert_eq!(AgentStatus::from("working"), AgentStatus::Working);
        assert_eq!(AgentStatus::from("blocked"), AgentStatus::Blocked);
        assert_eq!(AgentStatus::from("Idle"), AgentStatus::Other("Idle".into()));
    }
}
