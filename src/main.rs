use std::path::PathBuf;
use std::process::ExitCode;

use clap::error::{ContextKind, ErrorKind};
use clap::{Arg, ArgAction, CommandFactory, FromArgMatches, Parser, Subcommand};

use gjc_connect::commands::{self, Ctx};
use gjc_connect::platform::{self, Bundle};
use gjc_connect::secrets::SecretStore;
use gjc_connect::ui::Cancelled;

const HELP_TEMPLATE: &str = "{about}\n\n사용법: {usage}\n\n{all-args}\n{after-help}";

/// Gajae-Code(gjc)를 사내망 모델 서버에 연결하고 실행하는 도구
#[derive(Parser)]
#[command(
    name = "gjc-connect",
    version,
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    after_help = "인자 없이 실행하면 처음 설정(또는 메뉴)이 시작됩니다."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// 현재 설정으로 gjc를 실행합니다
    Run {
        /// 작업할 프로젝트 폴더 (생략하면 현재 폴더)
        #[arg(value_name = "프로젝트폴더", help_heading = "인자")]
        project_dir: Option<PathBuf>,
    },
    /// 저장된 서버·모델 설정을 선택해 전환합니다
    Switch,
    /// 새 서버·모델 설정을 추가합니다
    Add,
    /// 저장된 설정 목록을 보여 줍니다
    List,
    /// 저장된 설정을 삭제합니다
    Remove {
        /// 삭제할 설정 이름
        #[arg(value_name = "이름", help_heading = "인자")]
        name: String,
        /// 확인 질문 없이 삭제합니다
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// 서버 연결을 확인합니다 (이름을 생략하면 현재 설정)
    Test {
        /// 확인할 설정 이름
        #[arg(value_name = "이름", help_heading = "인자")]
        name: Option<String>,
    },
}

fn build_command() -> clap::Command {
    let help_arg = || {
        Arg::new("help")
            .short('h')
            .long("help")
            .action(ArgAction::Help)
            .help("도움말을 표시합니다")
            .help_heading("옵션")
    };
    Cli::command()
        .help_template(HELP_TEMPLATE)
        .subcommand_help_heading("명령어")
        .subcommand_value_name("명령어")
        .arg(help_arg())
        .arg(
            Arg::new("version")
                .short('V')
                .long("version")
                .action(ArgAction::Version)
                .help("버전을 표시합니다")
                .help_heading("옵션"),
        )
        .mut_subcommands(|sub| {
            sub.help_template(HELP_TEMPLATE)
                .disable_help_flag(true)
                .arg(help_arg())
        })
}

/// clap의 영어 오류 대신 한국어 안내를 출력한다.
fn report_usage_error(err: &clap::Error) {
    let detail = |kind: ContextKind| err.get(kind).map(ToString::to_string);
    let message = match err.kind() {
        ErrorKind::UnknownArgument => format!(
            "알 수 없는 옵션입니다: {}",
            detail(ContextKind::InvalidArg).unwrap_or_default()
        ),
        ErrorKind::InvalidSubcommand => format!(
            "알 수 없는 명령어입니다: {}",
            detail(ContextKind::InvalidSubcommand).unwrap_or_default()
        ),
        ErrorKind::MissingRequiredArgument => format!(
            "필요한 값이 빠졌습니다: {}",
            detail(ContextKind::InvalidArg).unwrap_or_default()
        ),
        _ => "명령어 사용법이 올바르지 않습니다".to_string(),
    };
    eprintln!("오류: {message}");
    eprintln!("  사용법은 `gjc-connect --help`로 확인하세요.");
}

fn exit_code(code: i32) -> ExitCode {
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

fn run() -> anyhow::Result<i32> {
    let cli = match build_command()
        .try_get_matches()
        .and_then(|m| Cli::from_arg_matches(&m))
    {
        Ok(cli) => cli,
        Err(err) => {
            return Ok(match err.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                    err.print()?;
                    0
                }
                _ => {
                    report_usage_error(&err);
                    2
                }
            })
        }
    };

    let target = match platform::current_target() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("오류: {e}");
            return Ok(1);
        }
    };
    let bundle = Bundle::locate(target.os)?;
    let secrets = SecretStore::system(bundle.secrets_path());
    let mut ctx = Ctx::open(target, bundle, secrets)?;

    match cli.command {
        None => commands::first_run(&mut ctx),
        Some(Cmd::Run { project_dir }) => commands::run(&mut ctx, project_dir.as_deref()),
        Some(Cmd::Switch) => commands::switch(&mut ctx),
        Some(Cmd::Add) => commands::add(&mut ctx),
        Some(Cmd::List) => commands::list(&ctx),
        Some(Cmd::Remove { name, yes }) => commands::remove(&mut ctx, &name, yes),
        Some(Cmd::Test { name }) => commands::test(&mut ctx, name.as_deref()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => exit_code(code),
        Err(err) if err.downcast_ref::<Cancelled>().is_some() => {
            eprintln!("\n취소했습니다.");
            ExitCode::from(130)
        }
        Err(err) => {
            eprintln!("오류: {err:#}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_definition_is_valid() {
        build_command().debug_assert();
    }

    #[test]
    fn subcommands_parse() {
        let m = build_command()
            .try_get_matches_from(["gjc-connect", "remove", "dgx", "--yes"])
            .unwrap();
        let cli = Cli::from_arg_matches(&m).unwrap();
        assert!(matches!(cli.command, Some(Cmd::Remove { yes: true, .. })));
        let m = build_command()
            .try_get_matches_from(["gjc-connect"])
            .unwrap();
        assert!(Cli::from_arg_matches(&m).unwrap().command.is_none());
    }

    #[test]
    fn help_is_korean() {
        let help = build_command().render_help().to_string();
        assert!(help.contains("사용법:"));
        assert!(help.contains("명령어"));
        assert!(!help.contains("Usage:"));
        assert!(!help.contains("Commands:"));
    }

    #[test]
    fn unknown_subcommand_is_an_error() {
        let err = build_command()
            .try_get_matches_from(["gjc-connect", "bogus"])
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidSubcommand);
    }
}
