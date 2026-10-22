use std::sync::Arc;

use hailr_core::mux::Mux;
use hailr_core::protocol::{QuestionBody, QuestionId};
use tokio::sync::broadcast::{self, error::RecvError};

use super::Broker;
use crate::text::is_unprintable;

const MAX_TITLE: usize = 100;
const MAX_BODY: usize = 300;

/// 初めて届いた質問を 1 件ずつ通知する。まとめて届いても通知のプロセスを並べて起こさない
pub async fn run(
    broker: Arc<Broker>,
    mut arrivals: broadcast::Receiver<QuestionId>,
    mux: Arc<dyn Mux>,
) {
    loop {
        let id = match arrivals.recv().await {
            Ok(id) => id,
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return,
        };
        // 前の通知を待つ間に答えや書き換えが入るため、出す直前の写しで決める
        let Some(question) = broker.to_announce(&id) else {
            continue;
        };
        let (title, body) = notice(&question.body);
        let mux = Arc::clone(&mux);
        // 通知は届かなくても質問は一覧に出ており、stderr は TUI が使うため、失敗は出さずに捨てる
        let _ = tokio::task::spawn_blocking(move || mux.notify(&title, &body)).await;
    }
}

fn notice(body: &QuestionBody) -> (String, String) {
    let text = match body {
        QuestionBody::Ask { question, .. } => question,
        QuestionBody::Permission { description, .. } => description,
    };
    (plain(&body.title(), MAX_TITLE), plain(text, MAX_BODY))
}

