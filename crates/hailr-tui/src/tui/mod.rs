mod app;
mod input;
mod preview;
mod view;

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;

use crossterm::event::EventStream;
use futures_util::StreamExt;
use hailr_core::mux::Mux;
use hailr_core::protocol::{PaneId, QuestionId, WithdrawReason};
use ratatui::backend::CrosstermBackend;
use ratatui::{DefaultTerminal, Terminal};
use tokio::sync::{Notify, mpsc, watch};

use crate::broker::Broker;
use crate::config::Side;
use app::{App, Target};
use input::Action;

/// 端末を代替画面に切り替えて質問を出す。q / Ctrl-C、`stop`、入力が閉じたときに戻して終わる
pub async fn run(
    broker: Arc<Broker>,
    mux: Arc<dyn Mux>,
    side: Side,
    stop: impl Future<Output = ()>,
) -> io::Result<()> {
    let started = ratatui::try_init().and_then(|terminal| {
        crossterm::execute!(io::stdout(), ReportClicks)?;
        Ok(terminal)
    });
    let mut terminal = match started {
        Ok(terminal) => terminal,
        Err(e) => {
            release_mouse();
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
        release_mouse();
        previous_hook(info);
    }));
    let result = run_loop(&mut terminal, &broker, mux, side, stop, &panicked).await;
    release_mouse();
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
fn release_mouse() {
    let _ = crossterm::execute!(io::stdout(), crossterm::event::DisableMouseCapture);
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
    let previewing = tokio::spawn(preview::run(Arc::clone(&mux), pane_rx, shot_tx));
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
                Some(Ok(event)) => match input::action(&event).or_else(|| input::click(&event, &app)) {
                    Some(Action::Quit) => return Ok(()),
                    Some(action) => match apply(&mut app, broker, action) {
                        Some(Effect::Open(link)) => {
                            if let Err(e) = checked_url(&link).and_then(|url| open_link(&url)) {
                                app.set_notice(format!("not opened: {e}"));
                            }
                        }
                        Some(Effect::Attach(id, pane)) => {
                            // crossterm の読み手が子に渡した端末の入力を横取りするため、渡す前に落とす
                            drop(events);
                            let attached = attach(terminal, &*mux, broker, &id, &pane, stop.as_mut(), panicked);
                            let Some(handover) = attached.await? else {
                                return Ok(());
                            };
                            discard_typeahead()?;
                            events = EventStream::new();
                            back_from_attach(&mut app, broker, &id, handover);
                        }
                        None => {}
                    },
                    None if input::is_press(&event) => {
                        app.clear_notice();
                        app.end_opening();
                    }
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

/// TUI の外で行うこと
#[derive(Debug, PartialEq, Eq)]
enum Effect {
    Attach(QuestionId, PaneId),
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
            | Action::Attach
            | Action::Open
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
        Action::Attach => {
            return app
                .selected()
                .map(|q| Effect::Attach(q.id, q.pane_id.clone()));
        }
        Action::Open => {
            if app.shown_links() > 0 {
                app.start_opening();
            }
        }
        Action::Quit => {}
    }
    None
}

enum Handover {
    /// 子を起こせなかった
    Refused(String),
    /// 人が attach から抜けた。herdr が 0 以外で終わったときは、その様子を持つ
    Returned(Option<String>),
}

enum Waited {
    Exited(io::Result<std::process::ExitStatus>),
    Stopped,
    Panicked,
}

/// stop のときは None
async fn attach(
    terminal: &mut DefaultTerminal,
    mux: &dyn Mux,
    broker: &Broker,
    id: &QuestionId,
    pane: &PaneId,
    stop: Pin<&mut impl Future<Output = ()>>,
    panicked: &Notify,
) -> io::Result<Option<Handover>> {
    let command = match mux.attach_command(pane) {
        Ok(command) => command,
        Err(e) => {
            tracing::warn!("cannot attach to {:?}: {e}", pane.0);
            return Ok(Some(Handover::Refused(e.to_string())));
        }
    };
    leave(terminal)?;
    let child = match spawn_attach(command, broker, id) {
        Ok(child) => child,
        Err(e) => {
            enter(terminal)?;
            tracing::warn!("cannot attach to {:?}: {e}", pane.0);
            return Ok(Some(Handover::Refused(format!("cannot start herdr: {e}"))));
        }
    };
    match wait_child(child, stop, panicked).await {
        Waited::Exited(result) => {
            enter(terminal)?;
            Ok(Some(Handover::Returned(ended(pane, result))))
        }
        Waited::Stopped => {
            reset_modes();
            Ok(None)
        }
        Waited::Panicked => {
            reset_modes();
            Err(io::Error::other("a task panicked"))
        }
    }
}

/// 起こせなかった pane には印を付けない
fn spawn_attach(
    command: std::process::Command,
    broker: &Broker,
    id: &QuestionId,
) -> io::Result<tokio::process::Child> {
    let child = tokio::process::Command::from(command)
        .kill_on_drop(true)
        .spawn()?;
    broker.mark_attached(id);
    Ok(child)
}

/// stop と panic のときは子を殺す
async fn wait_child(
    mut child: tokio::process::Child,
    stop: Pin<&mut impl Future<Output = ()>>,
    panicked: &Notify,
) -> Waited {
    let waited = tokio::select! {
        biased;
        () = panicked.notified() => Waited::Panicked,
        () = stop => Waited::Stopped,
        result = child.wait() => return Waited::Exited(result),
    };
    let _ = child.kill().await;
    waited
}

fn ended(pane: &PaneId, result: io::Result<std::process::ExitStatus>) -> Option<String> {
    match result {
        Ok(status) if status.success() => None,
        Ok(status) => {
            tracing::warn!("attach to {} ended with {status}", pane.0);
            Some(format!("herdr {status}"))
        }
        Err(e) => {
            tracing::warn!("cannot wait for the attach to {}: {e}", pane.0);
            Some(format!("cannot wait for herdr: {e}"))
        }
    }
}

/// raw mode は解かずに渡す。解くと crossterm が元の端末の設定を忘れ、herdr を殺したときに戻せなくなるため
fn leave(terminal: &mut DefaultTerminal) -> io::Result<()> {
    terminal.show_cursor()?;
    crossterm::execute!(
        io::stdout(),
        crossterm::event::DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen
    )
}

/// ratatui の init は panic の hook を重ねて入れ、`Terminal::clear` は戻った直後に答えの遅れる位置の問い合わせをするため、Terminal だけを作り直す
fn enter(terminal: &mut DefaultTerminal) -> io::Result<()> {
    crossterm::execute!(
        io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
        ReportClicks
    )?;
    *terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    Ok(())
}

/// 殺された herdr は自分で入れたモードを戻せないため、こちらで切る。端末が閉じていれば書けないので失敗は無視する
fn reset_modes() {
    let _ = crossterm::execute!(
        io::stdout(),
        crossterm::event::DisableMouseCapture,
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableFocusChange,
        crossterm::event::PopKeyboardEnhancementFlags,
        crossterm::style::Print("\x1b[?1l\x1b>"),
        crossterm::cursor::Show
    );
}

/// EventStream を落としても crossterm の内部に読み終えたキーが残り、戻った直後に再生されるため捨てる。
/// 落としたときの起こしが 1 回ぶん空の poll として返るため、空が 2 回続くまで読む
fn discard_typeahead() -> io::Result<()> {
    let mut empty = 0;
    while empty < 2 {
        if crossterm::event::poll(std::time::Duration::ZERO)? {
            crossterm::event::read()?;
            empty = 0;
        } else {
            empty += 1;
        }
    }
    Ok(())
}

/// attach の間につなぎ直しで写しが作り直されると印が消えるため、付け直す
fn back_from_attach(app: &mut App, broker: &Broker, id: &QuestionId, handover: Handover) {
    let notice = match handover {
        Handover::Refused(reason) => Some(format!("not attached: {reason}")),
        Handover::Returned(ended) => {
            broker.mark_attached(id);
            ended.map(|ended| format!("attach ended: {ended}"))
        }
    };
    app.set_questions(broker.snapshot());
    if let Some(notice) = notice {
        app.set_notice(notice);
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

    #[test]
    fn attaches_to_the_selected_pane_only_after_a_choice() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (_events, _guard) = broker.connect(client);
        let mut app = App::default();
        assert_eq!(
            apply(&mut app, &broker, Action::Attach),
            None,
            "no questions"
        );

        let first = ask(client, &["Yes"]);
        let mut next = ask(client, &[]);
        next.asked_at = 2;
        next.pane_id = PaneId("w1:p2".into());
        let mut app = second_selected(&broker, &[first.clone(), next.clone()]);
        let target = Some(Effect::Attach(next.id, next.pane_id.clone()));
        assert_eq!(apply(&mut app, &broker, Action::Attach), target);

        apply(&mut app, &broker, Action::Previous);
        apply(&mut app, &broker, Action::Answer(0));
        assert_eq!(apply(&mut app, &broker, Action::Attach), None, "moved");
        assert!(app.notice().unwrap().contains("pick a question first"));
    }

    #[test]
    fn marks_the_question_again_when_coming_back_from_the_pane() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let question = ask(client, &[]);
        let (_old, guard) = broker.connect(client);
        broker.register(question.clone()).unwrap();
        broker.mark_attached(&question.id);
        // attach の間に ask-mcp がつなぎ直し、同じ質問を登録し直した
        drop(guard);
        let (mut events, _guard) = broker.connect(client);
        broker.register(question.clone()).unwrap();
        let mut app = App::default();

        let ended = Some("herdr exit status: 1".into());
        back_from_attach(&mut app, &broker, &question.id, Handover::Returned(ended));
        assert_eq!(app.notice(), Some("attach ended: herdr exit status: 1"));
        assert_eq!(app.selected().map(|q| q.id), Some(question.id));
        for status in [AgentStatus::Idle, AgentStatus::Working] {
            let pane = PaneInfo {
                id: question.pane_id.clone(),
                status,
                label: String::new(),
            };
            broker.reconcile_panes(broker.listing_start(), &[pane]);
        }
        let withdrawn = ClientEvent::Withdrawn {
            id: question.id,
            reason: WithdrawReason::AnsweredInPane,
        };
        assert_eq!(events.try_recv().unwrap(), withdrawn);
    }

    #[test]
    fn does_not_mark_a_question_whose_pane_could_not_be_attached() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (mut events, _guard) = broker.connect(client);
        let question = ask(client, &[]);
        broker.register(question.clone()).unwrap();
        let mut app = App::default();

        let refused = Handover::Refused("invalid pane id".into());
        back_from_attach(&mut app, &broker, &question.id, refused);
        assert_eq!(app.notice(), Some("not attached: invalid pane id"));
        for status in [AgentStatus::Idle, AgentStatus::Working] {
            let pane = PaneInfo {
                id: question.pane_id.clone(),
                status,
                label: String::new(),
            };
            broker.reconcile_panes(broker.listing_start(), &[pane]);
        }
        assert!(events.try_recv().is_err(), "still waiting for an answer");
    }

    fn spawn(script: &str) -> tokio::process::Child {
        tokio::process::Command::new("sh")
            .args(["-c", script])
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    }

    #[tokio::test]
    async fn marks_the_question_only_once_the_attach_started() {
        let broker = Broker::new();
        let client = ClientId(Uuid::new_v4());
        let (mut events, _guard) = broker.connect(client);
        let question = ask(client, &[]);
        broker.register(question.clone()).unwrap();
        let answered_in_pane = |events: &mut tokio::sync::mpsc::Receiver<ClientEvent>| {
            for status in [AgentStatus::Idle, AgentStatus::Working] {
                let pane = PaneInfo {
                    id: question.pane_id.clone(),
                    status,
                    label: String::new(),
                };
                broker.reconcile_panes(broker.listing_start(), &[pane]);
            }
            events.try_recv().is_ok()
        };

        let missing = std::process::Command::new("/nonexistent/herdr");
        assert!(spawn_attach(missing, &broker, &question.id).is_err());
        assert!(!answered_in_pane(&mut events), "not marked");

        let mut child =
            spawn_attach(std::process::Command::new("true"), &broker, &question.id).unwrap();
        child.wait().await.unwrap();
        assert!(answered_in_pane(&mut events), "marked");
    }

    #[tokio::test]
    async fn tells_how_the_attach_ended() {
        let stop = std::future::pending::<()>();
        tokio::pin!(stop);
        let pane = PaneId("w1:p1".into());
        let cases = [
            ("exit 0", None),
            ("exit 3", Some("herdr exit status: 3")),
            ("kill -9 $$", Some("herdr signal: 9 (SIGKILL)")),
        ];
        for (script, expected) in cases {
            let waited = wait_child(spawn(script), stop.as_mut(), &Notify::new()).await;
            let Waited::Exited(result) = waited else {
                panic!("{script} should exit");
            };
            assert_eq!(ended(&pane, result).as_deref(), expected, "{script}");
        }
    }

    #[tokio::test]
    async fn kills_the_attach_on_stop_or_panic() {
        let stop = std::future::ready(());
        tokio::pin!(stop);
        let child = spawn("sleep 10");
        let pid = child.id().unwrap().to_string();
        let waited = wait_child(child, stop, &Notify::new()).await;
        assert!(matches!(waited, Waited::Stopped));
        assert!(!alive(&pid), "the child is gone on stop");

        let stop = std::future::pending::<()>();
        tokio::pin!(stop);
        let panicked = Notify::new();
        panicked.notify_one();
        let child = spawn("sleep 10");
        let pid = child.id().unwrap().to_string();
        let waited = wait_child(child, stop, &panicked).await;
        assert!(matches!(waited, Waited::Panicked));
        assert!(!alive(&pid), "the child is gone on panic");
    }

    fn alive(pid: &str) -> bool {
        std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
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
        assert_eq!(
            apply(&mut app, &broker, Action::Attach),
            None,
            "Enter only cancels"
        );
        apply(&mut app, &broker, Action::Open);
        assert_eq!(apply(&mut app, &broker, Action::Delete), None);
        assert_eq!(broker.snapshot().len(), 1, "d only cancels");
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
