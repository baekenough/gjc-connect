//! gjc 실행: 격리 환경변수, 고정 플래그, Git Bash 경로 설정.
//!
//! 기존 Windows 런처(`gjc.cmd`)의 동작을 OS 공통으로 재현한다.
//! 핵심은 `GJC_CODING_AGENT_DIR`을 `<묶음>/data/agent`로 지정해
//! 사용자의 `~/.gjc/agent` 설정을 전혀 쓰지 않는 것이다.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::platform::{Bundle, Os};

/// gjc에 항상 넘기는 플래그 (도구 6종만 허용, MCP·LSP·PTY·제목 생성 끔).
pub const GJC_FIXED_ARGS: [&str; 6] = [
    "--tools",
    "read,edit,write,bash,search,find",
    "--no-mcp",
    "--no-lsp",
    "--no-pty",
    "--no-title",
];

/// 홈 폴더 아래 보조 상태(로그 등)에 쓰는 폴더 이름. 기본값 `.gjc`와 분리한다.
pub const GJC_CONFIG_DIRNAME: &str = ".gjc-connect";

/// gjc 격리·보호용 환경변수 목록.
pub fn isolated_env(bundle: &Bundle) -> Vec<(String, OsString)> {
    let text = |k: &str, v: &str| (k.to_string(), OsString::from(v));
    vec![
        (
            "GJC_CODING_AGENT_DIR".to_string(),
            bundle.agent_dir().into_os_string(),
        ),
        text("GJC_CONFIG_DIR", GJC_CONFIG_DIRNAME),
        text("GJC_MODEL_PRESET_REGISTRY_DISABLED", "1"),
        text("GJC_DISABLE_TELEMETRY", "1"),
        text("GJC_NO_TMUX", "1"),
        text("GJC_LAUNCH_POLICY", "direct"),
        text("GJC_NO_PTY", "1"),
        text("GJC_UI_LANGUAGE", "ko"),
    ]
}

/// 실행할 명령 명세. (API 키가 들어 있으므로 `Debug`를 구현하지 않는다.)
pub struct LaunchSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub envs: Vec<(String, OsString)>,
    pub cwd: PathBuf,
    /// `PATH` 앞에 추가할 폴더 (Windows PortableGit의 `cmd`)
    pub path_prepend: Vec<PathBuf>,
}

/// `run` 명령용 실행 명세를 만든다.
///
/// `api_key`가 있으면 프로필이 가리키는 환경변수 이름으로 주입한다.
pub fn build_launch(
    bundle: &Bundle,
    api_key_env: Option<(&str, &str)>,
    project_dir: &Path,
) -> LaunchSpec {
    let mut envs = isolated_env(bundle);
    if let Some((name, value)) = api_key_env {
        envs.push((name.to_string(), OsString::from(value)));
    }
    let mut path_prepend = Vec::new();
    if bundle.os() == Os::Windows {
        let cmd = bundle.portable_git_cmd_dir();
        if cmd.join("git.exe").exists() {
            path_prepend.push(cmd);
        }
    }
    LaunchSpec {
        program: bundle.gjc_path(),
        args: GJC_FIXED_ARGS.iter().map(OsString::from).collect(),
        envs,
        cwd: project_dir.to_path_buf(),
        path_prepend,
    }
}

fn apply_env(cmd: &mut Command, envs: &[(String, OsString)], path_prepend: &[PathBuf]) {
    for (k, v) in envs {
        cmd.env(k, v);
    }
    if !path_prepend.is_empty() {
        let mut paths: Vec<PathBuf> = path_prepend.to_vec();
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        if let Ok(joined) = std::env::join_paths(paths) {
            cmd.env("PATH", joined);
        }
    }
}

