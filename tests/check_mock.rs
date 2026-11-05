//! 로컬 모의 서버를 상대로 한 연결 확인·실패 분류 테스트.

mod common;

use std::time::Duration;

use common::{closed_port_url, MockServer, Req, Resp};
use gjc_connect::check::{
    check_connection, fetch_model_listing, fetch_models, format_failure, format_success,
    CheckRequest, FailureKind, ModelListing,
};

const MODEL: &str = "qwen3-coder-next-fp8";
const TIMEOUT: Duration = Duration::from_secs(5);

fn models_json(len: Option<u64>) -> String {
    match len {
        Some(l) => {
            format!(r#"{{"object":"list","data":[{{"id":"{MODEL}","max_model_len":{l}}}]}}"#)
        }
        None => format!(r#"{{"object":"list","data":[{{"id":"{MODEL}"}}]}}"#),
    }
}

const CHAT_OK: &str =
    r#"{"id":"1","choices":[{"index":0,"message":{"role":"assistant","content":"pong"}}]}"#;

fn healthy(req: &Req) -> Resp {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/v1/models") => Resp::Json(200, models_json(Some(32768))),
        ("POST", "/v1/chat/completions") => Resp::Json(200, CHAT_OK.to_string()),
        _ => Resp::Json(404, r#"{"detail":"Not Found"}"#.to_string()),
    }
}

fn check(
    url: &str,
    key: Option<&str>,
    timeout: Duration,
) -> Result<gjc_connect::check::CheckSuccess, gjc_connect::check::CheckFailure> {
    check_connection(&CheckRequest {
        base_url: url,
        model: MODEL,
        api_key: key,
        timeout,
    })
}

#[test]
fn success_reads_max_model_len_and_sends_bearer_token() {
    let server = MockServer::start(healthy);
    let ok = check(&server.base_url, Some("sk-test"), TIMEOUT).unwrap();
    assert_eq!(ok.max_model_len, Some(32768));
    assert!(ok.latency < TIMEOUT);

    let reqs = server.recorded();
    assert_eq!(reqs.len(), 2);
    assert!(reqs
        .iter()
        .all(|r| r.authorization.as_deref() == Some("Bearer sk-test")));
    let chat = reqs.iter().find(|r| r.method == "POST").unwrap();
    let body: serde_json::Value = serde_json::from_str(&chat.body).unwrap();
    assert_eq!(body["model"], MODEL);
    assert_eq!(body["messages"][0]["role"], "user");
}

#[test]
fn success_without_key_sends_no_auth_header_and_tolerates_missing_max_model_len() {
    let server = MockServer::start(|req| match req.path.as_str() {
        "/v1/models" => Resp::Json(200, models_json(None)),
        _ => Resp::Json(200, CHAT_OK.to_string()),
    });
    let ok = check(&server.base_url, None, TIMEOUT).unwrap();
    assert_eq!(ok.max_model_len, None);
    assert!(server.recorded().iter().all(|r| r.authorization.is_none()));
}

#[test]
fn unauthorized_on_models_is_auth_failure() {
    for status in [401u16, 403] {
        let server = MockServer::start(move |_| {
            Resp::Json(
                status,
                r#"{"error":{"message":"Invalid API key"}}"#.to_string(),
            )
        });
        let err = check(&server.base_url, Some("bad"), TIMEOUT).unwrap_err();
        assert_eq!(err.kind, FailureKind::AuthFailed, "status {status}");
        assert!(format_failure(&err).starts_with("실패: API 키가 맞지 않습니다"));
    }
}

#[test]
fn unauthorized_on_chat_only_is_still_auth_failure() {
    let server = MockServer::start(|req| {
        if req.path.ends_with("/models") {
            Resp::Json(200, models_json(None))
        } else {
            Resp::Json(403, r#"{"error":"forbidden"}"#.to_string())
        }
    });
    let err = check(&server.base_url, None, TIMEOUT).unwrap_err();
    assert_eq!(err.kind, FailureKind::AuthFailed);
}

#[test]
fn model_not_in_list_is_model_missing_and_chat_is_never_called() {
    let server =
        MockServer::start(|_| Resp::Json(200, r#"{"data":[{"id":"other-model"}]}"#.to_string()));
    let err = check(&server.base_url, None, TIMEOUT).unwrap_err();
    assert_eq!(err.kind, FailureKind::ModelMissing);
    assert!(err.detail.contains("other-model"));
    assert!(server.recorded().iter().all(|r| r.method == "GET"));
}

#[test]
fn chat_404_naming_the_model_is_model_missing() {
    let server = MockServer::start(|req| {
        if req.path.ends_with("/models") {
            Resp::Json(200, models_json(None))
        } else {
            Resp::Json(
                404,
                format!(
                    r#"{{"error":{{"message":"The model `{MODEL}` does not exist.","code":404}}}}"#
                ),
            )
        }
    });
    let err = check(&server.base_url, None, TIMEOUT).unwrap_err();
    assert_eq!(err.kind, FailureKind::ModelMissing);
}

#[test]
fn unreachable_server_is_unreachable() {
    let err = check(&closed_port_url(), None, TIMEOUT).unwrap_err();
    assert_eq!(err.kind, FailureKind::Unreachable);
    let text = format_failure(&err);
    assert!(text.starts_with("실패: 서버에 연결할 수 없습니다"));
    assert!(text.contains("사내망·VPN 연결과 서버 주소를 확인하세요"));
}

#[test]
fn slow_server_times_out_as_unreachable() {
    let server = MockServer::start(|_| Resp::Hang(Duration::from_secs(3)));
    let started = std::time::Instant::now();
    let err = check(&server.base_url, None, Duration::from_millis(400)).unwrap_err();
    assert_eq!(err.kind, FailureKind::Unreachable);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "timeout was not enforced"
    );
}

#[test]
fn html_login_page_is_unexpected_response() {
    let server = MockServer::start(|_| Resp::Text(200, "<html>SSO login</html>".to_string()));
    let err = check(&server.base_url, None, TIMEOUT).unwrap_err();
    assert_eq!(err.kind, FailureKind::UnexpectedResponse);
    assert!(format_failure(&err).contains("접속 방식"));
}

#[test]
fn wrong_path_404_and_server_error_are_unexpected_response() {
    let server = MockServer::start(|_| Resp::Json(404, r#"{"detail":"Not Found"}"#.to_string()));
    assert_eq!(
        check(&server.base_url, None, TIMEOUT).unwrap_err().kind,
        FailureKind::UnexpectedResponse
    );
    let server = MockServer::start(|_| Resp::Json(500, "boom".to_string()));
    assert_eq!(
        check(&server.base_url, None, TIMEOUT).unwrap_err().kind,
        FailureKind::UnexpectedResponse
    );
}

#[test]
fn chat_without_choices_and_non_http_bytes_are_unexpected_response() {
    let server = MockServer::start(|req| {
        if req.path.ends_with("/models") {
            Resp::Json(200, models_json(None))
        } else {
            Resp::Json(200, r#"{"result":"ok"}"#.to_string())
        }
    });
    assert_eq!(
        check(&server.base_url, None, TIMEOUT).unwrap_err().kind,
        FailureKind::UnexpectedResponse
    );

    let garbage =
        MockServer::start(|_| Resp::Raw(b"\x16\x03\x01 not http at all\r\n\r\n".to_vec()));
    assert_eq!(
        check(&garbage.base_url, None, TIMEOUT).unwrap_err().kind,
        FailureKind::UnexpectedResponse
    );
}

#[test]
fn models_404_is_skipped_and_chat_decides_success() {
    let server = MockServer::start(|req| match req.path.as_str() {
        "/v1/models" => Resp::Json(404, r#"{"detail":"Not Found"}"#.to_string()),
        "/v1/chat/completions" => Resp::Json(200, CHAT_OK.to_string()),
        _ => Resp::Json(404, "{}".to_string()),
    });
    let ok = check(&server.base_url, Some("sk-gw"), TIMEOUT).unwrap();
    assert!(ok.model_list_skipped);
    assert_eq!(ok.max_model_len, None);
    assert!(server
        .recorded()
        .iter()
        .any(|r| r.method == "POST" && r.path == "/v1/chat/completions"));

    let text = format_success("gw", MODEL, ok.latency, ok.model_list_skipped);
    assert!(text.contains("모델 목록  : 서버가 제공하지 않음 (채팅 요청으로 확인)"));

    // 목록이 정상이면 안내 줄이 붙지 않는다.
    let healthy_server = MockServer::start(healthy);
    let ok = check(&healthy_server.base_url, None, TIMEOUT).unwrap();
    assert!(!ok.model_list_skipped);
    assert!(!format_success("x", MODEL, ok.latency, ok.model_list_skipped).contains("모델 목록"));
}

#[test]
fn models_405_with_chat_model_not_found_is_model_missing() {
    let server = MockServer::start(|req| {
        if req.path.ends_with("/models") {
            Resp::Json(405, r#"{"detail":"Method Not Allowed"}"#.to_string())
        } else {
            Resp::Json(
                404,
                format!(
                    r#"{{"error":{{"message":"The model `{MODEL}` does not exist.","code":404}}}}"#
                ),
            )
        }
    });
    let err = check(&server.base_url, None, TIMEOUT).unwrap_err();
    assert_eq!(err.kind, FailureKind::ModelMissing);
    assert!(format_failure(&err).starts_with("실패: 서버에 이 모델이 없습니다"));
}

#[test]
fn models_404_with_chat_401_is_auth_failure() {
    let server = MockServer::start(|req| {
        if req.path.ends_with("/models") {
            Resp::Json(404, r#"{"detail":"Not Found"}"#.to_string())
        } else {
            Resp::Json(
                401,
                r#"{"error":{"message":"Invalid API key"}}"#.to_string(),
            )
        }
    });
    let err = check(&server.base_url, Some("bad"), TIMEOUT).unwrap_err();
    assert_eq!(err.kind, FailureKind::AuthFailed);
}

#[test]
fn fetch_model_listing_reports_unavailable_on_404_and_405() {
    for status in [404u16, 405] {
        let server = MockServer::start(move |_| Resp::Json(status, "{}".to_string()));
        assert_eq!(
            fetch_model_listing(&server.base_url, None, TIMEOUT).unwrap(),
            ModelListing::Unavailable,
            "status {status}"
        );
        // 호환용 fetch_models는 오류로 취급한다.
        assert_eq!(
            fetch_models(&server.base_url, None, TIMEOUT)
                .unwrap_err()
                .kind,
            FailureKind::UnexpectedResponse
        );
    }
}

#[test]
fn fetch_models_lists_models_and_reports_failures() {
    let server = MockServer::start(healthy);
    let models = fetch_models(&server.base_url, None, TIMEOUT).unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, MODEL);
    assert_eq!(models[0].max_model_len, Some(32768));

    let err = fetch_models(&closed_port_url(), None, TIMEOUT).unwrap_err();
    assert_eq!(err.kind, FailureKind::Unreachable);
}

#[test]
fn trailing_slash_in_base_url_is_tolerated() {
    let server = MockServer::start(healthy);
    let url = format!("{}/", server.base_url);
    assert!(check(&url, None, TIMEOUT).is_ok());
}
