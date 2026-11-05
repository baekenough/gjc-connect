//! 지원 OS 판별, 번들 폴더 구조, Git Bash 탐색.

use std::env;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// 지원하는 운영체제.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
}

/// 지원하는 CPU 구조.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X64,
    Arm64,
}

/// 실행 대상 환경(OS + CPU).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub os: Os,
    pub arch: Arch,
}

impl Target {
    pub fn describe(&self) -> &'static str {
        match (self.os, self.arch) {
            (Os::Windows, _) => "Windows x64",
            (Os::MacOs, Arch::Arm64) => "macOS (Apple Silicon)",
            (Os::MacOs, Arch::X64) => "macOS (Intel)",
        }
    }
}

/// 지원하지 않는 환경.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlatformError {
    #[error("Windows와 Mac만 지원합니다 (현재 OS: {os})")]
    UnsupportedOs { os: String },
    #[error("이 {os}용 묶음은 {supported} CPU만 지원합니다 (현재 CPU: {arch})")]
    UnsupportedArch {
        os: &'static str,
        supported: &'static str,
        arch: String,
    },
}

/// `std::env::consts::{OS, ARCH}` 값으로 지원 여부를 판별하는 순수 함수.
pub fn detect_target(os: &str, arch: &str) -> Result<Target, PlatformError> {
    match (os, arch) {
        ("windows", "x86_64") => Ok(Target {
            os: Os::Windows,
            arch: Arch::X64,
        }),
        ("windows", other) => Err(PlatformError::UnsupportedArch {
            os: "Windows",
            supported: "x64",
            arch: other.to_string(),
        }),
        ("macos", "aarch64") => Ok(Target {
            os: Os::MacOs,
            arch: Arch::Arm64,
        }),
        ("macos", "x86_64") => Ok(Target {
            os: Os::MacOs,
            arch: Arch::X64,
        }),
        ("macos", other) => Err(PlatformError::UnsupportedArch {
            os: "Mac",
            supported: "Apple Silicon(arm64) 또는 Intel(x64)",
            arch: other.to_string(),
        }),
        (other, _) => Err(PlatformError::UnsupportedOs {
            os: other.to_string(),
        }),
    }
}

/// 현재 실행 중인 환경을 판별한다.
pub fn current_target() -> Result<Target, PlatformError> {
    detect_target(env::consts::OS, env::consts::ARCH)
}

/// 개발·테스트용으로 묶음 폴더를 강제로 지정하는 환경변수.
pub const BUNDLE_DIR_ENV: &str = "GJC_CONNECT_HOME";

/// 묶음 폴더 구조: `gjc-connect`, `bin/gjc`, `data/`, `tools/PortableGit`.
#[derive(Debug, Clone)]
pub struct Bundle {
    root: PathBuf,
    os: Os,
}

impl Bundle {
    pub fn new(root: impl Into<PathBuf>, os: Os) -> Self {
        Self {
            root: root.into(),
            os,
        }
    }

