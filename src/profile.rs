//! 서버·모델 설정(프로필)과 저장소.
//!
//! 프로필 목록은 `data/profiles.json`에 JSON으로 저장한다. API 키는 여기에 저장하지 않는다
//! (`secrets` 모듈 참고).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::atomic::write_atomic;

/// 접속 방식.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessKind {
    /// 모델 서버(vLLM 등)에 직접 접속
    Direct,
    /// 사내 게이트웨이(llm-ops)를 거쳐 접속
    Gateway,
}

impl AccessKind {
    pub fn label(self) -> &'static str {
        match self {
            AccessKind::Direct => "직접 접속",
            AccessKind::Gateway => "게이트웨이",
        }
    }
}

/// 인증 방식.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    None,
    ApiKey,
}

/// 저장되는 서버·모델 설정 한 건.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub access: AccessKind,
    /// `/v1`까지 포함한 정규화된 주소
    pub base_url: String,
    pub model: String,
    pub auth: AuthKind,
    /// 서버가 알려 준 `max_model_len` (모르면 `None`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

impl Profile {
    /// gjc `models.yml`의 provider 이름. 이름의 공백·특수문자는 `-`로 바꾼다.
    pub fn provider_id(&self) -> String {
        provider_id(&self.name)
    }

    /// gjc의 `/model` 선택자 형식 (`<provider>/<model>`).
    pub fn selector(&self) -> String {
        format!("{}/{}", self.provider_id(), self.model)
    }

    /// `run`이 API 키를 주입할 환경변수 이름. 인증이 없으면 `None`.
    pub fn api_key_env(&self) -> Option<String> {
        match self.auth {
            AuthKind::None => None,
            AuthKind::ApiKey => Some(api_key_env_name(&self.name)),
        }
    }
}

/// 이름에서 provider id를 만든다. 유니코드 글자·숫자와 `-_.`만 유지한다.
pub fn provider_id(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for c in name.trim().chars() {
        if c.is_alphanumeric() || c == '_' || c == '.' {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "profile".to_string()
    } else {
        trimmed.to_string()
    }
}

fn fnv1a_32(s: &str) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// 프로필 이름에서 결정적으로 환경변수 이름을 만든다.
///
/// ASCII가 아닌 이름(한글 등)도 충돌하지 않도록 이름 해시를 뒤에 붙인다.
pub fn api_key_env_name(name: &str) -> String {
    let mut slug = String::new();
    let mut last_underscore = true;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_uppercase());
            last_underscore = false;
        } else if !last_underscore {
            slug.push('_');
            last_underscore = true;
        }
    }
    let slug = slug.trim_matches('_');
    let hash = fnv1a_32(name);
    if slug.is_empty() {
        format!("GJC_CONNECT_KEY_{hash:08X}")
    } else {
        format!("GJC_CONNECT_KEY_{slug}_{hash:08X}")
    }
}

/// 설정 이름 검증 오류.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NameError {
    #[error("설정 이름을 입력해 주세요")]
    Empty,
    #[error("설정 이름은 40자 이하로 입력해 주세요")]
    TooLong,
    #[error("설정 이름에는 '/'를 사용할 수 없습니다")]
    HasSlash,
    #[error("설정 이름에 제어 문자를 사용할 수 없습니다")]
    HasControl,
}

