use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Mutex;

/// 起動したあとの出来事をファイルに書く。TUI が端末を使うため stderr には出さない。
/// 前回のログを `.1` に退かすので、動いている hailr のログを退かさないようソケットを取れてから呼ぶ
pub fn init(path: &Path) -> io::Result<()> {
    let file = open(path)?;
    tracing_subscriber::fmt()
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .with_target(false)
        .init();
    // TUI が端末を使っている間の panic は画面ごと消えるため、ログにも残す
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("{info}");
        default_hook(info);
    }));
    Ok(())
}

fn open(path: &Path) -> io::Result<File> {
    let mut previous = path.as_os_str().to_owned();
    previous.push(".1");
    match fs::rename(path, &previous) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    // 退かしたあとに何かが置かれていたら、symlink でも辿らずに断るため create_new で開く
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;

    #[test]
    fn keeps_one_previous_log_and_starts_a_private_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("hailr.log");
        for run in ["first", "second", "third"] {
            let mut file = open(&path).unwrap();
            file.write_all(run.as_bytes()).unwrap();
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "third");
        assert_eq!(
            fs::read_to_string(dir.join("hailr.log.1")).unwrap(),
            "second"
        );
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn never_writes_through_a_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let target = dir.join("target");
        fs::write(&target, "keep").unwrap();
        let path = dir.join("hailr.log");
        symlink(&target, &path).unwrap();

        open(&path).unwrap().write_all(b"log").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
        let created = fs::symlink_metadata(&path).unwrap();
        assert!(created.file_type().is_file());
        assert_eq!(created.permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::read_to_string(&path).unwrap(), "log");
        assert!(
            fs::symlink_metadata(dir.join("hailr.log.1"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
