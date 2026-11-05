//! gjc 에이전트 설정 파일(`models.yml`, `config.yml`) 생성.
//!
//! 기존 반입 패키지(`config/models.yml`)를 템플릿으로 삼는다.
//! API 키는 절대 파일에 쓰지 않고 `apiKeyEnv`로 환경변수 이름만 적는다.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::Serialize;
use thiserror::Error;

use crate::atomic::write_atomic;
use crate::platform::Bundle;
use crate::profile::{AuthKind, Profile};

/// 서버가 컨텍스트 길이를 알려 주지 않을 때의 기본값.
pub const DEFAULT_CONTEXT_WINDOW: u64 = 65_536;
/// 출력 토큰 상한의 기본값.
pub const DEFAULT_MAX_TOKENS: u64 = 8_192;

const MODELS_HEADER: &str =
    "# gjc-connect가 자동 생성한 파일입니다. 직접 수정해도 다음 실행 때 덮어써집니다.\n\
# API 키는 이 파일에 저장하지 않습니다 (OS 키체인 + 실행 시 환경변수로 전달).\n";

/// 기존 패키지의 `config/config.yml`과 같은 내용.
pub const CONFIG_YML: &str = "startup:
  checkUpdate: false
  skipLogoAnimation: true
defaultThinkingLevel: off
telemetry:
  enabled: false
marketplace:
  autoUpdate: off
compaction:
  reserveTokens: 8192
  keepRecentTokens: 16384
  remoteEnabled: false
edit:
  mode: replace
";

