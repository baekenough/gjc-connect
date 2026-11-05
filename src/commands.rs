//! 명령별 흐름: 첫 실행, run, switch, add, list, remove, test.
//!
//! 모든 함수는 프로세스 종료 코드(`i32`)를 돌려준다.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

use crate::check::{
    self, check_connection, fetch_model_listing, format_failure, format_success, CheckFailure,
    CheckRequest, CheckSuccess, ModelListing,
};
use crate::genconfig::write_agent_config;
use crate::launcher::{build_launch, gjc_version, launch, set_shell_path};
use crate::platform::{find_bash, BashSearch, Bundle, Os, Target};
use crate::profile::{provider_id, validate_name, AccessKind, AuthKind, Profile, ProfileStore};
use crate::secrets::{SecretStore, StoredIn};
use crate::ui;
use crate::urlnorm::normalize_base_url;

/// 명령 실행에 필요한 상태 묶음.
pub struct Ctx {
    pub target: Target,
    pub bundle: Bundle,
    pub secrets: SecretStore,
    pub store: ProfileStore,
    pub timeout: Duration,
}

impl Ctx {
    pub fn open(target: Target, bundle: Bundle, secrets: SecretStore) -> Result<Self> {
        let store = ProfileStore::load(&bundle.profiles_path())?;
        Ok(Self {
            target,
            bundle,
            secrets,
            store,
            timeout: check::timeout_from_env(),
        })
    }

    /// 저장된 프로필로 gjc `models.yml`/`config.yml`을 다시 만든다.
    fn sync_config(&self) -> Result<()> {
        write_agent_config(
            &self.bundle,
            self.store.profiles(),
            self.store.current_name(),
        )
        .context("gjc 설정 파일을 만들지 못했습니다")
    }

    /// 프로필에 필요한 API 키를 꺼낸다. 인증이 없으면 `None`.
    fn secret_for(&self, profile: &Profile) -> Result<Option<String>> {
        match profile.auth {
            AuthKind::None => Ok(None),
            AuthKind::ApiKey => self
                .secrets
                .get(&profile.name)
                .context("API 키를 읽지 못했습니다")?
                .map(Some)
                .ok_or_else(|| {
                    anyhow!(
                        "'{}' 설정의 API 키를 찾을 수 없습니다. `gjc-connect add`에서 같은 이름으로 다시 등록해 주세요",
                        profile.name
                    )
                }),
        }
    }

    fn check_profile(
        &self,
        profile: &Profile,
        key: Option<&str>,
    ) -> Result<CheckSuccess, CheckFailure> {
        run_check(&profile.base_url, &profile.model, key, self.timeout)
    }
}

fn run_check(
    base_url: &str,
    model: &str,
    key: Option<&str>,
    timeout: Duration,
) -> Result<CheckSuccess, CheckFailure> {
    println!("연결을 확인하는 중입니다... (최대 {}초)", timeout.as_secs());
    check_connection(&CheckRequest {
        base_url,
        model,
        api_key: key,
        timeout,
    })
}

fn quote_if_needed(name: &str) -> String {
    if name.chars().any(char::is_whitespace) {
        format!("\"{name}\"")
    } else {
        name.to_string()
    }
}

/// 메뉴·목록에 보여 줄 한 줄 설명.
pub fn profile_label(profile: &Profile, is_current: bool) -> String {
    let mut label = format!(
        "{} — {} ({})",
        profile.name,
        profile.model,
        profile.access.label()
    );
    if is_current {
        label.push_str(" (현재 사용 중)");
    }
    label
}

// ---------------------------------------------------------------------------
// 환경 확인
// ---------------------------------------------------------------------------

fn find_windows_bash(bundle: &Bundle) -> Option<PathBuf> {
    find_bash(&BashSearch::from_env(bundle), |p| p.is_file())
}

