mod app;
mod input;
mod preview;
mod view;

use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream};
use futures_util::StreamExt;
use hailr_core::mux::Mux;
use hailr_core::protocol::{PaneId, QuestionId, WithdrawReason};
use ratatui::DefaultTerminal;
use tokio::sync::{Notify, mpsc, watch};

use crate::broker::Broker;
use crate::config::Ui;
use app::{App, Choice, Target};
use input::{Action, Typing};

/// 押したボタンが効いたと見て取れるだけ、答えた質問を残す既定の長さ
pub const ANSWERED_FOR: Duration = Duration::from_millis(1500);

/// 端末を代替画面に切り替えて質問を出す。q / Ctrl-C、`stop`、入力が閉じたときに戻して終わる
pub async fn run(
    broker: Arc<Broker>,
    mux: Arc<dyn Mux>,
    ui: Ui,
    stop: impl Future<Output = ()>,
) -> io::Result<()> {
    let started = ratatui::try_init().and_then(|terminal| {
        crossterm::execute!(
            io::stdout(),
            ReportClicks,
            crossterm::event::EnableBracketedPaste
        )?;
        Ok(terminal)
    });
    let mut terminal = match started {
        Ok(terminal) => terminal,
        Err(e) => {
            release_input();
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
        // ratatui の restore はマウスの報告を止めないため、ここで止める
        release_input();
        previous_hook(info);
    }));
    let result = run_loop(&mut terminal, &broker, mux, ui, stop, &panicked).await;
    release_input();
    ratatui::restore();
    result
}

/// crossterm の EnableMouseCapture はポインタの移動も報告させ、動かすたびに描き直すことになるため、押下と離しだけを SGR 形式で頼む
struct ReportClicks;

impl crossterm::Command for ReportClicks {
    fn write_ansi(&self, f: &mut impl std::fmt::Write) -> std::fmt::Result {
        f.write_str("\x1b[?1000h\x1b[?1006h")
    }
}

/// 端末が閉じていれば書けないので失敗は無視する
fn release_input() {
    let _ = crossterm::execute!(
        io::stdout(),
        crossterm::event::DisableMouseCapture,
        crossterm::event::DisableBracketedPaste
    );
}

