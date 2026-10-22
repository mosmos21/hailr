use std::sync::Arc;
use std::time::Duration;

use hailr_core::mux::Mux;

use super::Broker;

pub const INTERVAL: Duration = Duration::from_secs(2);

/// pane の一覧を周期的に取り、attach して pane で答えた質問を取り下げる
pub async fn run(broker: Arc<Broker>, mux: Arc<dyn Mux>) {
    let mut ticks = tokio::time::interval(INTERVAL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_error = None;
    loop {
        ticks.tick().await;
        // herdr が止まっていると毎周期同じ失敗になるため、変わったときと戻ったときだけ記録する
        match poll_once(&broker, &mux).await {
            Ok(()) if last_error.take().is_some() => tracing::info!("listing panes works again"),
            Ok(()) => {}
            Err(e) if last_error.as_ref() != Some(&e) => {
                tracing::warn!("cannot list panes: {e}");
                last_error = Some(e);
            }
            Err(_) => {}
        }
    }
}

/// 取れなかった回は何も変えず、次の周期で取り直す
async fn poll_once(broker: &Broker, mux: &Arc<dyn Mux>) -> Result<(), String> {
    let mux = Arc::clone(mux);
    let start = broker.listing_start();
    match tokio::task::spawn_blocking(move || mux.list_panes()).await {
        Ok(Ok(panes)) => {
            broker.reconcile_panes(start, &panes);
            Ok(())
        }
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(format!("listing panes panicked: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::sync::Mutex;

    use hailr_core::mux::{AgentStatus, MuxError, PaneInfo};
    use hailr_core::protocol::{ClientId, PaneId, Question, QuestionBody, QuestionId};
    use uuid::Uuid;

    use super::*;

    /// list_panes が返す値を差し替えられる偽の Mux
    struct FakeMux(Mutex<Result<Vec<PaneInfo>, ()>>);

    impl Mux for FakeMux {
        fn list_panes(&self) -> Result<Vec<PaneInfo>, MuxError> {
            self.0
                .lock()
                .unwrap()
                .clone()
                .map_err(|()| MuxError::InvalidPane)
        }
        fn read_pane(&self, _: &PaneId) -> Result<String, MuxError> {
            unimplemented!()
        }
        fn attach_command(&self, _: &PaneId) -> Command {
            unimplemented!()
        }
        fn notify(&self, _: &str, _: &str) -> Result<(), MuxError> {
            unimplemented!()
        }
    }

    fn question(client: ClientId, pane: &str) -> Question {
        Question {
            id: QuestionId(Uuid::new_v4()),
            client_id: client,
            pane_id: PaneId(pane.into()),
            asked_at: 1,
            body: QuestionBody::Ask {
                title: "t".into(),
                question: "q".into(),
                options: vec![],
                links: vec![],
            },
        }
    }

    fn pane(status: AgentStatus) -> PaneInfo {
        PaneInfo {
            id: PaneId("w1:p1".into()),
            status,
            label: "w1:p1".into(),
        }
    }

    #[tokio::test]
    async fn reconciles_listed_panes_and_changes_nothing_on_errors() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (_rx, _guard) = broker.connect(client);
        let q = question(client, "w1:p1");
        broker.register(q.clone()).unwrap();
        broker.mark_attached(&q.id);

        let fake = Arc::new(FakeMux(Mutex::new(Ok(vec![pane(AgentStatus::Idle)]))));
        let mux: Arc<dyn Mux> = fake.clone();
        assert_eq!(poll_once(&broker, &mux).await, Ok(()));

        *fake.0.lock().unwrap() = Err(());
        assert!(poll_once(&broker, &mux).await.is_err());
        assert_eq!(
            broker.snapshot(),
            vec![q],
            "a failed listing must not withdraw"
        );

        *fake.0.lock().unwrap() = Ok(vec![pane(AgentStatus::Working)]);
        assert_eq!(poll_once(&broker, &mux).await, Ok(()));
        assert!(broker.snapshot().is_empty());
    }
}
