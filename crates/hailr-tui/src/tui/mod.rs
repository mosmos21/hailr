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
                    Some(Action::Previous) => app.select_previous(),
                    Some(Action::Next) => app.select_next(),
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

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
