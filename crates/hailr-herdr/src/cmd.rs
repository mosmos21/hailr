use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use hailr_core::mux::MuxError;
use wait_timeout::ChildExt;

const STDOUT_LIMIT: u64 = 16 * 1024 * 1024;
const STDERR_LIMIT: u64 = 64 * 1024;

/// 標準出力を返す。`name` はエラーに載せる名前で、ユーザー由来の引数を含めない
pub(crate) fn run(
    bin: &Path,
    name: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<Vec<u8>, MuxError> {
    let command = format!("{} {name}", bin.display());
    let deadline = Instant::now() + timeout;
    // 期限切れのときに孫までまとめて止めるため、子を自分のプロセスグループに入れる
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|source| MuxError::Spawn {
            command: command.clone(),
            source,
        })?;

    // パイプの容量を超える出力で子が止まらないよう、待つのと並行して読み切る
    let stdout = drain(child.stdout.take(), STDOUT_LIMIT);
    let stderr = drain(child.stderr.take(), STDERR_LIMIT);

    let status = match child.wait_timeout(timeout) {
        Ok(Some(status)) => status,
        Ok(None) => {
            kill_group(&mut child);
            return Err(MuxError::Timeout { command, timeout });
        }
        Err(source) => {
            kill_group(&mut child);
            return Err(MuxError::Wait { command, source });
        }
    };

    // 子が終わってもパイプを引き継いだ孫が残っていると EOF が来ないため、読み取りも期限内で打ち切る
    let remaining = || deadline.saturating_duration_since(Instant::now());
    let (Ok(stdout), Ok(stderr)) = (
        stdout.recv_timeout(remaining()),
        stderr.recv_timeout(remaining()),
    ) else {
        kill_group(&mut child);
        return Err(MuxError::Timeout { command, timeout });
    };

    if !status.success() {
        return Err(MuxError::Failed {
            command,
            status: status.to_string(),
            stderr: String::from_utf8_lossy(&stderr).trim().to_owned(),
        });
    }
    Ok(stdout)
}

fn kill_group(child: &mut Child) {
    // SAFETY: killpg はシグナルを送るだけで、このプロセスのメモリに触れない
    unsafe {
        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
    }
    let _ = child.wait();
}

fn drain(pipe: Option<impl Read + Send + 'static>, limit: u64) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.by_ref().take(limit).read_to_end(&mut buf);
            // 上限を超えた分も読み捨てないと、書き込み側の子が止まる
            let _ = io::copy(&mut pipe, &mut io::sink());
        }
        let _ = tx.send(buf);
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str, timeout: Duration) -> Result<Vec<u8>, MuxError> {
        run(Path::new("sh"), "test", &["-c", script], timeout)
    }

    #[test]
    fn returns_stdout() {
        assert_eq!(
            sh("printf hello", Duration::from_secs(2)).unwrap(),
            b"hello"
        );
    }

    #[test]
    fn reads_output_larger_than_the_pipe_buffer() {
        let out = sh("head -c 1000000 /dev/zero", Duration::from_secs(5)).unwrap();
        assert_eq!(out.len(), 1_000_000);
    }

    #[test]
    fn caps_stdout() {
        let script = format!("head -c {} /dev/zero", STDOUT_LIMIT + 1024);
        let out = sh(&script, Duration::from_secs(10)).unwrap();
        assert_eq!(out.len() as u64, STDOUT_LIMIT);
    }

    #[test]
    fn stops_an_endless_writer_at_the_deadline() {
        let started = Instant::now();
        let err = sh("yes", Duration::from_millis(500)).unwrap_err();
        assert!(matches!(err, MuxError::Timeout { .. }));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn keeps_stdout_when_stderr_floods() {
        let out = sh(
            "head -c 20000000 /dev/zero >&2; printf ok",
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(out, b"ok");
    }

    #[test]
    fn caps_stderr_on_failure() {
        let err = sh(
            "head -c 1000000 /dev/zero | tr '\\0' x >&2; exit 1",
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert!(
            matches!(err, MuxError::Failed { ref stderr, .. } if stderr.len() as u64 == STDERR_LIMIT)
        );
    }

    #[test]
    fn kills_after_timeout() {
        let started = Instant::now();
        let err = sh("sleep 5", Duration::from_millis(100)).unwrap_err();
        assert!(matches!(err, MuxError::Timeout { .. }));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// 孫の pid をファイルに書かせて `run` を呼び、期限切れになったあと孫が消えることを確かめる
    fn assert_grandchild_killed(script: impl Fn(&str) -> String, timeout: Duration) {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let started = Instant::now();
        let err = sh(&script(&pid_file.display().to_string()), timeout).unwrap_err();
        assert!(matches!(err, MuxError::Timeout { .. }));
        assert!(started.elapsed() < Duration::from_secs(2));

        let pid: libc::pid_t = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // 殺された孫は回収されるまで残るため、すぐには消えない
        let deadline = Instant::now() + Duration::from_secs(2);
        // SAFETY: シグナル 0 は存在の確認だけで、何も送らない
        while unsafe { libc::kill(pid, 0) } == 0 {
            assert!(Instant::now() < deadline, "grandchild {pid} is still alive");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn kills_grandchildren_after_wait_timeout() {
        // 出力を閉じてから走り続けるので、読み取りは終わり、終了待ちの期限切れの経路を通る
        assert_grandchild_killed(
            |f| format!("sleep 5 >&- 2>&- & echo $! > '{f}'; exec >&- 2>&-; sleep 5"),
            Duration::from_millis(300),
        );
    }

    #[test]
    fn kills_grandchildren_after_read_timeout() {
        assert_grandchild_killed(
            |f| format!("sleep 5 & echo $! > '{f}'; sleep 5"),
            Duration::from_millis(300),
        );
    }

    #[test]
    fn gives_up_on_a_grandchild_holding_the_pipe() {
        assert_grandchild_killed(
            |f| format!("sleep 5 & echo $! > '{f}'"),
            Duration::from_millis(300),
        );
    }

    #[test]
    fn reports_failure_with_stderr() {
        let err = sh("echo boom >&2; exit 3", Duration::from_secs(2)).unwrap_err();
        assert!(matches!(err, MuxError::Failed { ref stderr, .. } if stderr == "boom"));
    }

    #[test]
    fn keeps_arguments_out_of_errors() {
        let err = sh("exit 1; secret", Duration::from_secs(2)).unwrap_err();
        assert!(!err.to_string().contains("secret"));
    }

    #[test]
    fn reports_missing_binary() {
        let err = run(
            Path::new("hailr-no-such-bin"),
            "test",
            &[],
            Duration::from_secs(2),
        )
        .unwrap_err();
        assert!(matches!(err, MuxError::Spawn { .. }));
    }
}