/// 앞뒤 공백을 제거하고 이름을 검증한다.
pub fn validate_name(name: &str) -> Result<String, NameError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.chars().count() > 40 {
        return Err(NameError::TooLong);
    }
    if name.contains('/') || name.contains('\\') {
        return Err(NameError::HasSlash);
    }
    if name.chars().any(char::is_control) {
        return Err(NameError::HasControl);
    }
    Ok(name.to_string())
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("설정 파일을 읽을 수 없습니다 ({path}): {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("설정 파일이 손상되었습니다 ({path}): {message}. 파일을 백업한 뒤 삭제하면 새로 만들 수 있습니다")]
    Corrupt { path: PathBuf, message: String },
    #[error("설정 파일을 저장할 수 없습니다 ({path}): {source}")]
    Write { path: PathBuf, source: io::Error },
    #[error("'{0}' 설정이 없습니다")]
    NotFound(String),
    #[error("'{new}' 이름은 기존 설정 '{existing}'과(와) 내부 이름이 겹칩니다. 다른 이름을 사용해 주세요")]
    ProviderConflict { new: String, existing: String },
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreData {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    current: Option<String>,
    #[serde(default)]
    profiles: Vec<Profile>,
}

fn default_version() -> u32 {
    1
}

/// 프로필 저장소. 변경 메서드는 즉시 디스크에 원자적으로 저장한다.
#[derive(Debug)]
pub struct ProfileStore {
    path: PathBuf,
    data: StoreData,
}

impl ProfileStore {
    /// 파일이 없으면 빈 저장소를 만든다(파일은 첫 변경 때 생성).
    pub fn load(path: &Path) -> Result<Self, StoreError> {
        let data = match fs::read_to_string(path) {
            Ok(text) => {
                serde_json::from_str::<StoreData>(&text).map_err(|e| StoreError::Corrupt {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                })?
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => StoreData {
                version: 1,
                ..StoreData::default()
            },
            Err(source) => {
                return Err(StoreError::Read {
                    path: path.to_path_buf(),
                    source,
                })
            }
        };
        Ok(Self {
            path: path.to_path_buf(),
            data,
        })
    }

    fn save(&self) -> Result<(), StoreError> {
        let text = serde_json::to_string_pretty(&self.data).map_err(|e| StoreError::Corrupt {
            path: self.path.clone(),
            message: e.to_string(),
        })?;
        write_atomic(&self.path, text.as_bytes(), false).map_err(|source| StoreError::Write {
            path: self.path.clone(),
            source,
        })
    }

    pub fn profiles(&self) -> &[Profile] {
        &self.data.profiles
    }

    pub fn is_empty(&self) -> bool {
        self.data.profiles.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.data.profiles.iter().find(|p| p.name == name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub fn current_name(&self) -> Option<&str> {
        self.data.current.as_deref()
    }

    pub fn current(&self) -> Option<&Profile> {
        self.current_name().and_then(|n| self.get(n))
    }

    pub fn is_current(&self, name: &str) -> bool {
        self.current_name() == Some(name)
    }

    /// 추가하거나 같은 이름이면 덮어쓴다. 덮어썼으면 `true`.
    ///
    /// 덮어써도 현재 설정 지정은 바뀌지 않는다.
    pub fn upsert(&mut self, profile: Profile) -> Result<bool, StoreError> {
        let pid = profile.provider_id();
        if let Some(other) = self
            .data
            .profiles
            .iter()
            .find(|p| p.name != profile.name && p.provider_id() == pid)
        {
            return Err(StoreError::ProviderConflict {
                new: profile.name,
                existing: other.name.clone(),
            });
        }
        let replaced = match self
            .data
            .profiles
            .iter_mut()
            .find(|p| p.name == profile.name)
        {
            Some(slot) => {
                *slot = profile;
                true
            }
            None => {
                self.data.profiles.push(profile);
                false
            }
        };
        self.save()?;
        Ok(replaced)
    }

    /// 삭제하고 삭제된 프로필을 돌려준다. 현재 설정이었다면 현재 지정도 해제된다.
    pub fn remove(&mut self, name: &str) -> Result<Profile, StoreError> {
        let idx = self
            .data
            .profiles
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| StoreError::NotFound(name.to_string()))?;
        let removed = self.data.profiles.remove(idx);
        if self.data.current.as_deref() == Some(name) {
            self.data.current = None;
        }
        self.save()?;
        Ok(removed)
    }

    pub fn set_current(&mut self, name: &str) -> Result<(), StoreError> {
        if !self.contains(name) {
            return Err(StoreError::NotFound(name.to_string()));
        }
        self.data.current = Some(name.to_string());
        self.save()
    }

    /// 연결 확인에서 알게 된 컨텍스트 길이를 기록한다. 바뀐 경우에만 `true`.
    pub fn update_context_window(
        &mut self,
        name: &str,
        window: Option<u64>,
    ) -> Result<bool, StoreError> {
        let profile = self
            .data
            .profiles
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| StoreError::NotFound(name.to_string()))?;
        if window.is_none() || profile.context_window == window {
            return Ok(false);
        }
        profile.context_window = window;
        self.save()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample(name: &str, auth: AuthKind) -> Profile {
        Profile {
            name: name.to_string(),
            access: AccessKind::Direct,
            base_url: "http://10.0.0.5:8000/v1".to_string(),
            model: "qwen3-coder".to_string(),
            auth,
            context_window: None,
        }
    }

    fn store_in(dir: &tempfile::TempDir) -> ProfileStore {
        ProfileStore::load(&dir.path().join("data/profiles.json")).unwrap()
    }

    #[test]
    fn add_set_current_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store_in(&dir);
        assert!(store.is_empty());
        assert!(!store.upsert(sample("dgx", AuthKind::None)).unwrap());
        store.set_current("dgx").unwrap();

        let reloaded = store_in(&dir);
        assert_eq!(reloaded.profiles().len(), 1);
        assert_eq!(reloaded.current().unwrap().name, "dgx");
    }

    #[test]
    fn overwrite_keeps_current_and_replaces_fields() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store_in(&dir);
        store.upsert(sample("dgx", AuthKind::None)).unwrap();
        store.set_current("dgx").unwrap();

        let mut changed = sample("dgx", AuthKind::ApiKey);
        changed.model = "other".into();
        assert!(store.upsert(changed).unwrap());
        assert_eq!(store.profiles().len(), 1);
        assert_eq!(store.current().unwrap().model, "other");
        assert_eq!(store.current().unwrap().auth, AuthKind::ApiKey);
    }

    #[test]
    fn remove_clears_current_and_unknown_name_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store_in(&dir);
        store.upsert(sample("a", AuthKind::None)).unwrap();
        store.upsert(sample("b", AuthKind::None)).unwrap();
        store.set_current("a").unwrap();

        store.remove("b").unwrap();
        assert_eq!(store.current_name(), Some("a"));
        store.remove("a").unwrap();
        assert!(store.current().is_none());
        assert!(matches!(store.remove("a"), Err(StoreError::NotFound(_))));
        assert!(matches!(
            store.set_current("zzz"),
            Err(StoreError::NotFound(_))
        ));
    }

    #[test]
    fn corrupt_file_reports_korean_error_instead_of_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        fs::write(&path, "{ not json").unwrap();
        let err = ProfileStore::load(&path).unwrap_err();
        assert!(matches!(err, StoreError::Corrupt { .. }));
        assert!(err.to_string().contains("손상"));
    }

    #[test]
    fn provider_id_collisions_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store_in(&dir);
        store.upsert(sample("my server", AuthKind::None)).unwrap();
        let err = store
            .upsert(sample("my-server", AuthKind::None))
            .unwrap_err();
        assert!(matches!(err, StoreError::ProviderConflict { .. }));
    }

    #[test]
    fn context_window_update_reports_change_only_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = store_in(&dir);
        store.upsert(sample("a", AuthKind::None)).unwrap();
        assert!(store.update_context_window("a", Some(32768)).unwrap());
        assert!(!store.update_context_window("a", Some(32768)).unwrap());
        assert!(!store.update_context_window("a", None).unwrap());
        assert_eq!(store.get("a").unwrap().context_window, Some(32768));
    }

    #[test]
    fn names_are_validated() {
        assert_eq!(validate_name("  사내 Qwen "), Ok("사내 Qwen".to_string()));
        assert_eq!(validate_name("  "), Err(NameError::Empty));
        assert_eq!(validate_name("a/b"), Err(NameError::HasSlash));
        assert_eq!(validate_name(&"x".repeat(41)), Err(NameError::TooLong));
        assert_eq!(validate_name("a\nb"), Err(NameError::HasControl));
    }

    #[test]
    fn provider_id_and_env_names_are_stable_and_distinct() {
        assert_eq!(provider_id("My Server!"), "My-Server");
        assert_eq!(provider_id("사내 Qwen"), "사내-Qwen");
        assert_eq!(provider_id("///"), "profile");

        let a = api_key_env_name("dgx-1");
        assert!(a.starts_with("GJC_CONNECT_KEY_DGX_1_"));
        assert_eq!(a, api_key_env_name("dgx-1"));
        // 한글 이름도 서로 다른 변수로 구분된다.
        assert_ne!(api_key_env_name("사내"), api_key_env_name("외부"));
        assert!(api_key_env_name("사내").starts_with("GJC_CONNECT_KEY_"));
        // 환경변수로 쓸 수 있는 문자만 포함한다.
        assert!(api_key_env_name("a b/c-사내")
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_'));
    }

    #[test]
    fn selector_and_api_key_env() {
        let p = sample("dgx", AuthKind::ApiKey);
        assert_eq!(p.selector(), "dgx/qwen3-coder");
        assert!(p.api_key_env().is_some());
        assert!(sample("dgx", AuthKind::None).api_key_env().is_none());
    }
}
