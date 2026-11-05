//! 서버 주소 검증과 `/v1` 정규화.

use thiserror::Error;
use url::Url;

/// 정규화 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedUrl {
    /// 끝에 `/`가 없는 정규화된 주소 (예: `http://10.0.0.5:8000/v1`).
    pub url: String,
    /// 입력에 `/v1`이 없어 자동으로 붙였는지 여부(사용자 확인용).
    pub appended_v1: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UrlError {
    #[error("서버 주소를 입력해 주세요")]
    Empty,
    #[error("주소는 http:// 또는 https://로 시작해야 합니다")]
    MissingScheme,
    #[error("http:// 또는 https:// 주소만 사용할 수 있습니다 (입력한 방식: {0})")]
    UnsupportedScheme(String),
    #[error("주소에 서버 이름(호스트)이 없습니다")]
    MissingHost,
    #[error("주소에 계정 정보(사용자:비밀번호@)를 넣을 수 없습니다")]
    HasCredentials,
    #[error("주소 형식이 올바르지 않습니다: {0}")]
    Invalid(String),
}

/// 사용자가 붙여 넣기 쉬운 끝부분은 제거한다.
const STRIP_SUFFIXES: [&str; 2] = ["/chat/completions", "/models"];

fn is_version_segment(seg: &str) -> bool {
    seg.len() >= 2 && seg.starts_with('v') && seg[1..].chars().all(|c| c.is_ascii_digit())
}

/// 서버 주소를 검증하고 `/v1`로 끝나도록 정규화한다.
///
/// - `http://`/`https://`만 허용한다.
/// - 쿼리·프래그먼트·끝 슬래시는 제거한다.
/// - `/chat/completions`, `/models`로 끝나면 그 부분을 제거한다.
/// - 마지막 경로 조각이 `v1`, `v2` 같은 버전이 아니면 `/v1`을 붙이고 `appended_v1`을 true로 둔다.
pub fn normalize_base_url(input: &str) -> Result<NormalizedUrl, UrlError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(UrlError::Empty);
    }
    if !trimmed.contains("://") {
        return Err(UrlError::MissingScheme);
    }
    let mut parsed = Url::parse(trimmed).map_err(|e| UrlError::Invalid(e.to_string()))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(UrlError::UnsupportedScheme(other.to_string())),
    }
    if parsed.host_str().map(str::is_empty).unwrap_or(true) {
        return Err(UrlError::MissingHost);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(UrlError::HasCredentials);
    }

    parsed.set_query(None);
    parsed.set_fragment(None);

    let mut path = parsed.path().trim_end_matches('/').to_string();
    for suffix in STRIP_SUFFIXES {
        if let Some(stripped) = path.strip_suffix(suffix) {
            path = stripped.trim_end_matches('/').to_string();
            break;
        }
    }

    let last = path.rsplit('/').next().unwrap_or("");
    let appended_v1 = !is_version_segment(last);
    if appended_v1 {
        path.push_str("/v1");
    }

    parsed.set_path(&path);
    Ok(NormalizedUrl {
        url: parsed.as_str().trim_end_matches('/').to_string(),
        appended_v1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(input: &str) -> NormalizedUrl {
        normalize_base_url(input).unwrap()
    }

    #[test]
    fn already_v1_is_kept_without_flag() {
        let n = ok("http://192.168.0.10:18000/v1");
        assert_eq!(n.url, "http://192.168.0.10:18000/v1");
        assert!(!n.appended_v1);
    }

    #[test]
    fn missing_v1_is_appended_and_flagged() {
        let n = ok("  http://10.0.0.5:8000  ");
        assert_eq!(n.url, "http://10.0.0.5:8000/v1");
        assert!(n.appended_v1);
        let g = ok("https://llm-ops.internal/gateway");
        assert_eq!(g.url, "https://llm-ops.internal/gateway/v1");
        assert!(g.appended_v1);
    }

    #[test]
    fn trailing_slash_query_and_endpoint_suffix_are_cleaned() {
        assert_eq!(ok("http://h:1/v1/").url, "http://h:1/v1");
        assert_eq!(ok("http://h:1/v1?x=1#frag").url, "http://h:1/v1");
        let pasted = ok("http://h:1/v1/chat/completions");
        assert_eq!(pasted.url, "http://h:1/v1");
        assert!(!pasted.appended_v1);
        assert_eq!(ok("http://h:1/v1/models").url, "http://h:1/v1");
    }

    #[test]
    fn other_version_segments_are_respected() {
        let n = ok("http://h/api/v2");
        assert_eq!(n.url, "http://h/api/v2");
        assert!(!n.appended_v1);
        // "v"만 있거나 숫자가 아닌 경우는 버전이 아니다.
        assert!(ok("http://h/vendor").appended_v1);
    }

    #[test]
    fn invalid_inputs_are_rejected() {
        assert_eq!(normalize_base_url("   "), Err(UrlError::Empty));
        assert_eq!(
            normalize_base_url("10.0.0.5:8000"),
            Err(UrlError::MissingScheme)
        );
        assert!(matches!(
            normalize_base_url("ftp://10.0.0.5/v1"),
            Err(UrlError::UnsupportedScheme(s)) if s == "ftp"
        ));
        assert_eq!(
            normalize_base_url("http://user:pw@host/v1"),
            Err(UrlError::HasCredentials)
        );
        assert!(normalize_base_url("http://").is_err());
    }

    #[test]
    fn ipv6_hosts_work() {
        assert_eq!(ok("http://[::1]:8000").url, "http://[::1]:8000/v1");
    }
}
