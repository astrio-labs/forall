use super::sanitize_user_agent;
use super::*;
use core_test_support::skip_if_no_network;
use pretty_assertions::assert_eq;

#[test]
fn credential_endpoints_require_tls_except_on_loopback() {
    for url in [
        "https://auth.example.org/oauth/token",
        "http://localhost:9000",
        "http://127.0.0.1:9000",
        "http://[::1]:9000",
    ] {
        assert!(credential_endpoint(url).is_ok(), "{url}");
    }
    for url in [
        "http://auth.example.org",
        "http://localhost.example.org",
        "http://localhost@auth.example.org",
        "ftp://localhost",
        "relative/path",
    ] {
        assert!(credential_endpoint(url).is_err(), "{url}");
    }
}

#[tokio::test]
async fn credential_request_returns_redirect_to_caller() {
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(path("/start"))
        .respond_with(ResponseTemplate::new(307).insert_header("Location", "/next"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/next"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let response = credential_request(reqwest::Method::POST, &format!("{}/start", server.uri()))
        .unwrap()
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
}

#[test]
fn test_get_forall_user_agent() {
    let user_agent = get_forall_user_agent();
    let originator = originator().value;
    let prefix = format!("{originator}/");
    assert!(user_agent.starts_with(&prefix));
}

#[test]
fn is_first_party_originator_matches_known_values() {
    assert_eq!(is_first_party_originator(DEFAULT_ORIGINATOR), true);
    assert_eq!(is_first_party_originator("forall-tui"), true);
    assert_eq!(is_first_party_originator("forall_vscode"), true);
    assert_eq!(is_first_party_originator("Forall Something Else"), true);
    assert_eq!(is_first_party_originator("forall_cli"), false);
    assert_eq!(is_first_party_originator("Other"), false);
}

#[test]
fn is_first_party_chat_originator_matches_known_values() {
    assert_eq!(is_first_party_chat_originator("forall_atlas"), true);
    assert_eq!(
        is_first_party_chat_originator("forall_chatgpt_desktop"),
        true
    );
    assert_eq!(is_first_party_chat_originator(DEFAULT_ORIGINATOR), false);
    assert_eq!(is_first_party_chat_originator("forall_vscode"), false);
}

#[tokio::test]
async fn test_create_client_sets_default_headers() {
    skip_if_no_network!();

    set_default_client_residency_requirement(Some(ResidencyRequirement::Us));

    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    let client = create_client();

    // Spin up a local mock server and capture a request.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let resp = client
        .get(server.uri())
        .send()
        .await
        .expect("failed to send request");
    assert!(resp.status().is_success());

    let requests = server
        .received_requests()
        .await
        .expect("failed to fetch received requests");
    assert!(!requests.is_empty());
    let headers = &requests[0].headers;

    // originator header is set to the provided value
    let originator_header = headers
        .get("originator")
        .expect("originator header missing");
    assert_eq!(originator_header.to_str().unwrap(), originator().value);

    // User-Agent matches the computed Forall UA for that originator
    let expected_ua = get_forall_user_agent();
    let ua_header = headers
        .get("user-agent")
        .expect("user-agent header missing");
    assert_eq!(ua_header.to_str().unwrap(), expected_ua);

    let residency_header = headers
        .get(RESIDENCY_HEADER_NAME)
        .expect("residency header missing");
    assert_eq!(residency_header.to_str().unwrap(), "us");

    set_default_client_residency_requirement(/*enforce_residency*/ None);
}

#[test]
fn test_invalid_suffix_is_sanitized() {
    let prefix = "forall_cli_rs/0.0.0";
    let suffix = "bad\rsuffix";

    assert_eq!(
        sanitize_user_agent(format!("{prefix} ({suffix})"), prefix),
        "forall_cli_rs/0.0.0 (bad_suffix)"
    );
}

#[test]
fn test_invalid_suffix_is_sanitized2() {
    let prefix = "forall_cli_rs/0.0.0";
    let suffix = "bad\0suffix";

    assert_eq!(
        sanitize_user_agent(format!("{prefix} ({suffix})"), prefix),
        "forall_cli_rs/0.0.0 (bad_suffix)"
    );
}

#[test]
#[cfg(target_os = "macos")]
fn test_macos() {
    use regex_lite::Regex;
    let user_agent = get_forall_user_agent();
    let originator = regex_lite::escape(originator().value.as_str());
    let re = Regex::new(&format!(
        r"^{originator}/\d+\.\d+\.\d+ \(Mac OS \d+\.\d+\.\d+; (x86_64|arm64)\) (\S+)$"
    ))
    .unwrap();
    assert!(re.is_match(&user_agent));
}
