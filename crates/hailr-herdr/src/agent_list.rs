use std::path::Path;

use hailr_core::mux::{AgentStatus, PaneInfo};
use hailr_core::protocol::PaneId;
use serde::Deserialize;

#[derive(Deserialize)]
struct Response {
    result: ResultBody,
}

#[derive(Deserialize)]
struct ResultBody {
    agents: Vec<Agent>,
}

#[derive(Deserialize)]
struct Agent {
    pane_id: String,
    agent_status: String,
    cwd: Option<String>,
}

pub(crate) fn parse(json: &[u8]) -> Result<Vec<PaneInfo>, serde_json::Error> {
    let response: Response = serde_json::from_slice(json)?;
    Ok(response
        .result
        .agents
        .into_iter()
        .map(|agent| PaneInfo {
            label: label(agent.cwd.as_deref(), &agent.pane_id),
            status: AgentStatus::from(agent.agent_status.as_str()),
            id: PaneId(agent.pane_id),
        })
        .collect())
}

fn label(cwd: Option<&str>, pane_id: &str) -> String {
    match cwd
        .and_then(|cwd| Path::new(cwd).file_name())
        .and_then(|name| name.to_str())
    {
        Some(dir) => format!("{dir} / {pane_id}"),
        None => pane_id.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_agent_list() {
        let panes = parse(include_bytes!("../tests/fixtures/agent-list.json")).unwrap();
        assert_eq!(
            panes,
            vec![
                PaneInfo {
                    id: PaneId("w1B:p1".into()),
                    status: AgentStatus::Idle,
                    label: "hailr / w1B:p1".into(),
                },
                PaneInfo {
                    id: PaneId("w1B:p2".into()),
                    status: AgentStatus::Working,
                    label: "api / w1B:p2".into(),
                },
                PaneInfo {
                    id: PaneId("w2A:p1".into()),
                    status: AgentStatus::Other("sleeping".into()),
                    label: "w2A:p1".into(),
                },
            ]
        );
    }

    #[test]
    fn rejects_unexpected_shape() {
        assert!(parse(br#"{"agents":[]}"#).is_err());
    }
}
