use serde::Deserialize;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub mux: MuxKind,
    pub ui: UiConfig,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MuxKind {
    #[default]
    Herdr,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    /// 質問の一覧を置く側
    pub questions: Side,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    #[default]
    Right,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_uses_defaults() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.mux, MuxKind::Herdr);
        assert_eq!(config.ui.questions, Side::Right);
    }

    #[test]
    fn reads_questions_side() {
        let config: Config = toml::from_str("[ui]\nquestions = \"left\"\n").unwrap();
        assert_eq!(config.ui.questions, Side::Left);
    }

    #[test]
    fn rejects_unknown_keys() {
        assert!(toml::from_str::<Config>("[ui]\nquestion = \"left\"\n").is_err());
    }
}
