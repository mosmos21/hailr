mod app;
mod input;
mod preview;
mod view;

use std::future::Future;
use std::io;
use std::sync::Arc;

use crossterm::event::EventStream;
use futures_util::StreamExt;
use hailr_core::mux::Mux;
use hailr_core::protocol::WithdrawReason;
use ratatui::DefaultTerminal;
use tokio::sync::{Notify, mpsc, watch};

use crate::broker::Broker;
use crate::config::Side;
use app::App;
use input::Action;

/// 端末を代替画面に切り替えて質問を出す。q / Ctrl-C、`stop`、入力が閉じたときに戻して終わる
pub async fn run(
    broker: Arc<Broker>,
    mux: Arc<dyn Mux>,
    side: Side,
    stop: impl Future<Output = ()>,
) -> io::Result<()> {
    let mut terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        Err(e) => {
            ratatui::restore();
            return Err(e);
        }
    };
    // ratatui の hook はどのタスクの panic でも端末を戻すため、戻った端末に描き続けないよう TUI も閉じる
    let panicked = Arc::new(Notify::new());
    let on_panic = Arc::clone(&panicked);
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        on_panic.notify_one();
        previous_hook(info);
    }));
    let result = run_loop(&mut terminal, &broker, mux, side, stop, &panicked).await;
    ratatui::restore();
    result
}

async fn run_loop(
    terminal: &mut DefaultTerminal,
    broker: &Broker,
    mux: Arc<dyn Mux>,
    side: Side,
    stop: impl Future<Output = ()>,
    panicked: &Notify,
) -> io::Result<()> {
    let mut changes = broker.changes();
    let mut app = App::default();
    app.set_questions(broker.snapshot());
    let (pane_tx, pane_rx) = watch::channel(app.selected_pane());
    let (shot_tx, mut shots) = mpsc::channel(1);
    let previewing = tokio::spawn(preview::run(mux, pane_rx, shot_tx));
    let _stop_previewing = AbortOnDrop(previewing);
    let mut events = EventStream::new();
    tokio::pin!(stop);

    loop {
        terminal.draw(|frame| view::draw(frame, &app, side))?;
        // panic で端末が戻ったあとに 1 フレームも描かないよう、panic を先に見る
        tokio::select! {
            biased;
            () = panicked.notified() => return Err(io::Error::other("a task panicked")),
            () = &mut stop => return Ok(()),
            event = events.next() => match event {
                None => return Ok(()),
                Some(Err(e)) => return Err(e),
                Some(Ok(event)) => match input::action(&event) {
                    Some(Action::Quit) => return Ok(()),
                    Some(action) => apply(&mut app, broker, action),
                    None if input::is_press(&event) => app.clear_notice(),
                    None => {}
                },
            },
            Ok(()) = changes.changed() => app.set_questions(broker.snapshot()),
            Some(shot) = shots.recv() => app.set_shot(shot),
        }
        let repointed = pane_tx.send_if_modified(|pane| {
            let selected = app.selected_pane();
            let changed = *pane != selected;
            *pane = selected;
            changed
        });
        if repointed {
            app.forget_shot();
        }
    }
}