#[derive(Debug, Error)]
pub enum GenError {
    #[error("models.yml을 만들 수 없습니다: {0}")]
    Serialize(#[from] serde_yaml_ng::Error),
    #[error("설정 파일을 저장할 수 없습니다 ({path}): {source}")]
    Io {
        path: std::path::PathBuf,
        source: io::Error,
    },
}

#[derive(Serialize)]
struct ModelsFile {
    providers: BTreeMap<String, Provider>,
    #[serde(rename = "modelBindings", skip_serializing_if = "Option::is_none")]
    model_bindings: Option<ModelBindings>,
}

#[derive(Serialize)]
struct Provider {
    #[serde(rename = "baseUrl")]
    base_url: String,
    api: &'static str,
    auth: &'static str,
    #[serde(rename = "apiKeyEnv", skip_serializing_if = "Option::is_none")]
    api_key_env: Option<String>,
    #[serde(rename = "cacheRetention")]
    cache_retention: &'static str,
    models: Vec<Model>,
}

#[derive(Serialize)]
struct Model {
    id: String,
    name: String,
    reasoning: bool,
    input: Vec<&'static str>,
    #[serde(rename = "contextWindow")]
    context_window: u64,
    #[serde(rename = "maxTokens")]
    max_tokens: u64,
    cost: Cost,
    compat: Compat,
}

#[derive(Serialize)]
struct Cost {
    input: u32,
    output: u32,
    #[serde(rename = "cacheRead")]
    cache_read: u32,
    #[serde(rename = "cacheWrite")]
    cache_write: u32,
}

#[derive(Serialize)]
struct Compat {
    #[serde(rename = "supportsStore")]
    supports_store: bool,
    #[serde(rename = "supportsDeveloperRole")]
    supports_developer_role: bool,
    #[serde(rename = "supportsReasoningEffort")]
    supports_reasoning_effort: bool,
    #[serde(rename = "supportsStrictMode")]
    supports_strict_mode: bool,
    #[serde(rename = "maxTokensField")]
    max_tokens_field: &'static str,
    #[serde(rename = "extraBody")]
    extra_body: ExtraBody,
}

#[derive(Serialize)]
struct ExtraBody {
    temperature: f64,
    top_p: f64,
}

#[derive(Serialize)]
struct ModelBindings {
    #[serde(rename = "modelRoles")]
    model_roles: ModelRoles,
}

#[derive(Serialize)]
struct ModelRoles {
    default: String,
    smol: String,
    slow: String,
    plan: String,
}

/// `maxTokens`는 8192로 하되 컨텍스트 길이의 절반을 넘지 않게(항상 contextWindow보다 작게) 제한한다.
pub fn max_tokens_for(context_window: u64) -> u64 {
    DEFAULT_MAX_TOKENS.min(context_window / 2).max(1)
}

fn provider_for(profile: &Profile) -> Provider {
    let context_window = profile
        .context_window
        .filter(|w| *w > 0)
        .unwrap_or(DEFAULT_CONTEXT_WINDOW);
    let (auth, api_key_env) = match profile.auth {
        AuthKind::None => ("none", None),
        AuthKind::ApiKey => ("apiKey", profile.api_key_env()),
    };
    Provider {
        base_url: profile.base_url.clone(),
        api: "openai-completions",
        auth,
        api_key_env,
        cache_retention: "none",
        models: vec![Model {
            id: profile.model.clone(),
            name: profile.model.clone(),
            reasoning: false,
            input: vec!["text"],
            context_window,
            max_tokens: max_tokens_for(context_window),
            cost: Cost {
                input: 0,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
            compat: Compat {
                supports_store: false,
                supports_developer_role: false,
                supports_reasoning_effort: false,
                supports_strict_mode: false,
                max_tokens_field: "max_tokens",
                extra_body: ExtraBody {
                    temperature: 0.7,
                    top_p: 0.8,
                },
            },
        }],
    }
}

/// 저장된 모든 프로필을 provider로 등록하고, 현재 프로필을 역할(default/smol/slow/plan)에 바인딩한 `models.yml`을 만든다.
///
/// `current`가 `None`이거나 목록에 없으면 `modelBindings`는 생략한다.
pub fn render_models_yml(profiles: &[Profile], current: Option<&str>) -> Result<String, GenError> {
    let providers = profiles
        .iter()
        .map(|p| (p.provider_id(), provider_for(p)))
        .collect();
    let model_bindings = current
        .and_then(|name| profiles.iter().find(|p| p.name == name))
        .map(|p| {
            let selector = p.selector();
            ModelBindings {
                model_roles: ModelRoles {
                    default: selector.clone(),
                    smol: selector.clone(),
                    slow: selector.clone(),
                    plan: selector,
                },
            }
        });
    let file = ModelsFile {
        providers,
        model_bindings,
    };
    Ok(format!(
        "{MODELS_HEADER}{}",
        serde_yaml_ng::to_string(&file)?
    ))
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> GenError + '_ {
    move |source| GenError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// 에이전트 폴더에 `models.yml`(항상 갱신)과 `config.yml`(없을 때만)을 기록한다.
///
/// `config.yml`은 gjc가 `config set shellPath` 등으로 직접 수정하므로 덮어쓰지 않는다.
pub fn write_agent_config(
    bundle: &Bundle,
    profiles: &[Profile],
    current: Option<&str>,
) -> Result<(), GenError> {
    let dir = bundle.agent_dir();
    fs::create_dir_all(&dir).map_err(io_err(&dir))?;

    let models_path = dir.join("models.yml");
    let models = render_models_yml(profiles, current)?;
    write_atomic(&models_path, models.as_bytes(), false).map_err(io_err(&models_path))?;

    let config_path = dir.join("config.yml");
    if !config_path.exists() {
        write_atomic(&config_path, CONFIG_YML.as_bytes(), false).map_err(io_err(&config_path))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Os;
    use crate::profile::AccessKind;
    use serde_yaml_ng::Value;

    fn profile(name: &str, auth: AuthKind, window: Option<u64>) -> Profile {
        Profile {
            name: name.into(),
            access: AccessKind::Direct,
            base_url: "http://10.0.0.5:8000/v1".into(),
            model: "qwen3-coder-next-fp8".into(),
            auth,
            context_window: window,
        }
    }

    fn parse(yml: &str) -> Value {
        serde_yaml_ng::from_str(yml).unwrap()
    }

    #[test]
    fn no_auth_profile_matches_reference_template() {
        let yml = render_models_yml(&[profile("dgx", AuthKind::None, None)], Some("dgx")).unwrap();
        let v = parse(&yml);
        let p = &v["providers"]["dgx"];
        assert_eq!(p["baseUrl"], "http://10.0.0.5:8000/v1");
        assert_eq!(p["api"], "openai-completions");
        assert_eq!(p["auth"], "none");
        assert!(p.get("apiKeyEnv").is_none());
        assert_eq!(p["cacheRetention"], "none");
        let m = &p["models"][0];
        assert_eq!(m["id"], "qwen3-coder-next-fp8");
        assert_eq!(m["reasoning"], false);
        assert_eq!(m["input"][0], "text");
        assert_eq!(m["contextWindow"], 65536);
        assert_eq!(m["maxTokens"], 8192);
        assert_eq!(m["compat"]["supportsStore"], false);
        assert_eq!(m["compat"]["maxTokensField"], "max_tokens");
        assert_eq!(m["compat"]["extraBody"]["top_p"], 0.8);
        for role in ["default", "smol", "slow", "plan"] {
            assert_eq!(
                v["modelBindings"]["modelRoles"][role],
                "dgx/qwen3-coder-next-fp8"
            );
        }
    }

    #[test]
    fn api_key_profile_uses_env_name_and_never_the_secret() {
        let p = profile("gw", AuthKind::ApiKey, None);
        let env = p.api_key_env().unwrap();
        let yml = render_models_yml(&[p], Some("gw")).unwrap();
        let v = parse(&yml);
        assert_eq!(v["providers"]["gw"]["auth"], "apiKey");
        assert_eq!(v["providers"]["gw"]["apiKeyEnv"], env.as_str());
        assert!(
            !yml.contains("apiKey:"),
            "inline apiKey must not be emitted"
        );
    }

    #[test]
    fn multiple_profiles_are_all_registered_and_only_current_is_bound() {
        let mut other = profile("외부 게이트웨이", AuthKind::ApiKey, Some(32768));
        other.model = "llama".into();
        let profiles = vec![profile("dgx", AuthKind::None, None), other];
        let v = parse(&render_models_yml(&profiles, Some("외부 게이트웨이")).unwrap());
        let providers = v["providers"].as_mapping().unwrap();
        assert_eq!(providers.len(), 2);
        assert!(providers.contains_key("dgx"));
        assert!(providers.contains_key("외부-게이트웨이"));
        assert_eq!(
            v["modelBindings"]["modelRoles"]["default"],
            "외부-게이트웨이/llama"
        );
        assert_eq!(
            v["providers"]["외부-게이트웨이"]["models"][0]["contextWindow"],
            32768
        );
    }

    #[test]
    fn no_current_profile_omits_bindings() {
        let yml = render_models_yml(&[profile("dgx", AuthKind::None, None)], None).unwrap();
        assert!(parse(&yml).get("modelBindings").is_none());
        let yml =
            render_models_yml(&[profile("dgx", AuthKind::None, None)], Some("ghost")).unwrap();
        assert!(parse(&yml).get("modelBindings").is_none());
        // 프로필이 하나도 없어도 유효한 YAML이다.
        assert!(parse(&render_models_yml(&[], None).unwrap())["providers"].is_mapping());
    }

    #[test]
    fn max_tokens_stays_below_context_window() {
        assert_eq!(max_tokens_for(65536), 8192);
        assert_eq!(max_tokens_for(16384), 8192);
        assert_eq!(max_tokens_for(8192), 4096);
        assert_eq!(max_tokens_for(1), 1);
        for w in [2u64, 100, 4096, 8192, 32768, 262_144] {
            assert!(max_tokens_for(w) < w || w < 2, "w={w}");
        }
    }

    #[test]
    fn server_context_window_is_used_and_zero_falls_back() {
        let v = parse(
            &render_models_yml(&[profile("a", AuthKind::None, Some(131_072))], None).unwrap(),
        );
        assert_eq!(v["providers"]["a"]["models"][0]["contextWindow"], 131_072);
        let v = parse(&render_models_yml(&[profile("a", AuthKind::None, Some(0))], None).unwrap());
        assert_eq!(v["providers"]["a"]["models"][0]["contextWindow"], 65536);
    }

    #[test]
    fn config_yml_template_is_valid_and_has_privacy_settings() {
        let v = parse(CONFIG_YML);
        assert_eq!(v["telemetry"]["enabled"], false);
        assert_eq!(v["startup"]["checkUpdate"], false);
        assert_eq!(v["defaultThinkingLevel"], "off");
        assert_eq!(v["compaction"]["reserveTokens"], 8192);
        assert_eq!(v["compaction"]["keepRecentTokens"], 16384);
    }

    #[test]
    fn write_agent_config_is_atomic_and_preserves_existing_config_yml() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = Bundle::new(dir.path(), Os::MacOs);
        let profiles = vec![profile("dgx", AuthKind::None, None)];

        write_agent_config(&bundle, &profiles, Some("dgx")).unwrap();
        let agent = bundle.agent_dir();
        assert!(agent.join("models.yml").exists());
        assert_eq!(
            fs::read_to_string(agent.join("config.yml")).unwrap(),
            CONFIG_YML
        );

        // gjc가 config.yml을 수정했다고 가정: 다시 생성해도 보존되어야 한다.
        fs::write(agent.join("config.yml"), "shellPath: /bin/bash\n").unwrap();
        write_agent_config(&bundle, &profiles, None).unwrap();
        assert_eq!(
            fs::read_to_string(agent.join("config.yml")).unwrap(),
            "shellPath: /bin/bash\n"
        );
        // models.yml은 항상 최신 상태로 갱신된다.
        let v = parse(&fs::read_to_string(agent.join("models.yml")).unwrap());
        assert!(v.get("modelBindings").is_none());
    }
}