/// gjc를 실행하고 종료 코드를 돌려준다. 표준 입출력은 그대로 연결한다.
pub fn launch(spec: &LaunchSpec) -> Result<i32> {
    // Ctrl+C는 자식(gjc)이 처리한다. 부모가 먼저 죽어 터미널이 어지러워지지 않도록 무시한다.
    let _ = ctrlc::set_handler(|| {});

    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args).current_dir(&spec.cwd);
    apply_env(&mut cmd, &spec.envs, &spec.path_prepend);
    let status = cmd
        .status()
        .with_context(|| format!("gjc를 실행할 수 없습니다: {}", spec.program.display()))?;
    Ok(status.code().unwrap_or(1))
}

/// 제한 시간 안에 명령을 실행하고 표준 출력을 돌려준다. 시간이 넘으면 프로세스를 종료한다.
fn run_captured(mut cmd: Command, timeout: Duration) -> Result<(bool, String)> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().context("프로그램을 시작할 수 없습니다")?;
    let started = Instant::now();
    loop {
        match child
            .try_wait()
            .context("프로그램 상태를 확인할 수 없습니다")?
        {
            Some(_) => break,
            None if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("{}초 안에 끝나지 않아 중단했습니다", timeout.as_secs());
            }
            None => thread::sleep(Duration::from_millis(50)),
        }
    }
    let output = child
        .wait_with_output()
        .context("프로그램 출력을 읽을 수 없습니다")?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((output.status.success(), text))
}

/// gjc를 격리 환경으로 실행하는 보조 명령을 만든다 (홈의 `.gjc` 설정을 쓰지 않는다).
fn gjc_command(bundle: &Bundle) -> Command {
    let mut cmd = Command::new(bundle.gjc_path());
    apply_env(&mut cmd, &isolated_env(bundle), &[]);
    cmd
}

/// `gjc --version` 출력 (예: `gjc/0.19.3`).
pub fn gjc_version(bundle: &Bundle) -> Result<String> {
    let mut cmd = gjc_command(bundle);
    cmd.arg("--version");
    let (ok, out) = run_captured(cmd, Duration::from_secs(20))?;
    if !ok || out.is_empty() {
        bail!("gjc 버전을 확인할 수 없습니다");
    }
    Ok(out)
}

