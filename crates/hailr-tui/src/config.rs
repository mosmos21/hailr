use std::path::Path;
use std::sync::Arc;
use std::{fs, io};

use hailr_core::mux::Mux;
use hailr_herdr::HerdrMux;
use serde::Deserialize;

/// 端末マルチプレクサの実装の名前を並べるので、core ではなく実装を全部持つ binary に置く
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub mux: MuxKind,
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
        let path = std::env::temp_dir().join(format!("hailr-no-config-{}", std::process::id()));
        assert_eq!(Config::load(&path).unwrap(), Config::default());
    }

    #[test]
    fn reads_the_mux() {
        assert_eq!(toml::from_str::<Config>("").unwrap().mux, MuxKind::Herdr);
        assert_eq!(
            toml::from_str::<Config>("mux = \"herdr\"").unwrap().mux,
            MuxKind::Herdr
        );
    }

    #[test]
    fn rejects_unknown_values_and_keys() {
        for input in ["mux = \"tmux\"", "mux = \"Herdr\"", "muxx = \"herdr\""] {
            assert!(toml::from_str::<Config>(input).is_err(), "{input}");
        }
    }
}
