mod agent_list;
mod cmd;

use std::path::PathBuf;
use std::time::Duration;

use hailr_core::mux::{Mux, MuxError, PaneInfo};
use hailr_core::protocol::PaneId;

const TIMEOUT: Duration = Duration::from_secs(2);

// Ghostty が動いていないときに起動させないよう、動いているときだけ herdr の terminal を前に出す
const RAISE_HERDR: &str = r#"if application "Ghostty" is running then
    tell application "Ghostty" to focus (first terminal whose name is "herdr")
end if"#;

pub struct HerdrMux {
    bin: PathBuf,
    osascript: Option<PathBuf>,
}

impl HerdrMux {
    pub fn new() -> Self {
        Self {
            osascript: cfg!(target_os = "macos").then(|| "/usr/bin/osascript".into()),
            ..Self::with_bin("herdr")
        }
    }

    /// herdr の場所だけを差し替える。ウィンドウを前に出す osascript は呼ばない
    pub fn with_bin(bin: impl Into<PathBuf>) -> Self {
        Self {
            bin: bin.into(),
            osascript: None,
        }
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
        cmd::run(&self.bin, "agent focus", &["agent", "focus", pane], TIMEOUT)?;
        // herdr の中では切り替わっているため、前に出せなくても成功として返す
        if let Some(osascript) = &self.osascript {
            let _ = cmd::run(osascript, "raise herdr", &["-e", RAISE_HERDR], TIMEOUT);
        }
        Ok(())
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
        dir: tempfile::TempDir,
    }

    impl Stub {
        /// 引数を 1 行ずつ `args` に書き、`stdout` を出力する偽の herdr
        fn new(stdout: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("stdout"), stdout).unwrap();
            let script = format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{0}/args'\ncat '{0}/stdout'\n",
                dir.path().display()
            );
            let bin = dir.path().join("herdr");
            fs::write(&bin, script).unwrap();
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
            Self { dir }
        }

        fn mux(&self) -> HerdrMux {
            HerdrMux::with_bin(self.bin())
        }

        fn bin(&self) -> PathBuf {
            self.dir.path().join("herdr")
        }

        fn args(&self) -> Vec<String> {
            fs::read_to_string(self.dir.path().join("args"))
                .unwrap()
                .lines()
                .map(str::to_owned)
                .collect()
        }
    }

    fn pane(id: &str) -> PaneId {
        PaneId(id.to_owned())
    }

    #[test]
    fn list_panes_calls_agent_list() {
        let stub = Stub::new(r#"{"result":{"agents":[]}}"#);
        assert!(stub.mux().list_panes().unwrap().is_empty());
        assert_eq!(stub.args(), ["agent", "list"]);
    }

    #[test]
    fn read_pane_reads_the_visible_screen_with_ansi() {
        let stub = Stub::new("\x1b[1mhi\x1b[0m");
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
        let stub = Stub::new("");
        for id in ["", "-x", "--help"] {
            let err = stub.mux().read_pane(&pane(id)).unwrap_err();
            assert!(matches!(err, MuxError::InvalidPane));
        }
        assert!(!stub.dir.path().join("args").exists());
    }

    #[test]
    fn notify_keeps_dashes_from_becoming_options() {
        let stub = Stub::new("");
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
        let stub = Stub::new("{}");
        stub.mux().focus(&pane("w1B:p1")).unwrap();
        assert_eq!(stub.args(), ["agent", "focus", "w1B:p1"]);
        let refused = stub.mux().focus(&pane("-x"));
        assert!(matches!(refused, Err(MuxError::InvalidPane)));
        assert_eq!(
            stub.args(),
            ["agent", "focus", "w1B:p1"],
            "not called again"
        );
    }

    #[test]
    fn brings_the_herdr_window_forward_after_focusing() {
        let herdr = Stub::new("{}");
        let osascript = Stub::new("");
        let mux = HerdrMux {
            osascript: Some(osascript.bin()),
            ..herdr.mux()
        };
        mux.focus(&pane("w1B:p1")).unwrap();
        assert_eq!(herdr.args(), ["agent", "focus", "w1B:p1"]);
        let script: Vec<String> = RAISE_HERDR.lines().map(str::to_owned).collect();
        assert_eq!(osascript.args(), [vec!["-e".to_owned()], script].concat());

        let mux = HerdrMux {
            osascript: Some(osascript.dir.path().join("missing")),
            ..herdr.mux()
        };
        assert!(mux.focus(&pane("w1B:p1")).is_ok(), "focused even so");
        assert!(HerdrMux::with_bin("herdr").osascript.is_none());
    }

    #[test]
    fn leaves_the_window_alone_when_focus_fails() {
        let osascript = Stub::new("");
        let mux = HerdrMux {
            osascript: Some(osascript.bin()),
            ..HerdrMux::with_bin(osascript.dir.path().join("missing"))
        };
        assert!(mux.focus(&pane("w1B:p1")).is_err());
        assert!(!osascript.dir.path().join("args").exists());
    }
}