/// 押した時点の選択に対して行う。知らせは次のキーで消す
fn apply(app: &mut App, broker: &Broker, action: Action) {
    app.clear_notice();
    let acts = matches!(action, Action::Answer(_) | Action::Delete);
    if acts && app.selected().is_none() {
        return;
    }
    // 答えた直後の押し直しやキーリピートが、見ていない次の質問（権限確認を含む）に届かないようにする
    if acts && app.moved_on_its_own() {
        app.set_notice("the selection moved; press ↑ or ↓ to pick a question first");
        return;
    }
    match action {
        Action::Previous => app.select_previous(),
        Action::Next => app.select_next(),
        Action::Answer(button) => {
            let Some(question) = app.selected() else {
                return;
            };
            if button >= app.shown_buttons() {
                return;
            }
            let Some(answer) = question.body.options().get(button).map(|o| o.to_string()) else {
                return;
            };
            let result = broker.answer(&question.id, answer);
            // 失敗しても broker は質問を消していることがある。押し直しが消えた質問に向かわないよう、すぐ取り直す
            app.set_questions(broker.snapshot());
            if let Err(e) = result {
                app.set_notice(format!("not answered: {e}"));
            }
        }
        Action::Delete => {
            if let Some(question) = app.selected() {
                broker.withdraw(&question.id, WithdrawReason::Deleted);
                app.set_questions(broker.snapshot());
            }
        }
        Action::Quit => {}
    }
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use hailr_core::protocol::{ClientEvent, ClientId, PaneId, Question, QuestionBody, QuestionId};
    use uuid::Uuid;

    use super::*;

    fn ask(client: ClientId, options: &[&str]) -> Question {
        Question {
            id: QuestionId(Uuid::new_v4()),
            client_id: client,
            pane_id: PaneId("w1:p1".into()),
            asked_at: 1,
            body: QuestionBody::Ask {
                title: "t".into(),
                question: "q".into(),
                options: options.iter().map(|o| o.to_string()).collect(),
                links: vec![],
            },
        }
    }

    /// 質問を並べて登録し、2 件目を選んだ状態にする
    fn second_selected(broker: &Broker, questions: &[Question]) -> App {
        for q in questions {
            broker.register(q.clone()).unwrap();
        }
        let mut app = App::default();
        app.set_questions(broker.snapshot());
        app.set_shown_buttons(9);
        apply(&mut app, broker, Action::Next);
        app
    }

    #[test]
    fn answers_the_selected_question_with_the_original_option() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (mut events, _guard) = broker.connect(client);
        let first = ask(client, &["Yes"]);
        let mut second = ask(client, &["Yes", "No\u{1b}[2J"]);
        second.asked_at = 2;
        let mut app = second_selected(&broker, &[first.clone(), second.clone()]);

        apply(&mut app, &broker, Action::Answer(2));
        assert!(events.try_recv().is_err(), "no third button");
        app.set_shown_buttons(1);
        apply(&mut app, &broker, Action::Answer(1));
        assert!(events.try_recv().is_err(), "the second button is hidden");
        app.set_shown_buttons(2);
        apply(&mut app, &broker, Action::Answer(1));
        let answered = ClientEvent::Answered {
            id: second.id,
            answer: "No\u{1b}[2J".into(),
        };
        assert_eq!(events.try_recv().unwrap(), answered);
        assert_eq!(broker.snapshot(), vec![first]);
        assert!(app.notice().is_none());
    }

    #[test]
    fn deletes_the_selected_question() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (mut events, _guard) = broker.connect(client);
        let first = ask(client, &[]);
        let mut second = ask(client, &[]);
        second.asked_at = 2;
        let mut app = second_selected(&broker, &[first.clone(), second.clone()]);

        apply(&mut app, &broker, Action::Delete);
        let withdrawn = ClientEvent::Withdrawn {
            id: second.id,
            reason: WithdrawReason::Deleted,
        };
        assert_eq!(events.try_recv().unwrap(), withdrawn);
        assert_eq!(broker.snapshot(), vec![first]);
    }

    #[test]
    fn waits_for_a_choice_after_the_selection_moved_on_its_own() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (mut events, _guard) = broker.connect(client);
        let first = ask(client, &["Yes"]);
        let mut next = ask(client, &["Allow"]);
        next.asked_at = 2;
        for q in [&first, &next] {
            broker.register(q.clone()).unwrap();
        }
        let mut app = App::default();
        app.set_questions(broker.snapshot());
        app.set_shown_buttons(9);

        apply(&mut app, &broker, Action::Answer(0));
        assert!(events.try_recv().is_ok());
        for action in [Action::Answer(0), Action::Answer(0), Action::Delete] {
            apply(&mut app, &broker, action);
            assert!(
                events.try_recv().is_err(),
                "nothing sent to the next question"
            );
            assert!(app.notice().unwrap().contains("pick a question first"));
        }
        apply(&mut app, &broker, Action::Previous);
        apply(&mut app, &broker, Action::Answer(0));
        assert!(events.try_recv().is_ok());
    }

    #[test]
    fn tells_when_the_answer_could_not_be_delivered_until_the_next_key() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (events, _guard) = broker.connect(client);
        drop(events);
        broker.register(ask(client, &["Yes"])).unwrap();
        let mut app = App::default();
        app.set_questions(broker.snapshot());
        app.set_shown_buttons(9);

        apply(&mut app, &broker, Action::Answer(0));
        assert!(app.notice().unwrap().contains("ask again"));
        apply(&mut app, &broker, Action::Next);
        assert!(app.notice().is_none());
    }
}
