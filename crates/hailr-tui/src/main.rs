// TUI から呼ぶ操作は後続の PR で入るため、それまで未使用の警告を止める
#[allow(dead_code)]
mod broker;
mod config;

use std::process::ExitCode;
use std::sync::Arc;

use hailr_core::paths;
use tokio::signal::unix::{SignalKind, signal};

use broker::{Broker, notifier, poller, serve, socket};
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
    eprintln!("hailr: listening on {}", path.display());

    let broker = Broker::new();
    let mux = config.mux();
    let notifying = tokio::spawn(notifier::run(
        Arc::clone(&broker),
        broker.arrivals(),
        Arc::clone(&mux),
    ));
    let polling = tokio::spawn(poller::run(Arc::clone(&broker), mux));
    let result = serve::run(listener, socket_file, broker, terminated()).await;
    polling.abort();
    notifying.abort();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hailr: {e}");
            ExitCode::FAILURE
        }
    }
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
