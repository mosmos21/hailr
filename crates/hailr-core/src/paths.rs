use std::ffi::OsString;
use std::path::PathBuf;

const APP: &str = "hailr";

pub fn state_dir() -> Option<PathBuf> {
    state_dir_from(|key| std::env::var_os(key))
}

pub fn socket_path() -> Option<PathBuf> {
    state_dir().map(|dir| dir.join("broker.sock"))
}

pub fn log_path(name: &'static str) -> Option<PathBuf> {
    state_dir().map(|dir| dir.join(name))
}

pub fn config_path() -> Option<PathBuf> {
    config_dir_from(|key| std::env::var_os(key)).map(|dir| dir.join("config.toml"))
}

fn state_dir_from(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    xdg_dir(&env, "XDG_STATE_HOME", ".local/state")
}

fn config_dir_from(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    xdg_dir(&env, "XDG_CONFIG_HOME", ".config")
}

// 相対パスだと broker と ask-mcp で作業ディレクトリが違えば別の場所を指すため、絶対パスのときだけ使う
fn xdg_dir(env: &impl Fn(&str) -> Option<OsString>, key: &str, fallback: &str) -> Option<PathBuf> {
    let absolute = |key: &str| env(key).map(PathBuf::from).filter(|p| p.is_absolute());
    let base = absolute(key).or_else(|| absolute("HOME").map(|home| home.join(fallback)))?;
    Some(base.join(APP))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn uses_xdg_state_home_when_absolute() {
        let dir = state_dir_from(env(&[("XDG_STATE_HOME", "/x/state"), ("HOME", "/home/u")]));
        assert_eq!(dir, Some(PathBuf::from("/x/state/hailr")));
    }

    #[test]
    fn ignores_relative_xdg_state_home() {
        let dir = state_dir_from(env(&[("XDG_STATE_HOME", "state"), ("HOME", "/home/u")]));
        assert_eq!(dir, Some(PathBuf::from("/home/u/.local/state/hailr")));
    }

    #[test]
    fn falls_back_to_home_for_config() {
        let dir = config_dir_from(env(&[("HOME", "/home/u")]));
        assert_eq!(dir, Some(PathBuf::from("/home/u/.config/hailr")));
    }

    #[test]
    fn ignores_empty_xdg_state_home() {
        let dir = state_dir_from(env(&[("XDG_STATE_HOME", ""), ("HOME", "/home/u")]));
        assert_eq!(dir, Some(PathBuf::from("/home/u/.local/state/hailr")));
    }

    #[test]
    fn none_without_an_absolute_home() {
        assert_eq!(state_dir_from(env(&[])), None);
        assert_eq!(state_dir_from(env(&[("HOME", "")])), None);
        assert_eq!(config_dir_from(env(&[("HOME", "rel")])), None);
    }
}
