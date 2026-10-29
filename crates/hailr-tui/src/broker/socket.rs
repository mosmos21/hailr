use std::fs::{self, DirBuilder, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use tokio::net::UnixListener;

/// ソケットのファイル。drop したら消す
pub struct SocketFile {
    path: PathBuf,
}

impl Drop for SocketFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// 同じユーザーのプロセスだけが届くよう、置き場のディレクトリを 0700 にしてから待ち受ける。
/// ディレクトリの権限を変えるので、hailr 専用の `paths::socket_path()` にだけ使う
pub fn bind(path: &Path) -> io::Result<(UnixListener, SocketFile)> {
    if let Some(dir) = path.parent() {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        fs::set_permissions(dir, Permissions::from_mode(0o700))?;
    }
    remove_leftover(path)?;
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, Permissions::from_mode(0o600))?;
    Ok((
        listener,
        SocketFile {
            path: path.to_owned(),
        },
    ))
}

/// 前回の残骸のソケットだけを消す。ソケット以外のファイルや、動いている hailr のソケットには触れない
fn remove_leftover(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if !metadata.file_type().is_socket() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the socket path is taken by something that is not a socket",
        ));
    }
    match UnixStream::connect(path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "another hailr is already serving this socket",
        )),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => fs::remove_file(path),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[tokio::test]
    async fn creates_a_private_directory_and_removes_the_socket_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state/broker.sock");
        let (_listener, file) = bind(&path).unwrap();
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert!(path.exists());
        drop(file);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn tightens_an_existing_directory() {
        let tmp = tempfile::tempdir().unwrap();
        fs::set_permissions(tmp.path(), Permissions::from_mode(0o755)).unwrap();
        let (_listener, _file) = bind(&tmp.path().join("broker.sock")).unwrap();
        assert_eq!(mode(tmp.path()), 0o700);
    }

    #[tokio::test]
    async fn replaces_a_leftover_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("broker.sock");
        let (listener, file) = bind(&path).unwrap();
        drop(listener);
        // 異常終了を真似て、ファイルだけを残す
        std::mem::forget(file);
        assert!(bind(&path).is_ok());
    }

    #[tokio::test]
    async fn the_socket_file_is_private() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("broker.sock");
        let (_listener, _file) = bind(&path).unwrap();
        assert_eq!(mode(&path), 0o600);
    }

    #[tokio::test]
    async fn never_removes_something_that_is_not_a_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("broker.sock");
        fs::write(&file, "keep me").unwrap();
        let link = tmp.path().join("link.sock");
        std::os::unix::fs::symlink(&file, &link).unwrap();

        for path in [&file, &link] {
            let err = bind(path).err().unwrap();
            assert_eq!(err.kind(), io::ErrorKind::AlreadyExists, "{path:?}");
        }
        assert_eq!(fs::read_to_string(&file).unwrap(), "keep me");
        assert!(fs::symlink_metadata(&link).is_ok());
    }

    #[tokio::test]
    async fn refuses_a_socket_that_is_in_use() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("broker.sock");
        let (_listener, _file) = bind(&path).unwrap();
        let err = bind(&path).err().unwrap();
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
        assert!(path.exists());
    }
}
