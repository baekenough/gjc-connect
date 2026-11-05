//! API 키 보관.
//!
//! 우선 OS 키체인(`keyring`)에 저장하고, 키체인을 쓸 수 없으면
//! `data/secrets.json` 파일(유닉스에서는 0600)로 대체한다.
//! `models.yml`에는 키를 절대 쓰지 않는다.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::atomic::write_atomic;

/// 키체인 서비스 이름.
pub const KEYRING_SERVICE: &str = "gjc-connect";

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("키체인 오류: {0}")]
    Keyring(String),
    #[error("키 파일을 읽거나 쓸 수 없습니다 ({path}): {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("키 파일 형식이 올바르지 않습니다 ({path}): {message}")]
    Parse { path: PathBuf, message: String },
}

/// 비밀 값 저장소 한 종류.
pub trait SecretBackend {
    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError>;
    fn get(&self, key: &str) -> Result<Option<String>, SecretError>;
    /// 없는 키를 지워도 오류가 아니다.
    fn delete(&self, key: &str) -> Result<(), SecretError>;
}

/// OS 키체인(macOS Keychain, Windows 자격 증명 관리자).
#[derive(Debug, Default)]
pub struct KeyringBackend;

impl KeyringBackend {
    fn entry(key: &str) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(KEYRING_SERVICE, key).map_err(|e| SecretError::Keyring(e.to_string()))
    }
}

impl SecretBackend for KeyringBackend {
    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        Self::entry(key)?
            .set_password(secret)
            .map_err(|e| SecretError::Keyring(e.to_string()))
    }

    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        match Self::entry(key)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretError::Keyring(e.to_string())),
        }
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SecretError::Keyring(e.to_string())),
        }
    }
}

/// `data/secrets.json` 파일 보관소 (키체인 대체용).
#[derive(Debug)]
pub struct FileBackend {
    path: PathBuf,
}

impl FileBackend {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn read_map(&self) -> Result<BTreeMap<String, String>, SecretError> {
        match fs::read_to_string(&self.path) {
            Ok(text) if text.trim().is_empty() => Ok(BTreeMap::new()),
            Ok(text) => serde_json::from_str(&text).map_err(|e| SecretError::Parse {
                path: self.path.clone(),
                message: e.to_string(),
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(source) => Err(SecretError::Io {
                path: self.path.clone(),
                source,
            }),
        }
    }

    fn write_map(&self, map: &BTreeMap<String, String>) -> Result<(), SecretError> {
        let text = serde_json::to_string_pretty(map).map_err(|e| SecretError::Parse {
            path: self.path.clone(),
            message: e.to_string(),
        })?;
        write_atomic(&self.path, text.as_bytes(), true).map_err(|source| SecretError::Io {
            path: self.path.clone(),
            source,
        })
    }
}

impl SecretBackend for FileBackend {
    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        let mut map = self.read_map()?;
        map.insert(key.to_string(), secret.to_string());
        self.write_map(&map)
    }

    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        Ok(self.read_map()?.get(key).cloned())
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        let mut map = self.read_map()?;
        if map.remove(key).is_some() {
            self.write_map(&map)?;
        }
        Ok(())
    }
}

/// 실제로 저장된 위치.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredIn {
    Keychain,
    /// 키체인을 쓸 수 없어 파일로 대체함 (사용자에게 경고해야 한다)
    File,
}

/// 키체인 우선, 실패 시 파일로 대체하는 저장소.
pub struct SecretStore {
    primary: Box<dyn SecretBackend>,
    fallback: Box<dyn SecretBackend>,
}

impl SecretStore {
    pub fn new(primary: Box<dyn SecretBackend>, fallback: Box<dyn SecretBackend>) -> Self {
        Self { primary, fallback }
    }

    /// 실제 OS 키체인 + `secrets_path` 파일 대체.
    pub fn system(secrets_path: PathBuf) -> Self {
        Self::new(
            Box::new(KeyringBackend),
            Box::new(FileBackend::new(secrets_path)),
        )
    }

    /// 키체인에 저장하고 다시 읽어 확인한다. 실패하면 파일에 저장한다.
    ///
    /// 파일 쓰기까지 실패하면 오류를 돌려준다.
    pub fn set(&self, key: &str, secret: &str) -> Result<StoredIn, SecretError> {
        let primary_ok = self.primary.set(key, secret).is_ok()
            && matches!(self.primary.get(key), Ok(Some(ref v)) if v == secret);
        if primary_ok {
            // 이전에 파일로 저장된 값이 남아 있으면 제거한다(최선 노력).
            let _ = self.fallback.delete(key);
            return Ok(StoredIn::Keychain);
        }
        self.fallback.set(key, secret)?;
        let _ = self.primary.delete(key);
        Ok(StoredIn::File)
    }