/// `gjc config set shellPath <bash>` (Windows에서 Git Bash 경로를 알려 준다).
pub fn set_shell_path(bundle: &Bundle, bash: &Path) -> Result<()> {
    let mut cmd = gjc_command(bundle);
    cmd.args(["config", "set", "shellPath"]).arg(bash);
    let (ok, _) = run_captured(cmd, Duration::from_secs(20))?;
    if !ok {
        bail!("gjc에 Git Bash 경로를 설정하지 못했습니다");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_value(envs: &[(String, OsString)], key: &str) -> Option<OsString> {
        envs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    #[test]
    fn isolated_env_points_agent_dir_into_bundle_and_disables_phone_home() {
        let bundle = Bundle::new("/opt/gjc", Os::MacOs);
        let envs = isolated_env(&bundle);
        assert_eq!(
            env_value(&envs, "GJC_CODING_AGENT_DIR").unwrap(),
            Path::new("/opt/gjc")
                .join("data")
                .join("agent")
                .into_os_string()
        );
        for (k, v) in [
            ("GJC_DISABLE_TELEMETRY", "1"),
            ("GJC_MODEL_PRESET_REGISTRY_DISABLED", "1"),
            ("GJC_NO_TMUX", "1"),
            ("GJC_LAUNCH_POLICY", "direct"),
            ("GJC_NO_PTY", "1"),
            ("GJC_UI_LANGUAGE", "ko"),
        ] {
            assert_eq!(env_value(&envs, k).unwrap(), OsString::from(v), "{k}");
        }
        assert_ne!(
            env_value(&envs, "GJC_CONFIG_DIR").unwrap(),
            OsString::from(".gjc")
        );
    }

    #[test]
    fn launch_spec_has_fixed_flags_cwd_and_api_key_env() {
        let bundle = Bundle::new("/opt/gjc", Os::MacOs);
        let spec = build_launch(
            &bundle,
            Some(("MY_KEY_ENV", "sk-123")),
            Path::new("/work/proj"),
        );
        assert_eq!(spec.program, Path::new("/opt/gjc").join("bin").join("gjc"));
        assert_eq!(spec.cwd, PathBuf::from("/work/proj"));
        let args: Vec<String> = spec
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args.join(" "),
            "--tools read,edit,write,bash,search,find --no-mcp --no-lsp --no-pty --no-title"
        );
        assert_eq!(
            env_value(&spec.envs, "MY_KEY_ENV").unwrap(),
            OsString::from("sk-123")
        );
        assert!(spec.path_prepend.is_empty());
    }

    #[test]
    fn no_api_key_means_no_extra_env() {
        let bundle = Bundle::new("/opt/gjc", Os::MacOs);
        let with = build_launch(&bundle, None, Path::new("/p"));
        assert_eq!(with.envs.len(), isolated_env(&bundle).len());
    }

    #[test]
    fn portable_git_cmd_dir_is_prepended_only_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = Bundle::new(dir.path(), Os::Windows);
        assert!(build_launch(&bundle, None, Path::new("C:/p"))
            .path_prepend
            .is_empty());
        std::fs::create_dir_all(bundle.portable_git_cmd_dir()).unwrap();
        std::fs::write(bundle.portable_git_cmd_dir().join("git.exe"), b"").unwrap();
        let spec = build_launch(&bundle, None, Path::new("C:/p"));
        assert_eq!(spec.path_prepend, vec![bundle.portable_git_cmd_dir()]);
    }

    #[cfg(unix)]
    mod with_stub {
        use super::*;
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        fn bundle_with_stub(script: &str) -> (tempfile::TempDir, Bundle) {
            let dir = tempfile::tempdir().unwrap();
            let bundle = Bundle::new(dir.path(), Os::MacOs);
            fs::create_dir_all(dir.path().join("bin")).unwrap();
            let path = bundle.gjc_path();
            fs::write(&path, script).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            (dir, bundle)
        }

        #[test]
        fn version_runs_stub_with_isolated_env() {
            let (_dir, bundle) = bundle_with_stub(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"gjc/9.9.9 agent=$GJC_CODING_AGENT_DIR\"; fi\n",
            );
            let out = gjc_version(&bundle).unwrap();
            assert!(out.starts_with("gjc/9.9.9"));
            assert!(out.contains("data/agent"));
        }

        #[test]
        fn failing_or_hanging_gjc_is_reported_not_panicked() {
            let (_d1, failing) = bundle_with_stub("#!/bin/sh\nexit 3\n");
            assert!(gjc_version(&failing).is_err());
            let missing = Bundle::new("/nonexistent/bundle", Os::MacOs);
            assert!(gjc_version(&missing).is_err());

            let (_d2, hanging) = bundle_with_stub("#!/bin/sh\nsleep 5\n");
            let mut cmd = gjc_command(&hanging);
            cmd.arg("--version");
            let err = run_captured(cmd, Duration::from_millis(200)).unwrap_err();
            assert!(err.to_string().contains("중단"));
        }

        #[test]
        fn launch_passes_args_env_and_exit_code() {
            let (dir, bundle) = bundle_with_stub(
                "#!/bin/sh\necho \"$@\" > \"$PWD/args.txt\"\necho \"$MY_KEY_ENV|$GJC_NO_PTY\" > \"$PWD/env.txt\"\nexit 7\n",
            );
            let work = dir.path().join("work");
            fs::create_dir_all(&work).unwrap();
            let spec = build_launch(&bundle, Some(("MY_KEY_ENV", "sk-xyz")), &work);
            let code = launch(&spec).unwrap();
            assert_eq!(code, 7);
            let args = fs::read_to_string(work.join("args.txt")).unwrap();
            assert!(args.contains("--no-mcp"));
            assert_eq!(
                fs::read_to_string(work.join("env.txt")).unwrap().trim(),
                "sk-xyz|1"
            );
        }
    }
}
