// TUI から呼ぶ操作は後続の PR で入るため、それまで未使用の警告を止める
#[allow(dead_code)]
mod broker;
mod config;
mod log;
mod text;
mod tui;

use std::process::ExitCode;
use std::sync::Arc;

use hailr_core::paths;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::oneshot;

use broker::{Broker, notifier, serve, socket};
use config::Config;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let config = match paths::config_path().map(|p| Config::load(&p)) {
        Some(Ok(config)) => config,
        Some(Err(e)) => {
            eprintln!("hailr: {e}");
            return ExitCode::FAILURE;
        }
        None => Config::default(),
    };
    let Some(path) = paths::socket_path() else {
        eprintln!("hailr: cannot decide the socket path (HOME is not an absolute path)");
        return ExitCode::FAILURE;
    };
    let (listener, socket_file) = match socket::bind(&path) {
        Ok(bound) => bound,
        Err(e) => {
            eprintln!("hailr: cannot listen on {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    match paths::log_path("hailr.log").map(|p| log::init(&p)) {
        Some(Ok(())) => {}
        Some(Err(e)) => eprintln!("hailr: cannot open the log file; running without it: {e}"),
        None => {}
    }
    tracing::info!("listening on {}", path.display());

    let broker = Broker::new();
    let mux = config.mux();
    let notifying = config.notify.toast.then(|| {
        tokio::spawn(notifier::run(
            Arc::clone(&broker),
            broker.arrivals(),
            Arc::clone(&mux),
        ))
    });
    let (quit_tx, quit_rx) = oneshot::channel::<()>();
    let quit = async {
        let _ = quit_rx.await;
    };
    let mut serving = tokio::spawn(serve::run(listener, socket_file, Arc::clone(&broker), quit));

    // broker が先に止まったら、質問が届かない画面を出し続けないよう TUI も閉じる
    let mut served = None;
    let shown = tui::run(broker, mux, config.ui, async {
        tokio::select! {
            () = terminated() => {}
            result = &mut serving => served = Some(result),
        }
    })
    .await;
    let _ = quit_tx.send(());
    let served = match served {
        Some(result) => result,
        None => serving.await,
    };
    if let Some(notifying) = notifying {
        notifying.abort();
    }

    let mut code = ExitCode::SUCCESS;
    if let Err(e) = shown {
        eprintln!("hailr: the screen failed: {e}");
        code = ExitCode::FAILURE;
    }
    match served {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            eprintln!("hailr: {e}");
            code = ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("hailr: the broker stopped unexpectedly: {e}");
            code = ExitCode::FAILURE;
        }
    }
    code
}

/// 端末を閉じたとき（SIGHUP）も、ソケットを消してから終わる
async fn terminated() {
    let mut interrupt = signal(SignalKind::interrupt()).expect("install the SIGINT handler");
    let mut term = signal(SignalKind::terminate()).expect("install the SIGTERM handler");
    let mut hangup = signal(SignalKind::hangup()).expect("install the SIGHUP handler");
    tokio::select! {
        _ = interrupt.recv() => {}
        _ = term.recv() => {}
        _ = hangup.recv() => {}
    }
}