    /// 실행 파일이 있는 폴더(또는 `GJC_CONNECT_HOME`)를 묶음 폴더로 사용한다.
    pub fn locate(os: Os) -> std::io::Result<Self> {
        if let Some(dir) = env::var_os(BUNDLE_DIR_ENV).filter(|v| !v.is_empty()) {
            return Ok(Self::new(PathBuf::from(dir), os));
        }
        let exe = env::current_exe()?;
        let root = exe
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        Ok(Self::new(root, os))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn os(&self) -> Os {
        self.os
    }

    pub fn gjc_path(&self) -> PathBuf {
        let name = match self.os {
            Os::Windows => "gjc.exe",
            Os::MacOs => "gjc",
        };
        self.root.join("bin").join(name)
    }

    pub fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    /// gjc의 `GJC_CODING_AGENT_DIR`로 지정되는 격리 폴더.
    pub fn agent_dir(&self) -> PathBuf {
        self.data_dir().join("agent")
    }

    pub fn profiles_path(&self) -> PathBuf {
        self.data_dir().join("profiles.json")
    }

    /// 키체인을 쓸 수 없을 때의 API 키 보관 파일.
    pub fn secrets_path(&self) -> PathBuf {
        self.data_dir().join("secrets.json")
    }

    pub fn portable_git_dir(&self) -> PathBuf {
        self.root.join("tools").join("PortableGit")
    }

    pub fn portable_git_bash(&self) -> PathBuf {
        self.portable_git_dir().join("bin").join("bash.exe")
    }

    pub fn portable_git_cmd_dir(&self) -> PathBuf {
        self.portable_git_dir().join("cmd")
    }
}

/// Git Bash 후보 위치 입력값(환경에서 읽은 값을 주입하기 위한 구조체).
#[derive(Debug, Clone, Default)]
pub struct BashSearch {
    /// `GJC_BASH_PATH`
    pub env_override: Option<PathBuf>,
    /// 묶음에 포함된 PortableGit의 bash.exe
    pub portable: Option<PathBuf>,
    /// `%ProgramFiles%`
    pub program_files: Option<PathBuf>,
    /// `%LOCALAPPDATA%`
    pub local_app_data: Option<PathBuf>,
}

impl BashSearch {
    /// 실제 환경변수에서 값을 읽어 만든다.
    pub fn from_env(bundle: &Bundle) -> Self {
        let get = |k: &str| env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        Self {
            env_override: get("GJC_BASH_PATH"),
            portable: Some(bundle.portable_git_bash()),
            program_files: get("ProgramFiles"),
            local_app_data: get("LOCALAPPDATA"),
        }
    }
}

/// Git Bash 탐색 순서: `GJC_BASH_PATH` → 묶음 PortableGit → 시스템 Git → 사용자 Git.
///
/// `exists`를 주입받아 파일 시스템 없이도 테스트할 수 있다.
pub fn find_bash(search: &BashSearch, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let git_bash = |base: &Option<PathBuf>, parts: &[&str]| -> Option<PathBuf> {
        base.as_ref().map(|b| {
            let mut p = b.clone();
            for part in parts {
                p.push(part);
            }
            p.push("bin");
            p.push("bash.exe");
            p
        })
    };
    let candidates = [
        search.env_override.clone(),
        search.portable.clone(),
        git_bash(&search.program_files, &["Git"]),
        git_bash(&search.local_app_data, &["Programs", "Git"]),
    ];
    candidates.into_iter().flatten().find(|p| exists(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_targets_are_detected() {
        let win = detect_target("windows", "x86_64").unwrap();
        assert_eq!(win.os, Os::Windows);
        let m1 = detect_target("macos", "aarch64").unwrap();
        assert_eq!((m1.os, m1.arch), (Os::MacOs, Arch::Arm64));
        let intel = detect_target("macos", "x86_64").unwrap();
        assert_eq!((intel.os, intel.arch), (Os::MacOs, Arch::X64));
    }

    #[test]
    fn linux_is_rejected_with_korean_message() {
        let err = detect_target("linux", "x86_64").unwrap_err();
        assert!(matches!(err, PlatformError::UnsupportedOs { .. }));
        assert!(err.to_string().contains("Windows와 Mac만 지원합니다"));
    }

    #[test]
    fn unsupported_cpu_is_rejected() {
        let err = detect_target("windows", "aarch64").unwrap_err();
        assert!(matches!(err, PlatformError::UnsupportedArch { .. }));
        assert!(err.to_string().contains("x64"));
        assert!(detect_target("macos", "riscv64").is_err());
        assert!(detect_target("freebsd", "x86_64").is_err());
    }

    #[test]
    fn bundle_paths_follow_layout() {
        let win = Bundle::new("C:/Tools/gjc", Os::Windows);
        assert!(win.gjc_path().ends_with("bin/gjc.exe"));
        let mac = Bundle::new("/opt/gjc", Os::MacOs);
        assert!(mac.gjc_path().ends_with("bin/gjc"));
        assert!(mac.agent_dir().ends_with("data/agent"));
        assert!(mac.profiles_path().ends_with("data/profiles.json"));
        assert!(win
            .portable_git_bash()
            .ends_with("tools/PortableGit/bin/bash.exe"));
    }

    fn full_search() -> BashSearch {
        BashSearch {
            env_override: Some(PathBuf::from("/env/bash.exe")),
            portable: Some(PathBuf::from("/bundle/PortableGit/bin/bash.exe")),
            program_files: Some(PathBuf::from("/pf")),
            local_app_data: Some(PathBuf::from("/lad")),
        }
    }

    #[test]
    fn bash_search_prefers_env_override_then_portable() {
        let s = full_search();
        assert_eq!(
            find_bash(&s, |_| true).unwrap(),
            PathBuf::from("/env/bash.exe")
        );
        let portable = PathBuf::from("/bundle/PortableGit/bin/bash.exe");
        assert_eq!(find_bash(&s, |p| p == portable).unwrap(), portable);
    }

    #[test]
    fn bash_search_falls_back_to_system_then_user_git() {
        let s = full_search();
        let system = PathBuf::from("/pf")
            .join("Git")
            .join("bin")
            .join("bash.exe");
        assert_eq!(find_bash(&s, |p| p == system).unwrap(), system);
        let user = PathBuf::from("/lad")
            .join("Programs")
            .join("Git")
            .join("bin")
            .join("bash.exe");
        assert_eq!(find_bash(&s, |p| p == user).unwrap(), user);
    }

    #[test]
    fn bash_search_returns_none_when_nothing_exists() {
        assert!(find_bash(&full_search(), |_| false).is_none());
        assert!(find_bash(&BashSearch::default(), |_| true).is_none());
    }
}
