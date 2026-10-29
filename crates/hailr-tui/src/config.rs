use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use std::{fs, io};

use hailr_core::mux::Mux;
use hailr_herdr::HerdrMux;
use serde::{Deserialize, Deserializer};

/// 端末マルチプレクサの実装の名前を並べるので、core ではなく実装を全部持つ binary に置く
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub mux: MuxKind,
    pub ui: Ui,
    pub notify: Notify,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Notify {
    pub toast: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ui {
    pub questions: Side,
    /// 答えた質問を残す秒数
    #[serde(deserialize_with = "seconds")]
    pub answered_delay: Duration,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            questions: Side::default(),
            answered_delay: crate::tui::ANSWERED_FOR,
        }
    }
}

/// 大きすぎると時刻の足し算があふれて落ちるため、1 時間までにする
fn seconds<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
    let seconds = f64::deserialize(deserializer)?;
    let delay = Duration::try_from_secs_f64(seconds).map_err(serde::de::Error::custom)?;
    if delay > Duration::from_secs(3600) {
        return Err(serde::de::Error::custom(
            "answered_delay must be at most 3600 seconds",
        ));
    }
    Ok(delay)
}

/// 質問の一覧を画面のどちら側に置くか
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    #[default]
    Right,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MuxKind {
    #[default]
    Herdr,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read the config file: {0}")]
    Read(#[from] io::Error),
    #[error("invalid config file: {0}")]
    Parse(#[from] toml::de::Error),
}

impl Config {
    /// ファイルが無ければ既定値を使う
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn mux(&self) -> Arc<dyn Mux> {
        match self.mux {
            MuxKind::Herdr => Arc::new(HerdrMux::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_uses_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
    }

    #[test]
    fn reads_how_long_to_keep_an_answered_question() {
        let delay = |input: &str| toml::from_str::<Config>(input).unwrap().ui.answered_delay;
        assert_eq!(delay(""), crate::tui::ANSWERED_FOR);
        assert_eq!(delay("[ui]\nanswered_delay = 0"), Duration::ZERO);
        assert_eq!(delay("[ui]\nanswered_delay = 2"), Duration::from_secs(2));
        assert_eq!(
            delay("[ui]\nanswered_delay = 0.5"),
            Duration::from_millis(500)
        );
        assert_eq!(
            delay("[ui]\nanswered_delay = 3600"),
            Duration::from_secs(3600)
        );
    }

    #[test]
    fn reads_an_empty_notify_section() {
        assert!(!toml::from_str::<Config>("[notify]").unwrap().notify.toast);
    }

    #[test]
    fn rejects_unknown_values_and_keys() {
        for input in [
            "mux = \"tmux\"",
            "mux = \"Herdr\"",
            "muxx = \"herdr\"",
            "[ui]\nquestions = \"top\"",
            "[ui]\nquestions = \"Left\"",
            "[ui]\nlist = \"left\"",
            "[notify]\ntoast = \"yes\"",
            "[notify]\nsound = true",
            "[ui]\nanswered_delay = -1",
            "[ui]\nanswered_delay = nan",
            "[ui]\nanswered_delay = 3600.5",
            "[ui]\nanswered_delay = 1e19",
            "[ui]\nanswered_delay = \"1.5\"",
        ] {
            assert!(toml::from_str::<Config>(input).is_err(), "{input}");
        }
    }
}
