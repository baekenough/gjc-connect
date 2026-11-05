//! 모델 서버 연결 확인.
//!
//! 1. `GET {baseUrl}/models`로 모델 존재 여부와 `max_model_len`을 확인하고
//!    (404/405로 목록을 제공하지 않는 서버·게이트웨이는 이 단계를 건너뛴다)
//! 2. `POST {baseUrl}/chat/completions`로 짧은 요청을 보내 응답 시간을 잰다.
//!
//! 모든 HTTP 호출에 타임아웃이 있으며, 실패는 사용자가 조치할 수 있는 4가지로 분류한다.

use std::io::Read;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use thiserror::Error;

/// 기본 타임아웃(초). `GJC_CONNECT_TIMEOUT_SECS`로 바꿀 수 있다.
pub const DEFAULT_TIMEOUT_SECS: u64 = 10;
/// 모델 목록 확인을 건너뛰었을 때 성공 출력에 붙는 안내 한 줄.
pub const MODEL_LIST_SKIPPED_LINE: &str = "모델 목록  : 서버가 제공하지 않음 (채팅 요청으로 확인)";
/// 응답 본문을 읽을 최대 크기.
const MAX_BODY_BYTES: u64 = 2 * 1024 * 1024;

/// 환경변수 또는 기본값에서 타임아웃을 읽는다.
pub fn timeout_from_env() -> Duration {
    std::env::var("GJC_CONNECT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|s| *s > 0)
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
}

/// 실패 분류.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// 연결 불가·타임아웃
    Unreachable,
    /// 401/403
    AuthFailed,
    /// 서버에 모델이 없음
    ModelMissing,
    /// 응답 형식이 예상과 다름
    UnexpectedResponse,
}

impl FailureKind {
    pub fn headline(self) -> &'static str {
        match self {
            FailureKind::Unreachable => "서버에 연결할 수 없습니다",
            FailureKind::AuthFailed => "API 키가 맞지 않습니다",
            FailureKind::ModelMissing => "서버에 이 모델이 없습니다",
            FailureKind::UnexpectedResponse => "서버 응답을 이해할 수 없습니다",
        }
    }

    fn cause(self) -> &'static str {
        match self {
            FailureKind::Unreachable => "서버가 응답하지 않거나 주소에 닿을 수 없습니다",
            FailureKind::AuthFailed => "서버가 인증을 거부했습니다",
            FailureKind::ModelMissing => "입력한 모델 이름이 서버 모델 목록에 없습니다",
            FailureKind::UnexpectedResponse => "서버가 OpenAI 호환 형식이 아닌 응답을 보냈습니다",
        }
    }

    fn fix(self) -> &'static str {
        match self {
            FailureKind::Unreachable => "사내망·VPN 연결과 서버 주소를 확인하세요",
            FailureKind::AuthFailed => {
                "API 키(토큰)를 다시 확인하세요. 인증이 없는 서버라면 '인증 없음'을 선택하세요"
            }
            FailureKind::ModelMissing => {
                "서버의 모델 목록과 똑같이 모델 이름을 입력하세요 (대소문자 구분)"
            }
            FailureKind::UnexpectedResponse => {
                "접속 방식(직접/게이트웨이)과 주소 끝의 /v1 포함 여부를 확인하세요"
            }
        }
    }
}

/// 연결 확인 실패.
#[derive(Debug, Clone, Error)]
#[error("{}: {detail}", .kind.headline())]
pub struct CheckFailure {
    pub kind: FailureKind,
    /// 기술적 세부 사유(HTTP 상태, 서버 메시지 등)
    pub detail: String,
}

impl CheckFailure {
    fn new(kind: FailureKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

/// 연결 확인 성공 정보.
#[derive(Debug, Clone)]
pub struct CheckSuccess {
    /// `chat/completions` 왕복 시간
    pub latency: Duration,
    /// 서버가 알려 준 컨텍스트 길이
    pub max_model_len: Option<u64>,
    /// `/models`가 404/405라 모델 목록 확인을 건너뛰었는지 여부
    pub model_list_skipped: bool,
}

/// `GET /models` 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelListing {
    /// 서버가 목록을 돌려줌
    Available(Vec<ModelInfo>),
    /// 서버가 목록을 제공하지 않음 (HTTP 404/405)
    Unavailable,
}

/// 서버가 제공하는 모델 한 건.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub max_model_len: Option<u64>,
}

/// 연결 확인 요청.
pub struct CheckRequest<'a> {
    pub base_url: &'a str,
    pub model: &'a str,
    pub api_key: Option<&'a str>,
    pub timeout: Duration,
}

