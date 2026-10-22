use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `ask_human` の `options` と `links` の上限。ask-mcp と broker の両方で検査する
pub const MAX_OPTIONS: usize = 9;
pub const MAX_LINKS: usize = 5;

pub const PERMISSION_ALLOW: &str = "Allow";
pub const PERMISSION_DENY: &str = "Deny";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QuestionId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientId(pub Uuid);

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PaneId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub id: QuestionId,
    pub client_id: ClientId,
    pub pane_id: PaneId,
    /// ask-mcp が質問を作った時刻（unix ms）
    pub asked_at: u64,
    pub body: QuestionBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum QuestionBody {
    Ask {
        title: String,
        question: String,
        #[serde(default)]
        options: Vec<String>,
        #[serde(default)]
        links: Vec<String>,
    },
    Permission {
        request_id: String,
        tool_name: String,
        description: String,
        input_preview: String,
    },
}

impl QuestionBody {
    pub fn title(&self) -> String {
        match self {
            Self::Ask { title, .. } => title.clone(),
            Self::Permission { tool_name, .. } => format!("Allow {tool_name}?"),
        }
    }

    pub fn options(&self) -> Vec<&str> {
        match self {
            Self::Ask { options, .. } => options.iter().map(String::as_str).collect(),
            Self::Permission { .. } => vec![PERMISSION_ALLOW, PERMISSION_DENY],
        }
    }

    pub fn links(&self) -> &[String] {
        match self {
            Self::Ask { links, .. } => links,
            Self::Permission { .. } => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientEvent {
    Answered {
        id: QuestionId,
        answer: String,
    },
    Withdrawn {
        id: QuestionId,
        reason: WithdrawReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WithdrawReason {
    Deleted,
    AnsweredInPane,
    PaneClosed,
}
