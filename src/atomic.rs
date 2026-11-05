//! 임시 파일에 쓴 뒤 이름을 바꾸는 방식의 원자적 파일 쓰기.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// `path`에 `data`를 원자적으로 기록한다.
///
/// 같은 폴더에 임시 파일을 만들고 `fsync` 후 `rename`한다.
/// `private`가 true이면 유닉스에서 파일 권한을 0600으로 만든다
/// (Windows에서는 사용자 프로필 ACL에 맡긴다).
pub fn write_atomic(path: &Path, data: &[u8], private: bool) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let tmp: PathBuf = parent.join(format!(".{file_name}.tmp-{}", std::process::id()));

    let result = (|| {
        let mut file = open_tmp(&tmp, private)?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();

    if result.is_err() {
        // 정리는 최선 노력: 실패해도 원래 오류를 우선한다.
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(unix)]
fn open_tmp(tmp: &Path, private: bool) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    if private {
        opts.mode(0o600);
    }
    opts.open(tmp)
}

#[cfg(not(unix))]
fn open_tmp(tmp: &Path, _private: bool) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(tmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_new_file_and_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/c.txt");
        write_atomic(&path, b"hello", false).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"hello");
    }

    #[test]
    fn overwrites_existing_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.yml");
        write_atomic(&path, b"one", false).unwrap();
        write_atomic(&path, b"two", false).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, vec!["x.yml".to_string()]);
    }

    #[test]
    fn failure_cleans_up_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        // 대상이 비어 있지 않은 디렉터리라 rename이 실패한다.
        let target = dir.path().join("target");
        fs::create_dir_all(target.join("child")).unwrap();
        assert!(write_atomic(&target, b"data", false).is_err());
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["target".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn private_files_get_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        write_atomic(&path, b"s", true).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
