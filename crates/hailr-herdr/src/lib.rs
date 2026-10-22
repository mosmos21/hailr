mod agent_list;
mod cmd;

use std::path::PathBuf;
use std::time::Duration;

use hailr_core::mux::{Mux, MuxError, PaneInfo};
use hailr_core::protocol::PaneId;

const TIMEOUT: Duration = Duration::from_secs(2);

pub struct HerdrMux {
    bin: PathBuf,
}

impl HerdrMux {
    pub fn new() -> Self {
        Self::with_bin("herdr")
    }

    pub fn with_bin(bin: impl Into<PathBuf>) -> Self {
        Self { bin: bin.into() }
    }
}

impl Default for HerdrMux {
    fn default() -> Self {
        Self::new()
    }
}

impl Mux for HerdrMux {
    fn list_panes(&self) -> Result<Vec<PaneInfo>, MuxError> {
        let out = cmd::run(&self.bin, "agent list", &["agent", "list"], TIMEOUT)?;
        agent_list::parse(&out).map_err(|e| MuxError::Parse {
            command: format!("{} agent list", self.bin.display()),
            message: e.to_string(),
        })
    }

    fn read_pane(&self, pane: &PaneId) -> Result<String, MuxError> {
        let pane = checked(pane)?;
        let args = [
            "pane", "read", pane, "--source", "visible", "--format", "ansi",
        ];
        let out = cmd::run(&self.bin, "pane read", &args, TIMEOUT)?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    fn focus(&self, pane: &PaneId) -> Result<(), MuxError> {
        let pane = checked(pane)?;
        cmd::run(&self.bin, "agent focus", &["agent", "focus", pane], TIMEOUT).map(|_| ())
    }

    fn notify(&self, title: &str, body: &str) -> Result<(), MuxError> {
        let (title, body) = (displayable(title), displayable(body));
        let args = ["notification", "show", &title, "--body", &body];
        cmd::run(&self.bin, "notification show", &args, TIMEOUT).map(|_| ())
    }
}

// herdr の CLI は `--` に対応しておらず、`-` で始まる引数を位置によらずオプションとして読むため断る
fn checked(pane: &PaneId) -> Result<&str, MuxError> {
    if pane.0.is_empty() || pane.0.starts_with('-') {
        return Err(MuxError::InvalidPane);
    }
    Ok(&pane.0)
}

// 表示にしか使わない文字列なので、先頭の `-` を見た目の近い U+2010 に置き換えてオプション扱いを避ける
fn displayable(text: &str) -> String {
    let rest = text.trim_start_matches('-');
    "\u{2010}".repeat(text.len() - rest.len()) + rest
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    struct Stub {
        dir: PathBuf,
    }

    impl Stub {
        /// 引数を 1 行ずつ `args` に書き、`stdout` を出力する偽の herdr
        fn new(name: &str, stdout: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("hailr-herdr-{name}-{}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("stdout"), stdout).unwrap();
            let script = format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{0}/args'\ncat '{0}/stdout'\n",
                dir.display()
            );
            let bin = dir.join("herdr");
            fs::write(&bin, script).unwrap();
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
            Self { dir }
        }

        fn mux(&self) -> HerdrMux {
            HerdrMux::with_bin(self.dir.join("herdr"))
        }

        fn args(&self) -> Vec<String> {
            fs::read_to_string(self.dir.join("args"))
                .unwrap()
                .lines()
                .map(str::to_owned)
                .collect()
        }
    }

    impl Drop for Stub {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn pane(id: &str) -> PaneId {
        PaneId(id.to_owned())
    }

    #[test]
    fn list_panes_calls_agent_list() {
        let stub = Stub::new("list", r#"{"result":{"agents":[]}}"#);
        assert!(stub.mux().list_panes().unwrap().is_empty());
        assert_eq!(stub.args(), ["agent", "list"]);
    }

    #[test]
    fn read_pane_reads_the_visible_screen_with_ansi() {
        let stub = Stub::new("read", "\x1b[1mhi\x1b[0m");
        assert_eq!(
            stub.mux().read_pane(&pane("w1B:p1")).unwrap(),
            "\x1b[1mhi\x1b[0m"
        );
        assert_eq!(
            stub.args(),
            [
                "pane", "read", "w1B:p1", "--source", "visible", "--format", "ansi"
            ]
        );
    }

    #[test]
    fn read_pane_rejects_option_like_ids() {
        let stub = Stub::new("reject", "");
        for id in ["", "-x", "--help"] {
            let err = stub.mux().read_pane(&pane(id)).unwrap_err();
            assert!(matches!(err, MuxError::InvalidPane));
        }
        assert!(!stub.dir.join("args").exists());
    }

    #[test]
    fn notify_keeps_dashes_from_becoming_options() {
        let stub = Stub::new("notify", "");
        stub.mux().notify("--title", "-body - x").unwrap();
        assert_eq!(
            stub.args(),
            [
                "notification",
                "show",
                "\u{2010}\u{2010}title",
                "--body",
                "\u{2010}body - x"
            ]
        );
    }

    #[test]
    fn focuses_the_pane() {
        let stub = Stub::new("focus", "{}");
        stub.mux().focus(&pane("w1B:p1")).unwrap();
        assert_eq!(stub.args(), ["agent", "focus", "w1B:p1"]);
        for id in ["", "-x", "--help"] {
            let refused = stub.mux().focus(&pane(id));
            assert!(matches!(refused, Err(MuxError::InvalidPane)), "{id:?}");
        }
        assert_eq!(
            stub.args(),
            ["agent", "focus", "w1B:p1"],
            "not called again"
        );
    }
}