async fn run_loop(
    terminal: &mut DefaultTerminal,
    broker: &Broker,
    mux: Arc<dyn Mux>,
    ui: Ui,
    stop: impl Future<Output = ()>,
    panicked: &Notify,
) -> io::Result<()> {
    let mut changes = broker.changes();
    let mut app = App::default();
    app.set_answered_for(ui.answered_delay);
    app.set_questions(broker.snapshot());
    let (pane_tx, pane_rx) = watch::channel(app.selected_pane());
    let (shot_tx, mut shots) = mpsc::channel(1);
    let (focus_tx, mut focused) = mpsc::unbounded_channel();
    let previewing = tokio::spawn(preview::run(Arc::clone(&mux), pane_rx, shot_tx));
    let _stop_previewing = AbortOnDrop(previewing);
    let mut events = EventStream::new();
    tokio::pin!(stop);

    loop {
        terminal.draw(|frame| view::draw(frame, &app, ui.questions))?;
        let answered_until = app.answered_until();
        // panic で端末が戻ったあとに 1 フレームも描かないよう、panic を先に見る
        tokio::select! {
            biased;
            () = panicked.notified() => return Err(io::Error::other("a task panicked")),
            () = &mut stop => return Ok(()),
            event = events.next() => match event {
                None => return Ok(()),
                Some(Err(e)) => return Err(e),
                Some(Ok(event)) => match on_event(&mut app, broker, &event) {
                    Step::Quit => return Ok(()),
                    Step::Do(effect) => match effect {
                        Effect::Open(link) => {
                            if let Err(e) = checked_url(&link).and_then(|url| open_link(&url)) {
                                app.set_notice(format!("not opened: {e}"));
                            }
                        }
                        Effect::Focus(id, pane) => {
                            // herdr を待つ間も broker と描画を止めないよう、別のスレッドで呼んで結果だけ受け取る
                            let (mux, tx) = (Arc::clone(&mux), focus_tx.clone());
                            tokio::task::spawn_blocking(move || {
                                let result = mux
                                    .focus(&pane)
                                    .map(|()| id)
                                    .map_err(|e| format!("not focused on {}: {e}", pane.0));
                                let _ = tx.send(result);
                            });
                        }
                    },
                    Step::Nothing => {}
                },
            },
            Ok(()) = changes.changed() => app.set_questions(broker.snapshot()),
            () = tokio::time::sleep_until(answered_until.unwrap_or_else(Instant::now).into()), if answered_until.is_some() => {
                app.expire_answered(Instant::now(), broker.snapshot());
            }
            Some(shot) = shots.recv() => app.set_shot(shot),
            Some(result) = focused.recv() => after_focus(&mut app, broker, result),
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

/// TUI の外で行うこと
#[derive(Debug, PartialEq, Eq)]
enum Effect {
    Focus(QuestionId, PaneId),
    Open(String),
}

/// 押した時点の選択に対して行う。知らせは次のキーで消す
fn apply(app: &mut App, broker: &Broker, action: Action) -> Option<Effect> {
    app.clear_notice();
    // `o` の後の数字を回答と取り違えないよう、待ちの間は数字をリンクの番号にだけ使い、数字と `o` 以外のキーで待ちを終える
    if app.opening() {
        match action {
            Action::Answer(link) if link < app.shown_links() => {
                let url = app.selected()?.body.links().get(link)?.clone();
                return app.mark_opened(link).then_some(Effect::Open(url));
            }
            Action::Answer(_) | Action::Open => return None,
            // クリックは押した先がはっきりしているので、待ちを終えてそのまま行う
            Action::Click(_) => app.end_opening(),
            _ => {
                app.end_opening();
                return None;
            }
        }
    }
    let acts = matches!(
        action,
        Action::Answer(_)
            | Action::Delete
            | Action::Focus
            | Action::Open
            | Action::Reply
            | Action::Click(Target::Button(_) | Target::Link(_))
    );
    if acts && app.selected().is_none() {
        return None;
    }
    // 答えた直後の押し直しやキーリピートが、見ていない次の質問（権限確認を含む）に届かないようにする
    if acts && app.moved_on_its_own() {
        app.set_notice("the selection moved; press ↑ or ↓ to pick a question first");
        return None;
    }
    let settling = matches!(
        action,
        Action::Answer(_) | Action::Delete | Action::Reply | Action::Click(Target::Button(_))
    );
    if settling && app.selected_is_answered() {
        return None;
    }
    match action {
        Action::Previous => app.select_previous(),
        Action::Next => app.select_next(),
        Action::Click(Target::Question(id)) => app.select(id),
        Action::Click(Target::Button(button)) => return apply(app, broker, Action::Answer(button)),
        Action::Click(Target::Link(link)) if link < app.shown_links() => {
            return Some(Effect::Open(
                app.selected()?.body.links().get(link)?.clone(),
            ));
        }
        Action::Click(Target::Link(_)) => {}
        Action::Answer(button) => {
            let question = app.selected()?;
            if button >= app.shown_buttons() {
                return None;
            }
            let answer = question.body.options().get(button)?.to_string();
            let question = question.clone();
            let result = broker.answer(&question.id, answer);
            if result.is_ok() && !app.answered_for().is_zero() {
                app.set_answered(
                    question,
                    Choice::Button(button),
                    Instant::now() + app.answered_for(),
                );
            }
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
        Action::Focus => {
            return app
                .selected()
                .map(|q| Effect::Focus(q.id, q.pane_id.clone()));
        }
        Action::Reply => app.start_reply(),
        Action::Open => {
            if app.shown_links() > 0 {
                app.start_opening();
            }
        }
        Action::Quit => {}
    }
    None
}

#[derive(Debug, PartialEq, Eq)]
enum Step {
    Quit,
    Do(Effect),
    Nothing,
}

/// 返事を打っている間はキーを全部文字として扱い、それ以外のときだけコマンドとして読む
fn on_event(app: &mut App, broker: &Broker, event: &Event) -> Step {
    if app.reply().is_some() {
        return match input::typing(event) {
            Some(Typing::Quit) => Step::Quit,
            Some(typing) => {
                app.clear_notice();
                type_into(app, broker, typing);
                Step::Nothing
            }
            None => Step::Nothing,
        };
    }
    match input::action(event).or_else(|| input::click(event, app)) {
        Some(Action::Quit) => Step::Quit,
        Some(action) => apply(app, broker, action).map_or(Step::Nothing, Step::Do),
        None if input::is_press(event) => {
            app.clear_notice();
            app.end_opening();
            Step::Nothing
        }
        None => Step::Nothing,
    }
}

fn type_into(app: &mut App, broker: &Broker, typing: Typing) {
    match typing {
        Typing::Text(text) => app.type_reply(&text),
        Typing::Erase => app.erase_reply(),
        Typing::Cancel => {
            app.take_reply();
        }
        Typing::Send => send_reply(app, broker),
        Typing::Quit => {}
    }
}

/// 返事先が外れたあとの Enter は、打った文を黙って捨てないよう何もしない
fn send_reply(app: &mut App, broker: &Broker) {
    if app.reply_gone() {
        return;
    }
    let Some((id, text)) = app.reply().map(|(id, text)| (id, text.trim().to_string())) else {
        return;
    };
    if text.is_empty() {
        return;
    }
    app.take_reply();
    let question = app.questions().iter().find(|q| q.id == id).cloned();
    let result = broker.reply(&id, text.clone());
    if let (Ok(()), Some(question)) = (&result, question)
        && !app.answered_for().is_zero()
    {
        let until = Instant::now() + app.answered_for();
        app.set_answered(question, Choice::Reply(text), until);
    }
    app.set_questions(broker.snapshot());
    if let Err(e) = result {
        app.set_notice(format!("not sent: {e}"));
    }
}

/// pane で答えたら取り下げられるよう、移せた質問にだけ印を付ける。遅れて届いた失敗で、あとから出た知らせを消さない
fn after_focus(app: &mut App, broker: &Broker, result: Result<QuestionId, String>) {
    match result {
        Ok(id) => broker.mark_focused(&id),
        Err(e) if app.notice().is_none() => app.set_notice(e),
        Err(_) => {}
    }
}

/// http / https 以外の scheme は OS の関連付けでアプリを起こし、userinfo は表示のホストを偽らせるため断る
fn checked_url(link: &str) -> io::Result<url::Url> {
    let invalid = |reason: &str| io::Error::new(io::ErrorKind::InvalidInput, reason.to_owned());
    let url = url::Url::parse(link).map_err(|e| invalid(&e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(invalid("only http and https links open"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid("links with a user name do not open"));
    }
    Ok(url)
}

/// tokio の runtime の中で呼ぶ
fn open_link(url: &url::Url) -> io::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    // 待たずに手放す。終わった子は tokio が回収する
    let spawned = tokio::process::Command::new(opener)
        .arg(url.as_str())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(e) = &spawned {
        tracing::warn!("cannot start {opener}: {e}");
    }
    spawned.map(drop)
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use hailr_core::mux::{AgentStatus, PaneInfo};
    use hailr_core::protocol::{ClientEvent, ClientId, Question, QuestionBody};
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

    /// 答えた質問を残す時間が過ぎたあとの一覧にする
    fn wait_out(app: &mut App, broker: &Broker) {
        app.expire_answered(Instant::now() + ANSWERED_FOR, broker.snapshot());
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
    fn keeps_an_answered_question_until_its_time_is_up() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (mut events, _guard) = broker.connect(client);
        let first = ask(client, &["Yes", "No"]);
        let mut second = ask(client, &["Yes"]);
        second.asked_at = 2;
        let mut app = second_selected(&broker, &[first.clone(), second.clone()]);
        apply(&mut app, &broker, Action::Previous);

        apply(&mut app, &broker, Action::Answer(1));
        let answered = ClientEvent::Answered {
            id: first.id,
            answer: "No".into(),
        };
        assert_eq!(events.try_recv().ok(), Some(answered));
        assert_eq!(broker.snapshot(), vec![second.clone()]);
        assert_eq!(app.selected().map(|q| q.id), Some(first.id));
        assert_eq!(app.answered_with(first.id), Some(&Choice::Button(1)));
        assert!(!app.moved_on_its_own());

        for action in [
            Action::Delete,
            Action::Click(Target::Button(0)),
            Action::Answer(0),
        ] {
            apply(&mut app, &broker, action);
        }
        assert!(events.try_recv().is_err(), "pressed again");
        assert_eq!(
            app.notice(),
            None,
            "no warning for a press that does nothing"
        );
        assert_eq!(broker.snapshot(), vec![second.clone()]);
        app.expire_answered(Instant::now(), broker.snapshot());
        assert_eq!(
            app.selected().map(|q| q.id),
            Some(first.id),
            "gone too early"
        );

        wait_out(&mut app, &broker);
        assert_eq!(app.selected().map(|q| q.id), Some(second.id));
        assert!(app.moved_on_its_own());
    }

    #[test]
    fn treats_a_question_asked_again_as_new() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (mut events, _guard) = broker.connect(client);
        let question = ask(client, &["Yes"]);
        broker.register(question.clone()).unwrap();
        let mut app = App::default();
        app.set_questions(broker.snapshot());
        app.set_shown_buttons(9);

        apply(&mut app, &broker, Action::Answer(0));
        assert!(events.try_recv().is_ok());
        broker.register(question.clone()).unwrap();
        app.set_questions(broker.snapshot());
        assert_eq!(app.answered_with(question.id), None);
        assert!(
            app.moved_on_its_own(),
            "a held key must not answer it again"
        );
        apply(&mut app, &broker, Action::Answer(0));
        assert!(events.try_recv().is_err());

        apply(&mut app, &broker, Action::Previous);
        apply(&mut app, &broker, Action::Answer(0));
        assert!(events.try_recv().is_ok());
    }

    #[test]
    fn leaves_another_choice_alone_when_an_answered_question_comes_back() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (mut events, _guard) = broker.connect(client);
        let first = ask(client, &["Yes"]);
        let mut second = ask(client, &["Yes"]);
        second.asked_at = 2;
        let mut app = second_selected(&broker, &[first.clone(), second.clone()]);
        apply(&mut app, &broker, Action::Previous);

        apply(&mut app, &broker, Action::Answer(0));
        assert!(events.try_recv().is_ok());
        apply(&mut app, &broker, Action::Next);
        broker.register(first.clone()).unwrap();
        app.set_questions(broker.snapshot());
        assert!(!app.moved_on_its_own());
        assert_eq!(app.selected().map(|q| q.id), Some(second.id));
        apply(&mut app, &broker, Action::Answer(0));
        let answered = ClientEvent::Answered {
            id: second.id,
            answer: "Yes".into(),
        };
        assert_eq!(events.try_recv().ok(), Some(answered));
    }

    fn press(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn char(c: char) -> Event {
        press(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn enter() -> Event {
        press(KeyCode::Enter, KeyModifiers::NONE)
    }

    #[test]
    fn sends_a_typed_reply_without_acting_on_its_keys() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (mut events, _guard) = broker.connect(client);
        let question = ask(client, &["Yes"]);
        broker.register(question.clone()).unwrap();
        let mut app = App::default();
        app.set_questions(broker.snapshot());
        app.set_shown_buttons(9);

        assert_eq!(on_event(&mut app, &broker, &char('r')), Step::Nothing);
        on_event(&mut app, &broker, &Event::Paste("  ".into()));
        on_event(&mut app, &broker, &enter());
        assert_eq!(
            app.reply().map(|r| r.1),
            Some("  "),
            "Enter on a blank reply keeps it open"
        );
        assert_eq!(app.notice(), None);
        assert!(events.try_recv().is_err());
        app.take_reply();
        on_event(&mut app, &broker, &char('r'));
        let keys = [
            char('1'),
            char('q'),
            char('d'),
            char('f'),
            press(KeyCode::Char('d'), KeyModifiers::CONTROL),
            press(KeyCode::Enter, KeyModifiers::ALT),
            Event::Paste(" ok\n".into()),
        ];
        for key in &keys {
            assert_eq!(on_event(&mut app, &broker, key), Step::Nothing);
        }
        assert!(events.try_recv().is_err(), "keys typed are not actions");
        assert_eq!(broker.snapshot().len(), 1);
        on_event(&mut app, &broker, &enter());
        let answered = ClientEvent::Answered {
            id: question.id,
            answer: "1qdf ok".into(),
        };
        assert_eq!(events.try_recv().ok(), Some(answered));
        assert_eq!(app.reply(), None);
        assert_eq!(
            app.answered_with(question.id),
            Some(&Choice::Reply("1qdf ok".into()))
        );
        assert_eq!(on_event(&mut app, &broker, &enter()), Step::Nothing);
        on_event(&mut app, &broker, &char('r'));
        assert_eq!(app.reply(), None, "no second reply while it is kept");

        on_event(&mut app, &broker, &char('k'));
        wait_out(&mut app, &broker);
        let quit = press(KeyCode::Char('c'), KeyModifiers::CONTROL);
        broker.register(ask(client, &["Yes"])).unwrap();
        app.set_questions(broker.snapshot());
        on_event(&mut app, &broker, &char('k'));
        on_event(&mut app, &broker, &char('r'));
        assert!(app.reply().is_some());
        assert_eq!(on_event(&mut app, &broker, &quit), Step::Quit);
    }

    #[test]
    fn keeps_the_keys_out_of_commands_after_the_question_goes_away() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (mut events, _guard) = broker.connect(client);
        let first = ask(client, &["Yes"]);
        let mut second = ask(client, &["Yes"]);
        second.asked_at = 2;
        let mut app = second_selected(&broker, &[first.clone(), second.clone()]);
        apply(&mut app, &broker, Action::Previous);

        on_event(&mut app, &broker, &char('r'));
        let ignored = [
            press(KeyCode::Down, KeyModifiers::NONE),
            press(KeyCode::Char('q'), KeyModifiers::ALT),
        ];
        app.set_notice("old");
        for key in &ignored {
            assert_eq!(on_event(&mut app, &broker, key), Step::Nothing);
        }
        assert_eq!(
            app.selected().map(|q| q.id),
            Some(first.id),
            "↓ while typing"
        );
        on_event(&mut app, &broker, &char('x'));
        assert_eq!(app.notice(), None, "a typed key clears the notice");
        broker.withdraw(&first.id, WithdrawReason::Deleted);
        assert!(events.try_recv().is_ok());
        app.set_questions(broker.snapshot());
        assert!(app.reply_gone());
        for key in [char('j'), char('d'), char('1'), char('q'), char('f')]
            .iter()
            .chain(&ignored)
        {
            assert_eq!(on_event(&mut app, &broker, key), Step::Nothing);
        }
        assert_eq!(app.selected().map(|q| q.id), Some(second.id));
        assert!(events.try_recv().is_err(), "nothing answered");
        assert_eq!(broker.snapshot(), vec![second]);
        broker.register(first.clone()).unwrap();
        app.set_questions(broker.snapshot());
        assert!(
            app.reply_gone(),
            "asked again, but another question is selected"
        );
        on_event(&mut app, &broker, &enter());
        assert_eq!(app.reply().map(|r| r.1), Some("x"), "Enter keeps the text");
        assert!(events.try_recv().is_err(), "nothing sent");
        on_event(&mut app, &broker, &press(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.reply(), None, "Esc closes the box");
        assert_eq!(app.notice(), None);
    }

    #[test]
    fn cancels_a_reply_and_tells_when_it_cannot_be_sent() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (events, _guard) = broker.connect(client);
        broker.register(ask(client, &["Yes"])).unwrap();
        let mut app = App::default();
        app.set_questions(broker.snapshot());

        apply(&mut app, &broker, Action::Reply);
        type_into(&mut app, &broker, Typing::Text("draft".into()));
        type_into(&mut app, &broker, Typing::Cancel);
        assert_eq!(app.reply(), None);
        assert_eq!(broker.snapshot().len(), 1, "nothing sent");

        drop(events);
        apply(&mut app, &broker, Action::Reply);
        type_into(&mut app, &broker, Typing::Text("late".into()));
        type_into(&mut app, &broker, Typing::Send);
        assert!(app.notice().unwrap().starts_with("not sent"));
        assert_eq!(app.reply(), None);
        assert!(app.selected().is_none(), "not kept as answered");
    }

    #[test]
    fn keeps_nothing_when_the_delay_is_zero() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (_events, _guard) = broker.connect(client);
        broker.register(ask(client, &["Yes"])).unwrap();
        let mut app = App::default();
        app.set_answered_for(Duration::ZERO);
        app.set_questions(broker.snapshot());
        app.set_shown_buttons(9);
        apply(&mut app, &broker, Action::Answer(0));
        assert!(app.selected().is_none());

        broker.register(ask(client, &["Yes"])).unwrap();
        app.set_questions(broker.snapshot());
        apply(&mut app, &broker, Action::Previous);
        on_event(&mut app, &broker, &char('r'));
        on_event(&mut app, &broker, &char('y'));
        on_event(&mut app, &broker, &enter());
        assert!(app.selected().is_none());
    }

    #[test]
    fn lets_another_question_be_answered_while_one_is_kept() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (mut events, _guard) = broker.connect(client);
        let mut questions: Vec<_> = (1..=3).map(|_| ask(client, &["Yes"])).collect();
        for (at, q) in (1..).zip(questions.iter_mut()) {
            q.asked_at = at;
        }
        let mut app = second_selected(&broker, &questions);

        apply(&mut app, &broker, Action::Answer(0));
        assert!(events.try_recv().is_ok());
        let mut late = ask(client, &["Yes"]);
        late.asked_at = 4;
        broker.register(late.clone()).unwrap();
        app.set_questions(broker.snapshot());
        app.select_next();
        assert_eq!(
            app.selected().map(|q| q.id),
            Some(questions[2].id),
            "kept in its place"
        );
        assert_eq!(app.answered_with(questions[2].id), None);

        apply(&mut app, &broker, Action::Answer(0));
        let answered = ClientEvent::Answered {
            id: questions[2].id,
            answer: "Yes".into(),
        };
        assert_eq!(events.try_recv().ok(), Some(answered));
        assert_eq!(app.answered_with(questions[1].id), None, "replaced");
        assert!(!app.moved_on_its_own());

        app.select_next();
        assert_eq!(app.selected().map(|q| q.id), Some(late.id));
        wait_out(&mut app, &broker);
        assert_eq!(
            app.selected().map(|q| q.id),
            Some(late.id),
            "a choice stays"
        );
        assert!(!app.moved_on_its_own());
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
        wait_out(&mut app, &broker);
        for action in [
            Action::Answer(0),
            Action::Answer(0),
            Action::Delete,
            Action::Reply,
        ] {
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
    fn answers_a_question_that_arrives_after_the_list_emptied() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (mut events, _guard) = broker.connect(client);
        let first = ask(client, &["Yes"]);
        broker.register(first).unwrap();
        let mut app = App::default();
        app.set_questions(broker.snapshot());
        app.set_shown_buttons(9);

        apply(&mut app, &broker, Action::Answer(0));
        assert!(events.try_recv().is_ok());
        wait_out(&mut app, &broker);
        let next = ask(client, &["Allow"]);
        broker.register(next.clone()).unwrap();
        app.set_questions(broker.snapshot());
        apply(&mut app, &broker, Action::Answer(0));
        let answered = ClientEvent::Answered {
            id: next.id,
            answer: "Allow".into(),
        };
        assert_eq!(events.try_recv().ok(), Some(answered));
        assert_eq!(app.notice(), None);
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
        assert!(app.selected().is_none(), "not kept as answered");
        apply(&mut app, &broker, Action::Next);
        assert!(app.notice().is_none());
    }

    #[test]
    fn focuses_the_selected_pane_even_just_after_answering() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (_events, _guard) = broker.connect(client);
        let mut app = App::default();
        assert_eq!(
            apply(&mut app, &broker, Action::Focus),
            None,
            "no questions"
        );

        let first = ask(client, &["Yes"]);
        let mut next = ask(client, &["Yes"]);
        next.asked_at = 2;
        next.pane_id = PaneId("w1:p2".into());
        let mut app = second_selected(&broker, &[first.clone(), next.clone()]);
        let target = Some(Effect::Focus(next.id, next.pane_id.clone()));
        assert_eq!(apply(&mut app, &broker, Action::Focus), target);
        apply(&mut app, &broker, Action::Answer(0));
        assert_eq!(apply(&mut app, &broker, Action::Focus), target, "answered");

        wait_out(&mut app, &broker);
        assert_eq!(apply(&mut app, &broker, Action::Focus), None, "moved");
        assert!(app.notice().unwrap().contains("pick a question first"));
    }

    #[test]
    fn marks_only_a_question_whose_pane_was_focused() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (_events, _guard) = broker.connect(client);
        let mut question = ask(client, &[]);
        question.body = QuestionBody::Permission {
            request_id: "abcde".into(),
            tool_name: "Bash".into(),
            description: "d".into(),
            input_preview: "p".into(),
        };
        broker.register(question.clone()).unwrap();
        let mut app = App::default();
        app.set_questions(broker.snapshot());
        let pane = |status| PaneInfo {
            id: question.pane_id.clone(),
            status,
            label: String::new(),
        };
        let answer_in_pane = |broker: &Broker| {
            for status in [AgentStatus::Idle, AgentStatus::Working] {
                broker.reconcile_panes(broker.listing_start(), &[pane(status)]);
            }
        };

        app.set_notice("not answered: later");
        after_focus(&mut app, &broker, Err("not focused on w1:p1: x".into()));
        assert_eq!(app.notice(), Some("not answered: later"), "kept");
        answer_in_pane(&broker);
        assert_eq!(broker.snapshot().len(), 1, "no mark without a focus");

        app.clear_notice();
        after_focus(&mut app, &broker, Err("not focused on w1:p1: x".into()));
        assert_eq!(app.notice(), Some("not focused on w1:p1: x"));
        after_focus(&mut app, &broker, Ok(question.id));
        assert!(app.notice().is_some(), "a success leaves the notice");
        answer_in_pane(&broker);
        assert!(broker.snapshot().is_empty(), "answered in the pane");
    }

    fn with_links(client: ClientId, links: &[&str]) -> Question {
        let mut question = ask(client, &["Yes"]);
        if let QuestionBody::Ask { links: l, .. } = &mut question.body {
            *l = links.iter().map(|l| l.to_string()).collect();
        }
        question
    }

    #[test]
    fn opens_the_link_after_o_and_never_answers_with_that_digit() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (mut events, _guard) = broker.connect(client);
        let mut question = with_links(client, &["https://a.example/", "https://b.example/"]);
        if let QuestionBody::Ask { options, .. } = &mut question.body {
            options.push("No".into());
        }
        broker.register(question).unwrap();
        let mut app = App::default();
        app.set_questions(broker.snapshot());
        app.set_shown_buttons(2);
        app.set_shown_links(2);

        let open = |url: &str| Some(Effect::Open(url.into()));
        apply(&mut app, &broker, Action::Open);
        assert_eq!(
            apply(&mut app, &broker, Action::Answer(1)),
            open("https://b.example/")
        );
        assert_eq!(
            apply(&mut app, &broker, Action::Answer(1)),
            None,
            "held down"
        );
        assert_eq!(
            apply(&mut app, &broker, Action::Open),
            None,
            "o keeps waiting"
        );
        assert_eq!(
            apply(&mut app, &broker, Action::Answer(1)),
            None,
            "o 2 o 2 opens once"
        );
        assert_eq!(
            apply(&mut app, &broker, Action::Answer(0)),
            open("https://a.example/")
        );
        assert_eq!(
            apply(&mut app, &broker, Action::Answer(1)),
            open("https://b.example/")
        );
        app.set_shown_links(1);
        assert_eq!(
            apply(&mut app, &broker, Action::Answer(1)),
            None,
            "hidden link"
        );
        app.set_shown_links(0);
        assert_eq!(
            apply(&mut app, &broker, Action::Answer(0)),
            None,
            "nothing on screen"
        );
        assert!(app.opening(), "digits keep waiting");
        assert!(events.try_recv().is_err(), "no digit answered");

        let selected = app.selected_index();
        apply(&mut app, &broker, Action::Next);
        assert!(!app.opening(), "another key ends the wait");
        assert_eq!(app.selected_index(), selected, "and does nothing else");
        app.set_shown_links(2);
        apply(&mut app, &broker, Action::Open);
        assert_eq!(on_event(&mut app, &broker, &enter()), Step::Nothing);
        assert!(!app.opening(), "Enter only cancels");
        apply(&mut app, &broker, Action::Open);
        assert_eq!(apply(&mut app, &broker, Action::Delete), None);
        assert_eq!(broker.snapshot().len(), 1, "d only cancels");
        assert!(!app.opening());
        apply(&mut app, &broker, Action::Open);
        assert_eq!(apply(&mut app, &broker, Action::Focus), None, "f cancels o");
        assert!(!app.opening());

        app.set_shown_links(0);
        apply(&mut app, &broker, Action::Open);
        assert!(!app.opening(), "nothing to open");
    }

    #[test]
    fn waits_for_a_choice_before_opening_after_the_selection_moved() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (_events, _guard) = broker.connect(client);
        let mut app = App::default();
        app.set_shown_links(1);
        apply(&mut app, &broker, Action::Open);
        assert!(!app.opening(), "no questions");

        let first = with_links(client, &["https://a.example/"]);
        let mut next = with_links(client, &["https://b.example/"]);
        next.asked_at = 2;
        let mut app = second_selected(&broker, &[first.clone(), next]);
        app.set_shown_links(1);
        apply(&mut app, &broker, Action::Open);
        assert!(app.opening());
        let mut another = with_links(client, &[]);
        another.asked_at = 0;
        broker.register(another).unwrap();
        assert_eq!(app.selected_index(), Some(1));
        app.set_questions(broker.snapshot());
        assert_eq!(
            app.selected_index(),
            Some(2),
            "an older question came first"
        );
        assert!(app.opening(), "a new question does not end the wait");
        let opened = apply(&mut app, &broker, Action::Answer(0));
        assert_eq!(opened, Some(Effect::Open("https://b.example/".into())));
        // 開く待ちのまま、選んでいた質問が取り下げられた
        broker.withdraw(&app.selected().unwrap().id, WithdrawReason::Deleted);
        app.set_questions(broker.snapshot());
        assert!(!app.opening(), "the wait ends with the question");
        apply(&mut app, &broker, Action::Open);
        assert!(!app.opening(), "moved");
        assert!(app.notice().unwrap().contains("pick a question first"));
    }

    #[test]
    fn opens_only_plain_web_links() {
        for link in ["https://a.example/x", "HTTP://A.example"] {
            assert!(checked_url(link).is_ok(), "{link}");
        }
        for link in [
            "file:///etc/passwd",
            "vscode://x",
            "ftp://x",
            "-a",
            "not a url",
            "https://github.com@evil.example/",
            "https://u:p@example.com/",
            "https://:p@example.com/",
        ] {
            assert!(checked_url(link).is_err(), "{link}");
        }
    }

    #[test]
    fn answers_and_selects_by_click() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (mut events, _guard) = broker.connect(client);
        let first = with_links(client, &["https://a.example/"]);
        let mut next = with_links(client, &["https://b.example/"]);
        next.asked_at = 2;
        let mut app = second_selected(&broker, &[first.clone(), next.clone()]);
        app.set_shown_links(1);

        let click = |target| Action::Click(target);
        let opened = apply(&mut app, &broker, click(Target::Link(0)));
        assert_eq!(opened, Some(Effect::Open("https://b.example/".into())));
        assert_eq!(
            apply(&mut app, &broker, click(Target::Link(1))),
            None,
            "hidden link"
        );

        apply(&mut app, &broker, Action::Open);
        apply(&mut app, &broker, click(Target::Button(0)));
        assert!(!app.opening(), "a click ends the wait");
        let answered = ClientEvent::Answered {
            id: next.id,
            answer: "Yes".into(),
        };
        assert_eq!(events.try_recv().unwrap(), answered, "and answers");

        wait_out(&mut app, &broker);
        assert!(app.moved_on_its_own());
        apply(&mut app, &broker, click(Target::Button(0)));
        assert!(events.try_recv().is_err(), "not after the selection moved");
        let link = apply(&mut app, &broker, click(Target::Link(0)));
        assert_eq!(link, None, "no link either");
        apply(&mut app, &broker, click(Target::Question(first.id)));
        assert!(!app.moved_on_its_own(), "a click on the list is a choice");
        apply(&mut app, &broker, click(Target::Button(0)));
        assert!(events.try_recv().is_ok());
    }
}