// ---------------------------------------------------------------------------
// 순수 함수: 분류·파싱·표시
// ---------------------------------------------------------------------------

fn body_mentions_missing_model(body: &str) -> bool {
    let lower = body.to_lowercase();
    if !lower.contains("model") {
        return false;
    }
    [
        "not found",
        "does not exist",
        "doesn't exist",
        "not exist",
        "unknown model",
        "no such model",
        "invalid model",
        "model_not_found",
        "not supported",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// 서버 오류 본문에서 사람이 읽을 만한 한 줄을 뽑는다.
fn error_snippet(body: &str) -> String {
    let from_json = serde_json::from_str::<Value>(body).ok().and_then(|v| {
        v.pointer("/error/message")
            .or_else(|| v.get("message"))
            .or_else(|| v.get("detail"))
            .or_else(|| v.get("error"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let text = from_json.unwrap_or_else(|| body.to_string());
    let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = one_line.chars().take(160).collect();
    if one_line.chars().count() > 160 {
        out.push('…');
    }
    out
}

/// HTTP 오류 상태를 분류한다.
pub fn classify_http_failure(status: u16, body: &str) -> CheckFailure {
    let snippet = error_snippet(body);
    let detail = if snippet.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {snippet}")
    };
    let kind = match status {
        401 | 403 => FailureKind::AuthFailed,
        400 | 404 | 422 if body_mentions_missing_model(body) => FailureKind::ModelMissing,
        _ => FailureKind::UnexpectedResponse,
    };
    CheckFailure::new(kind, detail)
}

/// `GET /models` 응답 본문을 파싱한다.
pub fn parse_models(body: &str) -> Result<Vec<ModelInfo>, CheckFailure> {
    let value: Value = serde_json::from_str(body).map_err(|e| {
        CheckFailure::new(
            FailureKind::UnexpectedResponse,
            format!("모델 목록이 JSON이 아닙니다 ({e})"),
        )
    })?;
    let list = value
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
        .ok_or_else(|| {
            CheckFailure::new(
                FailureKind::UnexpectedResponse,
                "모델 목록에 data 배열이 없습니다",
            )
        })?;
    Ok(list
        .iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.to_string();
            let max_model_len = item.get("max_model_len").and_then(Value::as_u64);
            Some(ModelInfo { id, max_model_len })
        })
        .collect())
}

/// `POST /chat/completions` 성공 응답 검증 (choices 배열이 있어야 한다).
fn validate_chat_response(body: &str) -> Result<(), CheckFailure> {
    let value: Value = serde_json::from_str(body).map_err(|e| {
        CheckFailure::new(
            FailureKind::UnexpectedResponse,
            format!("응답이 JSON이 아닙니다 ({e})"),
        )
    })?;
    match value.get("choices").and_then(Value::as_array) {
        Some(choices) if !choices.is_empty() => Ok(()),
        _ => Err(CheckFailure::new(
            FailureKind::UnexpectedResponse,
            "응답에 choices가 없습니다",
        )),
    }
}

fn model_missing(model: &str, models: &[ModelInfo]) -> CheckFailure {
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).take(8).collect();
    let more = models.len().saturating_sub(ids.len());
    let mut detail = format!("'{model}' 모델이 목록에 없습니다");
    if ids.is_empty() {
        detail.push_str(" (서버가 모델을 하나도 알려 주지 않았습니다)");
    } else {
        detail.push_str(&format!(" — 서버 모델: {}", ids.join(", ")));
        if more > 0 {
            detail.push_str(&format!(" 외 {more}개"));
        }
    }
    CheckFailure::new(FailureKind::ModelMissing, detail)
}

/// 응답 시간 표시 (예: `1.8초`).
pub fn format_latency(latency: Duration) -> String {
    format!("{:.1}초", latency.as_secs_f64())
}

/// 성공 출력.
pub fn format_success(
    name: &str,
    model: &str,
    latency: Duration,
    model_list_skipped: bool,
) -> String {
    let mut out = format!(
        "성공: 모델이 응답했습니다\n  설정 이름: {name}\n  모델: {model}\n  응답 시간: {}",
        format_latency(latency)
    );
    if model_list_skipped {
        out.push_str("\n  ");
        out.push_str(MODEL_LIST_SKIPPED_LINE);
    }
    out
}

/// 실패 출력.
pub fn format_failure(failure: &CheckFailure) -> String {
    let kind = failure.kind;
    let cause = if failure.detail.is_empty() {
        kind.cause().to_string()
    } else {
        format!("{} ({})", kind.cause(), failure.detail)
    };
    format!(
        "실패: {}\n  원인: {cause}\n  해결: {}",
        kind.headline(),
        kind.fix()
    )
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(timeout)
        .timeout_connect(timeout)
        .user_agent(concat!("gjc-connect/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn transport_failure(err: &ureq::Transport) -> CheckFailure {
    use ureq::ErrorKind as K;
    let kind = match err.kind() {
        K::Dns | K::ConnectionFailed | K::Io | K::ProxyConnect | K::ProxyUnauthorized => {
            FailureKind::Unreachable
        }
        _ => FailureKind::UnexpectedResponse,
    };
    let detail = match err.kind() {
        K::Dns => "서버 이름을 찾을 수 없습니다".to_string(),
        K::ConnectionFailed => "연결이 거부되었거나 닿지 않습니다".to_string(),
        K::Io => "응답 시간이 초과되었거나 연결이 끊어졌습니다".to_string(),
        K::BadStatus | K::BadHeader => "HTTP 형식이 아닌 응답을 받았습니다".to_string(),
        _ => err.to_string(),
    };
    CheckFailure::new(kind, detail)
}

fn read_body(resp: ureq::Response) -> Result<String, CheckFailure> {
    let mut buf = Vec::new();
    resp.into_reader()
        .take(MAX_BODY_BYTES)
        .read_to_end(&mut buf)
        .map_err(|_| {
            CheckFailure::new(
                FailureKind::Unreachable,
                "응답을 받는 도중 시간이 초과되었거나 연결이 끊어졌습니다",
            )
        })?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn finish(result: Result<ureq::Response, ureq::Error>) -> Result<String, CheckFailure> {
    match result {
        Ok(resp) => read_body(resp),
        Err(ureq::Error::Status(code, resp)) => {
            let body = read_body(resp).unwrap_or_default();
            Err(classify_http_failure(code, &body))
        }
        Err(ureq::Error::Transport(t)) => Err(transport_failure(&t)),
    }
}

fn with_auth(req: ureq::Request, api_key: Option<&str>) -> ureq::Request {
    match api_key {
        Some(key) => req.set("Authorization", &format!("Bearer {key}")),
        None => req,
    }
}

/// `GET {baseUrl}/models`로 서버 모델 목록을 가져온다.
///
/// HTTP 404/405는 "목록을 제공하지 않는 서버"로 보고 [`ModelListing::Unavailable`]을 돌려준다.
/// 그 밖의 실패(401/403 포함)는 그대로 오류로 분류한다.
pub fn fetch_model_listing(
    base_url: &str,
    api_key: Option<&str>,
    timeout: Duration,
) -> Result<ModelListing, CheckFailure> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let req = with_auth(agent(timeout).get(&url), api_key);
    match req.call() {
        Err(ureq::Error::Status(404 | 405, _)) => Ok(ModelListing::Unavailable),
        other => parse_models(&finish(other)?).map(ModelListing::Available),
    }
}

/// `GET {baseUrl}/models`로 서버 모델 목록을 가져온다.
/// 서버가 목록을 제공하지 않으면(404/405) 응답 형식 오류로 취급한다.
pub fn fetch_models(
    base_url: &str,
    api_key: Option<&str>,
    timeout: Duration,
) -> Result<Vec<ModelInfo>, CheckFailure> {
    match fetch_model_listing(base_url, api_key, timeout)? {
        ModelListing::Available(models) => Ok(models),
        ModelListing::Unavailable => Err(CheckFailure::new(
            FailureKind::UnexpectedResponse,
            "서버가 모델 목록을 제공하지 않습니다",
        )),
    }
}

fn chat_probe(req: &CheckRequest<'_>) -> Result<Duration, CheckFailure> {
    let url = format!("{}/chat/completions", req.base_url.trim_end_matches('/'));
    let payload = json!({
        "model": req.model,
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 16,
        "temperature": 0,
        "stream": false,
    });
    let request = with_auth(agent(req.timeout).post(&url), req.api_key)
        .set("Content-Type", "application/json");
    let started = Instant::now();
    let body = finish(request.send_string(&payload.to_string()))?;
    let latency = started.elapsed();
    validate_chat_response(&body)?;
    Ok(latency)
}

/// 모델 목록 확인 → 짧은 채팅 요청까지 수행한다.
///
/// 서버가 `/models`에 404/405로 답하면 목록 확인과 `max_model_len` 감지를 건너뛰고
/// 채팅 요청 결과만으로 판단한다.
pub fn check_connection(req: &CheckRequest<'_>) -> Result<CheckSuccess, CheckFailure> {
    let (max_model_len, model_list_skipped) =
        match fetch_model_listing(req.base_url, req.api_key, req.timeout)? {
            ModelListing::Available(models) => {
                let Some(found) = models.iter().find(|m| m.id == req.model) else {
                    return Err(model_missing(req.model, &models));
                };
                (found.max_model_len, false)
            }
            ModelListing::Unavailable => (None, true),
        };
    let latency = chat_probe(req)?;
    Ok(CheckSuccess {
        latency,
        max_model_len,
        model_list_skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_statuses_are_classified() {
        for status in [401, 403] {
            let f = classify_http_failure(status, r#"{"error":{"message":"bad key"}}"#);
            assert_eq!(f.kind, FailureKind::AuthFailed);
            assert!(f.detail.contains("bad key"));
        }
    }

    #[test]
    fn missing_model_is_distinguished_from_wrong_path_404() {
        let vllm = r#"{"error":{"message":"The model `x` does not exist.","code":404}}"#;
        assert_eq!(
            classify_http_failure(404, vllm).kind,
            FailureKind::ModelMissing
        );
        // 경로가 틀린 404는 모델 문제가 아니다.
        assert_eq!(
            classify_http_failure(404, r#"{"detail":"Not Found"}"#).kind,
            FailureKind::UnexpectedResponse
        );
        assert_eq!(
            classify_http_failure(500, "internal error").kind,
            FailureKind::UnexpectedResponse
        );
    }

    #[test]
    fn models_are_parsed_with_optional_max_model_len() {
        let body =
            r#"{"object":"list","data":[{"id":"a","max_model_len":65536},{"id":"b"},{"nope":1}]}"#;
        let models = parse_models(body).unwrap();
        assert_eq!(
            models,
            vec![
                ModelInfo {
                    id: "a".into(),
                    max_model_len: Some(65536)
                },
                ModelInfo {
                    id: "b".into(),
                    max_model_len: None
                }
            ]
        );
        // 최상위가 배열인 게이트웨이 응답도 허용한다.
        assert_eq!(parse_models(r#"[{"id":"x"}]"#).unwrap().len(), 1);
    }

    #[test]
    fn unparsable_models_are_unexpected_response() {
        for body in ["<html>login</html>", r#"{"hello":1}"#, ""] {
            let err = parse_models(body).unwrap_err();
            assert_eq!(err.kind, FailureKind::UnexpectedResponse, "{body}");
        }
    }

    #[test]
    fn chat_response_requires_choices() {
        assert!(validate_chat_response(r#"{"choices":[{"message":{"content":"pong"}}]}"#).is_ok());
        assert!(validate_chat_response(r#"{"choices":[]}"#).is_err());
        assert!(validate_chat_response("not json").is_err());
    }

    #[test]
    fn output_format_matches_spec() {
        let ok = format_success("사내", "qwen3", Duration::from_millis(1840), false);
        assert!(ok.starts_with("성공: 모델이 응답했습니다"));
        assert!(ok.contains("설정 이름: 사내"));
        assert!(ok.contains("모델: qwen3"));
        assert!(ok.contains("응답 시간: 1.8초"));
        assert!(!ok.contains("모델 목록"));

        let skipped = format_success("사내", "qwen3", Duration::from_millis(1840), true);
        assert!(skipped.contains("모델 목록  : 서버가 제공하지 않음 (채팅 요청으로 확인)"));

        let bad = format_failure(&CheckFailure::new(FailureKind::Unreachable, "시간 초과"));
        assert!(bad.starts_with("실패: 서버에 연결할 수 없습니다"));
        assert!(bad.contains("원인:"));
        assert!(bad.contains("해결: 사내망·VPN 연결과 서버 주소를 확인하세요"));
    }

    #[test]
    fn model_missing_lists_candidates_and_truncates() {
        let models: Vec<ModelInfo> = (0..12)
            .map(|i| ModelInfo {
                id: format!("m{i}"),
                max_model_len: None,
            })
            .collect();
        let f = model_missing("zzz", &models);
        assert_eq!(f.kind, FailureKind::ModelMissing);
        assert!(f.detail.contains("m0"));
        assert!(f.detail.contains("외 4개"));
        assert!(model_missing("zzz", &[]).detail.contains("하나도"));
    }

    #[test]
    fn long_server_messages_are_truncated_to_one_line() {
        let body = format!(r#"{{"error":{{"message":"{}"}}}}"#, "x\n".repeat(400));
        let f = classify_http_failure(500, &body);
        assert!(!f.detail.contains('\n'));
        assert!(f.detail.chars().count() < 200);
    }
}
