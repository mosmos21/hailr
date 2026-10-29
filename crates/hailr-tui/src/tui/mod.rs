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

/// 選択が勝手に移ったときや `o` でリンクを開いたあと、押しっぱなしのキーを効かせない長さ
pub const MOVE_GUARD: Duration = Duration::from_secs(1);

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
                app.end_opening();
                // 開いた番号のキーリピートが回答にならないよう、選択が移ったときと同じく少しの間答えさせない
                app.hold_move();
                return Some(Effect::Open(url));
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
    let settling = matches!(
        action,
        Action::Answer(_) | Action::Delete | Action::Reply | Action::Click(Target::Button(_))
    );
    // 一覧が空の間も含め、押し続けたキーが移った先の質問（権限確認を含む）に届かないよう、測り直して捨てる。f も移した質問を消すので含める
    if (settling || matches!(action, Action::Focus)) && app.just_moved() {
        app.hold_move();
        return None;
    }
    if acts && app.selected().is_none() {
        return None;
    }
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

/// 移せた質問は pane で答えるので一覧から消す。遅れて届いた失敗で、あとから出た知らせを消さない
fn after_focus(app: &mut App, broker: &Broker, result: Result<QuestionId, String>) {
    match result {
        Ok(id) => {
            broker.withdraw(&id, WithdrawReason::AnsweredInPane);
            app.set_questions(broker.snapshot());
        }
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
mod tests;
