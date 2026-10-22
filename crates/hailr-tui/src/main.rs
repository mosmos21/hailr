// TUI から呼ぶ操作は後続の PR で入るため、それまで未使用の警告を止める
#[allow(dead_code)]
mod broker;

use std::process::ExitCode;

use hailr_core::paths;
use tokio::signal::unix::{SignalKind, signal};

use broker::{Broker, serve, socket};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
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

    match serve::run(listener, socket_file, Broker::new(), terminated()).await {
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
