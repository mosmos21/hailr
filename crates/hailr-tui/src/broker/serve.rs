use std::future::Future;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UnixListener;

use super::socket::SocketFile;
use super::{Broker, http};

/// 終了の合図のあと、接続が閉じるのをこれ以上は待たない
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// `shutdown` が終わるまで待ち受ける。終了時は SSE を閉じてから、期限付きで接続の終わりを待つ
pub async fn run(
    listener: UnixListener,
    socket_file: SocketFile,
    broker: Arc<Broker>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    let (stopping_tx, stopping_rx) = tokio::sync::oneshot::channel::<()>();
    let closer = Arc::clone(&broker);
    let server = axum::serve(listener, http::router(broker)).with_graceful_shutdown(async move {
        shutdown.await;
        // 猶予の間に起動した次の hailr のソケットを後から消さないよう、待ち受けをやめた時点で消す
        drop(socket_file);
        // with_graceful_shutdown は SSE の接続が終わるまで待つため、先に送り手を手放してストリームを終わらせる
        closer.close_all();
        let _ = stopping_tx.send(());
    });
    let deadline = async {
        if stopping_rx.await.is_ok() {
            tokio::time::sleep(SHUTDOWN_GRACE).await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        result = server => result,
        () = deadline => {
            tracing::warn!("closed connections that did not finish in time");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Instant;

    use hailr_core::protocol::{ClientId, Question};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;
    use tokio::sync::oneshot;
    use uuid::Uuid;

    use super::super::{RegisterError, socket};
    use super::*;
    use crate::test_support::{self, QuestionExt};

    struct Server {
        path: PathBuf,
        broker: Arc<Broker>,
        stop: Option<oneshot::Sender<()>>,
        task: tokio::task::JoinHandle<io::Result<()>>,
        _dir: tempfile::TempDir,
    }

    impl Server {
        async fn start() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("b.sock");
            let (listener, file) = socket::bind(&path).unwrap();
            let broker = Broker::new();
            let (stop, stopped) = oneshot::channel::<()>();
            let task = tokio::spawn(run(listener, file, Arc::clone(&broker), async {
                let _ = stopped.await;
            }));
            Self {
                _dir: dir,
                path,
                broker,
                stop: Some(stop),
                task,
            }
        }

        /// SSE をつなぎ、応答のヘッダーまで読んだストリームを返す
        async fn subscribe(&self, client: ClientId) -> UnixStream {
            let mut stream = UnixStream::connect(&self.path).await.unwrap();
            let request = format!(
                "GET /clients/{}/events HTTP/1.1\r\nHost: hailr\r\n\r\n",
                client.0
            );
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0u8; 1];
                stream.read_exact(&mut byte).await.unwrap();
                head.push(byte[0]);
            }
            assert!(head.starts_with(b"HTTP/1.1 200"));
            stream
        }
    }

    fn question(client: ClientId) -> Question {
        test_support::ask().client(client)
    }

    #[tokio::test]
    async fn notices_a_closed_sse_connection_quickly() {
        let server = Server::start().await;
        let client = ClientId(Uuid::new_v4());
        let stream = server.subscribe(client).await;
        assert!(server.broker.register(question(client)).is_ok());

        drop(stream);
        let started = Instant::now();
        // keep-alive の 15 秒を待たずに気づくことを確かめる
        while server.broker.register(question(client)) != Err(RegisterError::NoClient) {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the disconnect went unnoticed"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(server.broker.snapshot().is_empty());
    }

    #[tokio::test]
    async fn shuts_down_without_waiting_for_open_streams() {
        let mut server = Server::start().await;
        let mut stream = server.subscribe(ClientId(Uuid::new_v4())).await;

        let started = Instant::now();
        server.stop.take().unwrap().send(()).unwrap();
        (&mut server.task).await.unwrap().unwrap();
        assert!(
            started.elapsed() < SHUTDOWN_GRACE,
            "waited for the grace period"
        );

        // 送り手を手放したので、SSE は終わりを受け取って閉じている
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).await.unwrap();
        assert!(!server.path.exists(), "the socket file was left behind");
    }
}