fn ensure_gjc_present(bundle: &Bundle) -> Result<()> {
    let path = bundle.gjc_path();
    if path.is_file() {
        Ok(())
    } else {
        bail!(
            "gjc 실행 파일을 찾을 수 없습니다: {}\n  묶음을 다시 풀어 bin 폴더가 gjc-connect와 같은 위치에 있는지 확인하세요",
            path.display()
        )
    }
}

fn environment_check(ctx: &Ctx) -> Result<()> {
    println!("  운영체제: {} — 지원됨", ctx.target.describe());
    ensure_gjc_present(&ctx.bundle)?;
    match gjc_version(&ctx.bundle) {
        Ok(v) => println!("  gjc: {v}"),
        Err(e) => bail!("gjc를 실행할 수 없습니다: {e:#}\n  묶음이 손상되었거나 이 PC의 CPU와 맞지 않을 수 있습니다"),
    }
    if ctx.bundle.os() == Os::Windows {
        match find_windows_bash(&ctx.bundle) {
            Some(p) => println!("  Git Bash: {}", p.display()),
            None => println!(
                "  경고: Git Bash를 찾지 못했습니다. 실행 전에 {} 에 PortableGit을 풀거나 GJC_BASH_PATH를 지정하세요",
                ctx.bundle.portable_git_dir().display()
            ),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

fn strip_quotes(s: &str) -> &str {
    s.trim().trim_matches('"').trim_matches('\'')
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// 절대 경로로 바꾼다. 유닉스에서는 `..`와 심볼릭 링크도 정리한다.
/// Windows의 `canonicalize`는 `\\?\` 접두사를 붙여 일부 도구와 충돌하므로 쓰지 않는다.
fn absolute_dir(path: &Path) -> std::io::Result<PathBuf> {
    if cfg!(unix) {
        fs::canonicalize(path)
    } else {
        std::path::absolute(path)
    }
}

fn validate_dir(raw: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(strip_quotes(raw));
    if !path.is_dir() {
        return Err(format!("폴더를 찾을 수 없습니다: {}", path.display()));
    }
    absolute_dir(&path).map_err(|e| format!("경로를 확인할 수 없습니다: {e}"))
}

/// 작업 폴더 결정: 인자 → 현재 폴더. 설치 폴더 자체이면 프로젝트 폴더를 묻는다.
fn resolve_project_dir(ctx: &Ctx, arg: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = arg {
        return validate_dir(&p.to_string_lossy()).map_err(|m| anyhow!(m));
    }
    let cwd = std::env::current_dir().context("현재 폴더를 확인할 수 없습니다")?;
    if !same_dir(&cwd, ctx.bundle.root()) {
        return Ok(cwd);
    }
    println!("현재 폴더가 gjc-connect 설치 폴더입니다. 설치 폴더를 작업 폴더로 쓰면 안 됩니다.");
    ui::ensure_interactive().map_err(|_| {
        anyhow!("작업할 프로젝트 폴더를 지정하세요: gjc-connect run <프로젝트 폴더>")
    })?;
    let raw = ui::input("작업할 프로젝트 폴더 경로", None, |s| {
        validate_dir(s).map(|_| ())
    })?;
    validate_dir(&raw).map_err(|m| anyhow!(m))
}

fn no_current_message() {
    eprintln!("현재 사용 중인 서버·모델 설정이 없습니다.");
    eprintln!(
        "  `gjc-connect`로 처음 설정하거나, `gjc-connect switch`로 저장된 설정을 선택하세요."
    );
}

/// 현재 프로필로 gjc를 실행한다.
pub fn run(ctx: &mut Ctx, project_dir: Option<&Path>) -> Result<i32> {
    let Some(profile) = ctx.store.current().cloned() else {
        no_current_message();
        return Ok(1);
    };
    ensure_gjc_present(&ctx.bundle)?;
    let dir = resolve_project_dir(ctx, project_dir)?;
    let key = ctx.secret_for(&profile)?;

    ctx.sync_config()?;
    if ctx.bundle.os() == Os::Windows {
        let Some(bash) = find_windows_bash(&ctx.bundle) else {
            eprintln!("Git Bash를 찾을 수 없습니다.");
            eprintln!(
                "  tools\\PortableGit 폴더에 PortableGit을 풀거나(예: {}), GJC_BASH_PATH 환경변수에 bash.exe 경로를 지정하세요.",
                ctx.bundle.portable_git_bash().display()
            );
            return Ok(1);
        };
        set_shell_path(&ctx.bundle, &bash)?;
    }

    let api_key_env = profile.api_key_env();
    let spec = build_launch(
        &ctx.bundle,
        api_key_env.as_deref().zip(key.as_deref()),
        &dir,
    );
    println!(
        "gjc를 시작합니다 — 설정: {} / 모델: {} / 작업 폴더: {}",
        profile.name,
        profile.model,
        dir.display()
    );
    launch(&spec)
}

// ---------------------------------------------------------------------------
// add
// ---------------------------------------------------------------------------

struct Draft {
    name: String,
    access: AccessKind,
    base_url: String,
    model: String,
    api_key: Option<String>,
}

/// `add` 흐름의 결과.
pub enum AddOutcome {
    /// 저장함. `made_current`는 현재 설정으로 지정했는지 여부.
    Saved { name: String, made_current: bool },
    /// 저장하지 않고 끝냄.
    Discarded,
}

fn name_conflicts_with_other(ctx: &Ctx, name: &str) -> bool {
    let pid = provider_id(name);
    ctx.store
        .profiles()
        .iter()
        .any(|p| p.name != name && p.provider_id() == pid)
}

fn prompt_name(ctx: &Ctx, prev: Option<&Draft>) -> Result<String> {
    loop {
        let raw = ui::input(
            "설정 이름 (예: 사내-qwen)",
            prev.map(|d| d.name.as_str()),
            |s| validate_name(s).map(|_| ()).map_err(|e| e.to_string()),
        )?;
        let name = validate_name(&raw)?;
        if name_conflicts_with_other(ctx, &name) {
            println!("이미 있는 설정과 내부 이름이 겹칩니다. 다른 이름을 입력해 주세요.");
            continue;
        }
        if ctx.store.contains(&name) {
            let items = vec!["덮어쓰기".to_string(), "다른 이름 입력".to_string()];
            let pick = ui::select(&format!("'{name}' 이름의 설정이 이미 있습니다"), &items, 1)?;
            if pick == 1 {
                continue;
            }
        }
        return Ok(name);
    }
}

fn prompt_url(prev: Option<&Draft>) -> Result<String> {
    loop {
        let raw = ui::input(
            "서버 주소 (예: http://192.168.0.10:8000/v1)",
            prev.map(|d| d.base_url.as_str()),
            |s| normalize_base_url(s).map(|_| ()).map_err(|e| e.to_string()),
        )?;
        let normalized = normalize_base_url(&raw)?;
        if normalized.appended_v1 {
            let ok = ui::yes_no(
                &format!(
                    "주소 끝에 /v1이 없어 '{}'로 사용합니다. 맞습니까?",
                    normalized.url
                ),
                true,
            )?;
            if !ok {
                continue;
            }
        }
        return Ok(normalized.url);
    }
}

fn prompt_api_key(prev: Option<&Draft>) -> Result<Option<String>> {
    let items = vec!["인증 없음".to_string(), "API 키·토큰 사용".to_string()];
    let default = usize::from(prev.is_some_and(|d| d.api_key.is_some()));
    if ui::select("인증 방식", &items, default)? == 0 {
        return Ok(None);
    }
    if let Some(old) = prev.and_then(|d| d.api_key.clone()) {
        if ui::yes_no("앞에서 입력한 API 키를 그대로 사용할까요?", true)? {
            return Ok(Some(old));
        }
    }
    Ok(Some(ui::password(
        "API 키·토큰 (입력해도 화면에 표시되지 않습니다)",
    )?))
}

fn prompt_model(
    ctx: &Ctx,
    base_url: &str,
    key: Option<&str>,
    prev: Option<&Draft>,
) -> Result<String> {
    println!("서버에서 모델 목록을 가져오는 중입니다...");
    let prev_model = prev.map(|d| d.model.as_str());
    match fetch_model_listing(base_url, key, ctx.timeout) {
        Ok(ModelListing::Available(models)) if !models.is_empty() => {
            let mut items: Vec<String> = models
                .iter()
                .map(|m| match m.max_model_len {
                    Some(len) => format!("{} (컨텍스트 {len})", m.id),
                    None => m.id.clone(),
                })
                .collect();
            items.push("직접 입력".to_string());
            let default = prev_model
                .and_then(|pm| models.iter().position(|m| m.id == pm))
                .unwrap_or(0);
            let idx = ui::select("사용할 모델", &items, default)?;
            if let Some(m) = models.get(idx) {
                return Ok(m.id.clone());
            }
        }
        Ok(ModelListing::Available(_)) => {
            println!("서버가 모델 목록을 비워 두었습니다. 모델 이름을 직접 입력하세요.")
        }
        Ok(ModelListing::Unavailable) => println!(
            "이 서버는 모델 목록을 제공하지 않습니다. 사용할 모델 이름을 직접 입력하세요 (게이트웨이 관리자에게 받은 이름)."
        ),
        Err(f) => println!(
            "모델 목록을 가져오지 못했습니다 ({}). 모델 이름을 직접 입력하세요.",
            f.kind.headline()
        ),
    }
    ui::input("모델 이름", prev_model, |s| {
        if s.is_empty() {
            Err("모델 이름을 입력해 주세요".to_string())
        } else {
            Ok(())
        }
    })
}

fn prompt_draft(ctx: &Ctx, prev: Option<&Draft>) -> Result<Draft> {
    let name = prompt_name(ctx, prev)?;

    let access_items = vec![
        "서버에 직접 접속 (OpenAI 호환 API)".to_string(),
        "사내 게이트웨이를 거쳐 접속".to_string(),
    ];
    let access_default = usize::from(prev.is_some_and(|d| d.access == AccessKind::Gateway));
    let access = match ui::select("접속 방식", &access_items, access_default)? {
        0 => AccessKind::Direct,
        _ => AccessKind::Gateway,
    };

    let base_url = prompt_url(prev)?;
    let api_key = prompt_api_key(prev)?;
    let model = prompt_model(ctx, &base_url, api_key.as_deref(), prev)?;

    Ok(Draft {
        name,
        access,
        base_url,
        model,
        api_key,
    })
}

fn persist(
    ctx: &mut Ctx,
    draft: &Draft,
    context_window: Option<u64>,
    make_current: bool,
) -> Result<()> {
    match draft.api_key.as_deref() {
        Some(key) => {
            let stored = ctx
                .secrets
                .set(&draft.name, key)
                .context("API 키를 저장하지 못했습니다")?;
            if stored == StoredIn::File {
                println!(
                    "경고: 이 PC의 키체인을 사용할 수 없어 API 키를 파일({})에 저장했습니다.",
                    ctx.bundle.secrets_path().display()
                );
                println!("  이 묶음 폴더를 다른 사람과 공유하거나 반출하지 마세요.");
            }
        }
        None => {
            // 이전에 API 키를 쓰던 설정을 덮어쓴 경우 남은 키를 정리한다.
            let _ = ctx.secrets.delete(&draft.name);
        }
    }
    ctx.store.upsert(Profile {
        name: draft.name.clone(),
        access: draft.access,
        base_url: draft.base_url.clone(),
        model: draft.model.clone(),
        auth: if draft.api_key.is_some() {
            AuthKind::ApiKey
        } else {
            AuthKind::None
        },
        context_window,
    })?;
    if make_current {
        ctx.store.set_current(&draft.name)?;
    }
    ctx.sync_config()
}

/// 새 설정 추가: 입력 → 연결 확인 → 저장.
///
/// `steps`가 true이면 `[2/3]`, `[3/3]` 단계 표시를 출력한다(첫 실행용).
pub fn add_flow(ctx: &mut Ctx, steps: bool) -> Result<AddOutcome> {
    ui::ensure_interactive()?;
    let mut prev: Option<Draft> = None;
    loop {
        if steps {
            println!("\n[2/3] 서버·모델 정보 입력");
        }
        let draft = prompt_draft(ctx, prev.as_ref())?;

        loop {
            if steps {
                println!("\n[3/3] 연결 확인");
            }
            let result = run_check(
                &draft.base_url,
                &draft.model,
                draft.api_key.as_deref(),
                ctx.timeout,
            );
            match result {
                Ok(ok) => {
                    println!(
                        "\n{}",
                        format_success(
                            &draft.name,
                            &draft.model,
                            ok.latency,
                            ok.model_list_skipped
                        )
                    );
                    persist(ctx, &draft, ok.max_model_len, true)?;
                    println!(
                        "\n'{}' 설정을 저장하고 현재 설정으로 지정했습니다.",
                        draft.name
                    );
                    return Ok(AddOutcome::Saved {
                        name: draft.name,
                        made_current: true,
                    });
                }
                Err(failure) => {
                    println!("\n{}\n", format_failure(&failure));
                    let items = vec![
                        "연결 정보 다시 입력".to_string(),
                        "다시 시도".to_string(),
                        "저장하고 나중에 확인".to_string(),
                        "종료".to_string(),
                    ];
                    match ui::select("어떻게 할까요?", &items, 0)? {
                        0 => break,
                        1 => continue,
                        2 => {
                            persist(ctx, &draft, None, false)?;
                            println!(
                                "\n'{}' 설정을 저장했습니다. 연결을 확인하지 못했으므로 현재 설정은 바뀌지 않았습니다.",
                                draft.name
                            );
                            println!(
                                "  나중에 `gjc-connect test {}`로 확인하고 `gjc-connect switch`로 전환하세요.",
                                quote_if_needed(&draft.name)
                            );
                            let made_current = ctx.store.is_current(&draft.name);
                            return Ok(AddOutcome::Saved {
                                name: draft.name,
                                made_current,
                            });
                        }
                        _ => {
                            println!("저장하지 않고 종료합니다.");
                            return Ok(AddOutcome::Discarded);
                        }
                    }
                }
            }
        }
        prev = Some(draft);
    }
}

/// `gjc-connect add`
pub fn add(ctx: &mut Ctx) -> Result<i32> {
    add_flow(ctx, false)?;
    Ok(0)
}

// ---------------------------------------------------------------------------
// switch
// ---------------------------------------------------------------------------

/// `gjc-connect switch`
pub fn switch(ctx: &mut Ctx) -> Result<i32> {
    ui::ensure_interactive()?;

    if ctx.store.is_empty() {
        println!("저장된 서버·모델 설정이 없습니다.");
        let items = vec!["새 설정 추가".to_string(), "종료".to_string()];
        return match ui::select_opt("선택하세요", &items, 0)? {
            Some(0) => {
                add_flow(ctx, false)?;
                Ok(0)
            }
            _ => Ok(0),
        };
    }

    let mut items: Vec<String> = ctx
        .store
        .profiles()
        .iter()
        .map(|p| profile_label(p, ctx.store.is_current(&p.name)))
        .collect();
    items.push("+ 새 설정 추가".to_string());
    let default = ctx
        .store
        .current_name()
        .and_then(|c| ctx.store.profiles().iter().position(|p| p.name == c))
        .unwrap_or(0);

    let Some(idx) = ui::select_opt(
        "전환할 서버·모델 설정을 선택하세요 (Esc: 취소)",
        &items,
        default,
    )?
    else {
        println!("전환하지 않고 종료합니다.");
        return Ok(0);
    };
    if idx >= ctx.store.profiles().len() {
        add_flow(ctx, false)?;
        return Ok(0);
    }

    let profile = ctx.store.profiles()[idx].clone();
    let key = ctx.secret_for(&profile)?;
    match ctx.check_profile(&profile, key.as_deref()) {
        Ok(ok) => {
            println!(
                "\n{}",
                format_success(
                    &profile.name,
                    &profile.model,
                    ok.latency,
                    ok.model_list_skipped
                )
            );
            ctx.store
                .update_context_window(&profile.name, ok.max_model_len)?;
            ctx.store.set_current(&profile.name)?;
            ctx.sync_config()?;
            println!("\n'{}' 설정으로 전환했습니다.", profile.name);
            Ok(0)
        }
        Err(failure) => {
            println!("\n{}\n", format_failure(&failure));
            match ctx.store.current() {
                Some(cur) => println!(
                    "전환하지 않았습니다. 이전 설정('{}')을 계속 사용합니다.",
                    cur.name
                ),
                None => println!("전환하지 않았습니다. 현재 사용 중인 설정이 없습니다."),
            }
            Ok(1)
        }
    }
}

// ---------------------------------------------------------------------------
// list / remove / test
// ---------------------------------------------------------------------------

fn pad(s: &str, width: usize) -> String {
    let w = console::measure_text_width(s);
    format!("{s}{}", " ".repeat(width.saturating_sub(w)))
}

/// `gjc-connect list`
pub fn list(ctx: &Ctx) -> Result<i32> {
    if ctx.store.is_empty() {
        println!("저장된 서버·모델 설정이 없습니다.");
        println!("  `gjc-connect add`로 새 설정을 추가하세요.");
        return Ok(0);
    }
    println!("저장된 설정 {}개", ctx.store.profiles().len());
    let name_w = ctx
        .store
        .profiles()
        .iter()
        .map(|p| console::measure_text_width(&p.name))
        .max()
        .unwrap_or(0)
        + 2;
    let model_w = ctx
        .store
        .profiles()
        .iter()
        .map(|p| console::measure_text_width(&p.model))
        .max()
        .unwrap_or(0)
        + 2;
    for p in ctx.store.profiles() {
        let marker = if ctx.store.is_current(&p.name) {
            "*"
        } else {
            " "
        };
        let current = if ctx.store.is_current(&p.name) {
            "  (현재 사용 중)"
        } else {
            ""
        };
        println!(
            "{marker} {}{}{}  {}{}",
            pad(&p.name, name_w),
            pad(&p.model, model_w),
            pad(p.access.label(), 12),
            p.base_url,
            current
        );
    }
    Ok(0)
}

/// `gjc-connect remove <name>`
pub fn remove(ctx: &mut Ctx, name: &str, yes: bool) -> Result<i32> {
    if !ctx.store.contains(name) {
        eprintln!("'{name}' 이름의 설정이 없습니다. `gjc-connect list`로 이름을 확인하세요.");
        return Ok(1);
    }
    if !yes {
        ui::ensure_interactive().map_err(|_| {
            anyhow!("확인 없이 삭제하려면 --yes 옵션을 사용하세요: gjc-connect remove <이름> --yes")
        })?;
        if !ui::yes_no(
            &format!("'{name}' 설정을 삭제할까요? (저장된 API 키도 함께 삭제됩니다)"),
            false,
        )? {
            println!("삭제하지 않았습니다.");
            return Ok(0);
        }
    }
    let was_current = ctx.store.is_current(name);
    ctx.store.remove(name)?;
    if let Err(e) = ctx.secrets.delete(name) {
        println!("경고: 저장된 API 키를 지우지 못했습니다: {e}");
    }
    ctx.sync_config()?;
    println!("'{name}' 설정을 삭제했습니다.");
    if was_current {
        println!("현재 사용 중이던 설정이라 지금은 선택된 설정이 없습니다. `gjc-connect switch`로 선택하세요.");
    }
    Ok(0)
}

/// `gjc-connect test [name]`
pub fn test(ctx: &mut Ctx, name: Option<&str>) -> Result<i32> {
    let profile = match name {
        Some(n) => match ctx.store.get(n) {
            Some(p) => p.clone(),
            None => {
                eprintln!("'{n}' 이름의 설정이 없습니다. `gjc-connect list`로 이름을 확인하세요.");
                return Ok(1);
            }
        },
        None => match ctx.store.current() {
            Some(p) => p.clone(),
            None => {
                no_current_message();
                return Ok(1);
            }
        },
    };
    let key = ctx.secret_for(&profile)?;
    match ctx.check_profile(&profile, key.as_deref()) {
        Ok(ok) => {
            println!(
                "\n{}",
                format_success(
                    &profile.name,
                    &profile.model,
                    ok.latency,
                    ok.model_list_skipped
                )
            );
            if ctx
                .store
                .update_context_window(&profile.name, ok.max_model_len)?
            {
                ctx.sync_config()?;
            }
            Ok(0)
        }
        Err(failure) => {
            println!("\n{}", format_failure(&failure));
            Ok(1)
        }
    }
}

// ---------------------------------------------------------------------------
// 첫 실행 / 메뉴
// ---------------------------------------------------------------------------

fn print_current_header(ctx: &Ctx) {
    match ctx.store.current() {
        Some(p) => println!("현재 설정: {} — {} ({})", p.name, p.model, p.access.label()),
        None => println!("현재 사용 중인 설정이 없습니다."),
    }
}

/// 인자 없이 `gjc-connect`를 실행했을 때의 흐름.
pub fn first_run(ctx: &mut Ctx) -> Result<i32> {
    ui::ensure_interactive()?;
    println!("gjc-connect — Gajae-Code 사내 모델 연결 도구\n");
    println!("[1/3] 환경 확인");
    environment_check(ctx)?;

    if ctx.store.is_empty() {
        return match add_flow(ctx, true)? {
            AddOutcome::Saved {
                made_current: true, ..
            } => {
                let items = vec!["지금 gjc 실행".to_string(), "종료".to_string()];
                match ui::select_opt("설정이 끝났습니다", &items, 0)? {
                    Some(0) => run(ctx, None),
                    _ => {
                        println!("나중에 `gjc-connect run`으로 실행할 수 있습니다.");
                        Ok(0)
                    }
                }
            }
            _ => Ok(0),
        };
    }

    loop {
        println!();
        print_current_header(ctx);
        let items = vec![
            "실행".to_string(),
            "설정 전환".to_string(),
            "새 설정 추가".to_string(),
            "종료".to_string(),
        ];
        match ui::select_opt("무엇을 할까요?", &items, 0)? {
            Some(0) => {
                if ctx.store.current().is_none() {
                    no_current_message();
                    continue;
                }
                return run(ctx, None);
            }
            Some(1) => {
                switch(ctx)?;
            }
            Some(2) => {
                add_flow(ctx, false)?;
            }
            _ => return Ok(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str, access: AccessKind) -> Profile {
        Profile {
            name: name.into(),
            access,
            base_url: "http://h/v1".into(),
            model: "m".into(),
            auth: AuthKind::None,
            context_window: None,
        }
    }

    #[test]
    fn label_shows_name_model_access_and_current_marker() {
        let direct = profile_label(&p("dgx", AccessKind::Direct), true);
        assert_eq!(direct, "dgx — m (직접 접속) (현재 사용 중)");
        let gw = profile_label(&p("gw", AccessKind::Gateway), false);
        assert_eq!(gw, "gw — m (게이트웨이)");
    }

    #[test]
    fn pad_uses_display_width_for_korean() {
        // 한글은 폭 2로 계산된다.
        assert_eq!(console::measure_text_width(&pad("사내", 8)), 8);
        assert_eq!(console::measure_text_width(&pad("abc", 8)), 8);
        // 이미 넓으면 자르지 않는다.
        assert_eq!(pad("abcdefghij", 4), "abcdefghij");
    }

    #[test]
    fn quotes_and_dir_validation() {
        assert_eq!(strip_quotes("  \"C:\\work\\proj\"  "), "C:\\work\\proj");
        assert_eq!(quote_if_needed("a b"), "\"a b\"");
        assert_eq!(quote_if_needed("ab"), "ab");
        let dir = tempfile::tempdir().unwrap();
        assert!(validate_dir(&dir.path().to_string_lossy()).is_ok());
        let missing = dir.path().join("nope");
        let err = validate_dir(&missing.to_string_lossy()).unwrap_err();
        assert!(err.contains("폴더를 찾을 수 없습니다"));
    }

    fn test_ctx(dir: &tempfile::TempDir) -> Ctx {
        use crate::platform::{Arch, Target};
        use crate::secrets::{FileBackend, SecretStore};
        let bundle = Bundle::new(dir.path(), Os::MacOs);
        let secrets = SecretStore::new(
            Box::new(FileBackend::new(dir.path().join("primary.json"))),
            Box::new(FileBackend::new(bundle.secrets_path())),
        );
        Ctx::open(
            Target {
                os: Os::MacOs,
                arch: Arch::Arm64,
            },
            bundle,
            secrets,
        )
        .unwrap()
    }

    #[test]
    fn remove_deletes_profile_secret_and_regenerates_config() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(&dir);
        let mut prof = p("dgx", AccessKind::Direct);
        prof.auth = AuthKind::ApiKey;
        ctx.secrets.set("dgx", "sk-1").unwrap();
        ctx.store.upsert(prof).unwrap();
        ctx.store.set_current("dgx").unwrap();
        ctx.sync_config().unwrap();

        assert_eq!(remove(&mut ctx, "dgx", true).unwrap(), 0);
        assert!(ctx.store.is_empty());
        assert!(ctx.store.current().is_none());
        assert_eq!(ctx.secrets.get("dgx").unwrap(), None);
        let models = fs::read_to_string(ctx.bundle.agent_dir().join("models.yml")).unwrap();
        assert!(!models.contains("dgx"));
    }

    #[test]
    fn unknown_names_return_exit_code_1_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(&dir);
        assert_eq!(remove(&mut ctx, "ghost", true).unwrap(), 1);
        assert_eq!(test(&mut ctx, Some("ghost")).unwrap(), 1);
        // 현재 설정이 없으면 test/run 모두 거절한다.
        assert_eq!(test(&mut ctx, None).unwrap(), 1);
        assert_eq!(run(&mut ctx, None).unwrap(), 1);
    }

    #[test]
    fn missing_api_key_is_a_clear_error() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx(&dir);
        let mut prof = p("gw", AccessKind::Gateway);
        prof.auth = AuthKind::ApiKey;
        let err = ctx.secret_for(&prof).unwrap_err();
        assert!(err.to_string().contains("API 키를 찾을 수 없습니다"));
        assert_eq!(ctx.secret_for(&p("x", AccessKind::Direct)).unwrap(), None);
    }

    #[test]
    fn list_handles_empty_and_populated_store() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(&dir);
        assert_eq!(list(&ctx).unwrap(), 0);
        ctx.store
            .upsert(p("사내 서버", AccessKind::Gateway))
            .unwrap();
        ctx.store.set_current("사내 서버").unwrap();
        assert_eq!(list(&ctx).unwrap(), 0);
    }
}