// 質問の文言は Claude が書き、mux が端末に描くこともあるため、制御文字と向きを変える文字を空白にして長さを切る
fn plain(text: &str, max: usize) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if is_unprintable(c) { ' ' } else { c })
        .take(max)
        .collect();
    if text.chars().nth(max).is_some() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use hailr_core::mux::{MuxError, PaneInfo};
    use hailr_core::protocol::{ClientId, PaneId, Question, WithdrawReason};
    use tokio::sync::mpsc;
    use uuid::Uuid;

    use super::*;

    /// notify に渡された値を送り出して失敗を返す。同時に走った数の最大も数える
    struct FakeMux {
        sent: mpsc::UnboundedSender<(String, String)>,
        running: AtomicUsize,
        most: AtomicUsize,
    }

    impl Mux for FakeMux {
        fn list_panes(&self) -> Result<Vec<PaneInfo>, MuxError> {
            unimplemented!()
        }
        fn read_pane(&self, _: &PaneId) -> Result<String, MuxError> {
            unimplemented!()
        }
        fn attach_command(&self, _: &PaneId) -> Command {
            unimplemented!()
        }
        fn notify(&self, title: &str, body: &str) -> Result<(), MuxError> {
            let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(5));
            self.running.fetch_sub(1, Ordering::SeqCst);
            self.sent.send((title.into(), body.into())).unwrap();
            Err(MuxError::InvalidPane)
        }
    }

    struct Setup {
        broker: Arc<Broker>,
        client: ClientId,
        mux: Arc<FakeMux>,
        sent: mpsc::UnboundedReceiver<(String, String)>,
        _connection: (
            mpsc::Receiver<hailr_core::protocol::ClientEvent>,
            super::super::ClientGuard,
        ),
    }

    /// 通知を始める前に登録や回答を済ませられるよう、notifier はまだ動かさない
    fn setup() -> Setup {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let connection = broker.connect(client);
        let (tx, sent) = mpsc::unbounded_channel();
        let mux = Arc::new(FakeMux {
            sent: tx,
            running: AtomicUsize::new(0),
            most: AtomicUsize::new(0),
        });
        Setup {
            broker,
            client,
            mux,
            sent,
            _connection: connection,
        }
    }

    fn start(s: &Setup, arrivals: broadcast::Receiver<QuestionId>) -> tokio::task::JoinHandle<()> {
        let mux: Arc<dyn Mux> = s.mux.clone();
        tokio::spawn(run(Arc::clone(&s.broker), arrivals, mux))
    }

    fn ask(client: ClientId, question: &str) -> Question {
        Question {
            id: QuestionId(Uuid::new_v4()),
            client_id: client,
            pane_id: PaneId("w1:p1".into()),
            asked_at: 1,
            body: QuestionBody::Ask {
                title: "Deploy?".into(),
                question: question.into(),
                options: vec![],
                links: vec![],
            },
        }
    }

    async fn next(sent: &mut mpsc::UnboundedReceiver<(String, String)>) -> (String, String) {
        tokio::time::timeout(Duration::from_secs(5), sent.recv())
            .await
            .expect("notified in time")
            .unwrap()
    }

    #[tokio::test]
    async fn notifies_the_current_text_of_pending_questions_only() {
        let mut s = setup();
        let arrivals = s.broker.arrivals();
        let mut updated = ask(s.client, "old text");
        s.broker.register(updated.clone()).unwrap();
        updated.body = ask(s.client, "new text").body;
        s.broker.register(updated).unwrap();
        let settled = ask(s.client, "settled");
        s.broker.register(settled.clone()).unwrap();
        s.broker.withdraw(&settled.id, WithdrawReason::Deleted);
        let permission = Question {
            body: QuestionBody::Permission {
                request_id: "abcde".into(),
                tool_name: "Bash".into(),
                description: "run\n\u{1b}]52;c;x\u{7}\u{202e}tests".into(),
                input_preview: "pnpm test".into(),
            },
            ..ask(s.client, "")
        };
        s.broker.register(permission).unwrap();
        let notifying = start(&s, arrivals);

        assert_eq!(
            next(&mut s.sent).await,
            ("Deploy?".into(), "new text".into())
        );
        assert_eq!(
            next(&mut s.sent).await,
            ("Allow Bash?".into(), "run  ]52;c;x  tests".into())
        );
        notifying.abort();
    }

    #[tokio::test]
    async fn notifies_one_at_a_time_and_skips_what_overflowed() {
        let mut s = setup();
        let arrivals = s.broker.arrivals();
        let texts: Vec<String> = (0..=super::super::ARRIVALS)
            .map(|n| n.to_string())
            .collect();
        for text in &texts {
            s.broker.register(ask(s.client, text)).unwrap();
        }
        let notifying = start(&s, arrivals);

        let mut got = Vec::new();
        for _ in 1..texts.len() {
            got.push(next(&mut s.sent).await.1);
        }
        assert_eq!(got, texts[1..]);
        assert_eq!(s.mux.most.load(Ordering::SeqCst), 1);
        notifying.abort();
    }

    #[test]
    fn cuts_long_text_by_characters() {
        assert_eq!(plain("", 4), "");
        assert_eq!(plain("abcd", 4), "abcd");
        assert_eq!(plain("abcde", 4), "abcd…");
        assert_eq!(plain("あいうえ", 4), "あいうえ");
        assert_eq!(plain("あいうえお", 4), "あいうえ…");
    }

    #[test]
    fn blanks_c1_controls_too() {
        assert_eq!(plain("a\u{009B}2Jb\u{0085}c", 10), "a 2Jb c");
    }

    #[test]
    fn notice_cleans_and_cuts_title_and_body() {
        let body = QuestionBody::Permission {
            request_id: "abcde".into(),
            tool_name: format!("\u{1b}[2J{}", "x".repeat(MAX_TITLE)),
            description: "y".repeat(MAX_BODY + 1),
            input_preview: String::new(),
        };
        let (title, text) = notice(&body);
        assert!(title.starts_with("Allow  [2J"));
        assert_eq!(title.chars().count(), MAX_TITLE + 1);
        assert_eq!(text, format!("{}…", "y".repeat(MAX_BODY)));
    }
}