    pub fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        if let Ok(Some(v)) = self.primary.get(key) {
            return Ok(Some(v));
        }
        self.fallback.get(key)
    }

    /// 두 저장소 모두에서 지운다. 둘 다 실패했을 때만 오류.
    pub fn delete(&self, key: &str) -> Result<(), SecretError> {
        let a = self.primary.delete(key);
        let b = self.fallback.delete(key);
        match (a, b) {
            (Err(_), Err(e)) => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// 테스트용 메모리 저장소. `broken`이면 모든 호출이 실패한다.
    #[derive(Default)]
    struct MemBackend {
        map: RefCell<HashMap<String, String>>,
        broken: bool,
        /// set은 성공하지만 값을 저장하지 않는 "가짜 성공" 키체인
        black_hole: bool,
    }

    impl SecretBackend for MemBackend {
        fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
            if self.broken {
                return Err(SecretError::Keyring("사용 불가".into()));
            }
            if !self.black_hole {
                self.map.borrow_mut().insert(key.into(), secret.into());
            }
            Ok(())
        }
        fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
            if self.broken {
                return Err(SecretError::Keyring("사용 불가".into()));
            }
            Ok(self.map.borrow().get(key).cloned())
        }
        fn delete(&self, key: &str) -> Result<(), SecretError> {
            if self.broken {
                return Err(SecretError::Keyring("사용 불가".into()));
            }
            self.map.borrow_mut().remove(key);
            Ok(())
        }
    }

    fn file_backend(dir: &tempfile::TempDir) -> FileBackend {
        FileBackend::new(dir.path().join("data/secrets.json"))
    }

    #[test]
    fn keychain_is_used_when_available() {
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::new(
            Box::new(MemBackend::default()),
            Box::new(file_backend(&dir)),
        );
        assert_eq!(store.set("p1", "sk-1").unwrap(), StoredIn::Keychain);
        assert_eq!(store.get("p1").unwrap().as_deref(), Some("sk-1"));
        assert!(!dir.path().join("data/secrets.json").exists());
    }

    #[test]
    fn broken_keychain_falls_back_to_file_and_reports_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::new(
            Box::new(MemBackend {
                broken: true,
                ..Default::default()
            }),
            Box::new(file_backend(&dir)),
        );
        assert_eq!(store.set("p1", "sk-1").unwrap(), StoredIn::File);
        assert_eq!(store.get("p1").unwrap().as_deref(), Some("sk-1"));
    }

    #[test]
    fn keychain_that_silently_drops_values_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::new(
            Box::new(MemBackend {
                black_hole: true,
                ..Default::default()
            }),
            Box::new(file_backend(&dir)),
        );
        assert_eq!(store.set("p1", "sk-1").unwrap(), StoredIn::File);
        assert_eq!(store.get("p1").unwrap().as_deref(), Some("sk-1"));
    }

    #[test]
    fn delete_removes_from_both_and_missing_key_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let store = SecretStore::new(
            Box::new(MemBackend {
                broken: true,
                ..Default::default()
            }),
            Box::new(file_backend(&dir)),
        );
        store.set("p1", "sk-1").unwrap();
        store.delete("p1").unwrap();
        assert_eq!(store.get("p1").unwrap(), None);
        store.delete("never-existed").unwrap();
    }

    #[test]
    fn file_backend_keeps_multiple_keys_and_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let fb = file_backend(&dir);
        fb.set("a", "1").unwrap();
        fb.set("b", "2").unwrap();
        fb.set("a", "3").unwrap();
        assert_eq!(fb.get("a").unwrap().as_deref(), Some("3"));
        assert_eq!(fb.get("b").unwrap().as_deref(), Some("2"));
        assert_eq!(fb.get("c").unwrap(), None);
    }

    #[test]
    fn corrupt_secret_file_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.json");
        fs::write(&path, "garbage").unwrap();
        let fb = FileBackend::new(path);
        assert!(matches!(fb.get("a"), Err(SecretError::Parse { .. })));
    }

    #[cfg(unix)]
    #[test]
    fn secret_file_is_private_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fb = file_backend(&dir);
        fb.set("a", "1").unwrap();
        let mode = fs::metadata(dir.path().join("data/secrets.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
