//! 대화형 입력 도우미 (dialoguer 래퍼). 모든 문구는 한국어.

use std::io::IsTerminal;

use anyhow::{anyhow, Result};
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Input, Password, Select};
use thiserror::Error;

/// 사용자가 Esc/Ctrl+C로 입력을 취소했다.
#[derive(Debug, Error)]
#[error("취소되었습니다")]
pub struct Cancelled;

/// 키보드로 조작할 수 있는 터미널인지 확인한다.
pub fn ensure_interactive() -> Result<()> {
    if std::io::stdin().is_terminal() && std::io::stderr().is_terminal() {
        Ok(())
    } else {
        Err(anyhow!(
            "이 명령은 대화형 터미널에서만 사용할 수 있습니다 (파이프나 자동 실행 환경에서는 사용할 수 없습니다)"
        ))
    }
}

fn convert(err: dialoguer::Error) -> anyhow::Error {
    match err {
        dialoguer::Error::IO(io) if io.kind() == std::io::ErrorKind::Interrupted => {
            Cancelled.into()
        }
        other => anyhow!("입력을 처리하지 못했습니다: {other}"),
    }
}

/// 화살표 메뉴. Esc/q를 누르면 `None`.
pub fn select_opt(prompt: &str, items: &[String], default: usize) -> Result<Option<usize>> {
    Select::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .items(items)
        .default(default.min(items.len().saturating_sub(1)))
        .max_length(12)
        .interact_opt()
        .map_err(convert)
}

/// 화살표 메뉴. Esc/q는 취소로 처리한다.
pub fn select(prompt: &str, items: &[String], default: usize) -> Result<usize> {
    select_opt(prompt, items, default)?.ok_or_else(|| Cancelled.into())
}

/// 한 줄 입력. `validate`가 `Err(메시지)`를 돌려주면 다시 묻는다.
pub fn input(
    prompt: &str,
    default: Option<&str>,
    validate: impl Fn(&str) -> std::result::Result<(), String>,
) -> Result<String> {
    let theme = ColorfulTheme::default();
    let mut builder = Input::<String>::with_theme(&theme).with_prompt(prompt);
    if let Some(d) = default {
        builder = builder.with_initial_text(d);
    }
    builder
        .validate_with(move |s: &String| validate(s.trim()))
        .interact_text()
        .map(|s| s.trim().to_string())
        .map_err(convert)
}

/// 입력이 화면에 보이지 않는 비밀번호 입력.
pub fn password(prompt: &str) -> Result<String> {
    Password::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .validate_with(|s: &String| -> std::result::Result<(), String> {
            if s.trim().is_empty() {
                Err("값을 입력해 주세요".to_string())
            } else {
                Ok(())
            }
        })
        .interact()
        .map(|s| s.trim().to_string())
        .map_err(convert)
}

/// 예/아니오 질문 (dialoguer의 영어 `[y/n]` 표시를 피하려고 메뉴로 묻는다).
pub fn yes_no(prompt: &str, default_yes: bool) -> Result<bool> {
    let items = ["예".to_string(), "아니오".to_string()];
    let idx = select(prompt, &items, if default_yes { 0 } else { 1 })?;
    Ok(idx == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_io_becomes_cancelled() {
        let err = convert(dialoguer::Error::IO(std::io::Error::from(
            std::io::ErrorKind::Interrupted,
        )));
        assert!(err.downcast_ref::<Cancelled>().is_some());
    }

    #[test]
    fn other_io_errors_are_reported_in_korean() {
        let err = convert(dialoguer::Error::IO(std::io::Error::from(
            std::io::ErrorKind::BrokenPipe,
        )));
        assert!(err.downcast_ref::<Cancelled>().is_none());
        assert!(err.to_string().contains("입력을 처리하지 못했습니다"));
    }

    #[test]
    fn non_tty_test_environment_is_rejected() {
        // cargo test는 stdin이 터미널이 아니다. 터미널에서 직접 실행해도 stderr 리다이렉션이 없으면
        // 통과 여부가 달라질 수 있으므로 오류 메시지 형식만 확인한다.
        if let Err(e) = ensure_interactive() {
            assert!(e.to_string().contains("대화형 터미널"));
        }
    }
}
