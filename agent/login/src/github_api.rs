//! Minimal GitHub REST client backed by the stored connection token.
//!
//! Deliberately tiny: the desktop needs a repository picker, not a GitHub
//! SDK. Requests ride the shared reqwest builder (custom-CA, Forall
//! user-agent) and authenticate with the `repo`-scoped connection token.

use std::time::Duration;

use serde::Deserialize;

use crate::github_connect::GithubConnection;

/// Production API base; tests point at a local mock server instead.
pub const GITHUB_API_BASE: &str = "https://api.github.com";

const LIST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, thiserror::Error)]
pub enum GithubApiError {
    #[error("GitHub API request failed: {0}")]
    Request(String),
    #[error("GitHub API returned HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("invalid GitHub API response: {0}")]
    Invalid(String),
}

/// One repository visible to the token owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubRepoSummary {
    /// `owner/name`.
    pub full_name: String,
    pub name: String,
    /// Login of the owning user or organization.
    pub owner: String,
    pub private: bool,
    /// HTTPS clone URL; the git credential helper authenticates it.
    pub clone_url: String,
    pub default_branch: Option<String>,
    /// RFC 3339 time of the last push.
    pub pushed_at: Option<String>,
}

/// Lists repositories the connected account can access, most recently pushed
/// first (GitHub's `sort=pushed`), capped at one page of 100.
pub async fn list_repositories(
    connection: &GithubConnection,
    api_base: &str,
) -> Result<Vec<GithubRepoSummary>, GithubApiError> {
    #[derive(Deserialize)]
    struct RawOwner {
        login: String,
    }
    #[derive(Deserialize)]
    struct RawRepo {
        full_name: String,
        name: String,
        owner: RawOwner,
        private: bool,
        clone_url: String,
        default_branch: Option<String>,
        pushed_at: Option<String>,
    }

    let client = crate::auth::default_client::build_reqwest_client();
    let response = client
        .get(format!("{api_base}/user/repos"))
        .query(&[("per_page", "100"), ("sort", "pushed")])
        .bearer_auth(&connection.access_token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .timeout(LIST_TIMEOUT)
        .send()
        .await
        .map_err(|err| GithubApiError::Request(err.to_string()))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(GithubApiError::Status {
            status: status.as_u16(),
            body,
        });
    }
    let repos: Vec<RawRepo> = response
        .json()
        .await
        .map_err(|err| GithubApiError::Invalid(err.to_string()))?;
    Ok(repos
        .into_iter()
        .map(|repo| GithubRepoSummary {
            full_name: repo.full_name,
            name: repo.name,
            owner: repo.owner.login,
            private: repo.private,
            clone_url: repo.clone_url,
            default_branch: repo.default_branch,
            pushed_at: repo.pushed_at,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn connection(token: &str) -> GithubConnection {
        GithubConnection {
            access_token: token.to_string(),
            login: "octo".to_string(),
            scopes: vec!["repo".to_string()],
            connected_at: Utc::now(),
        }
    }

    /// Serves one canned `/user/repos` response, then returns the request's
    /// auth header and URL for assertions.
    fn spawn_mock_github(
        status: u16,
        body: serde_json::Value,
    ) -> (String, std::thread::JoinHandle<(Option<String>, String)>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let base = format!("http://{}", server.server_addr().to_ip().unwrap());
        let handle = std::thread::spawn(move || {
            let request = server.recv().unwrap();
            let auth = request
                .headers()
                .iter()
                .find(|header| header.field.equiv("Authorization"))
                .map(|header| header.value.as_str().to_string());
            let url = request.url().to_string();
            let response = tiny_http::Response::from_string(body.to_string())
                .with_status_code(status)
                .with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                        .unwrap(),
                );
            request.respond(response).unwrap();
            (auth, url)
        });
        (base, handle)
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn lists_repositories_with_bearer_auth() {
        let (base, server) = spawn_mock_github(
            200,
            serde_json::json!([
                {
                    "full_name": "octo/widgets",
                    "name": "widgets",
                    "owner": { "login": "octo" },
                    "private": true,
                    "clone_url": "https://github.com/octo/widgets.git",
                    "default_branch": "main",
                    "pushed_at": "2026-08-01T12:00:00Z"
                },
                {
                    "full_name": "acme/site",
                    "name": "site",
                    "owner": { "login": "acme" },
                    "private": false,
                    "clone_url": "https://github.com/acme/site.git",
                    "default_branch": null,
                    "pushed_at": null
                }
            ]),
        );

        let repos = block_on(list_repositories(&connection("tok-1"), &base)).unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].full_name, "octo/widgets");
        assert_eq!(repos[0].owner, "octo");
        assert!(repos[0].private);
        assert_eq!(repos[0].default_branch.as_deref(), Some("main"));
        assert_eq!(repos[1].full_name, "acme/site");

        let (auth, url) = server.join().unwrap();
        assert_eq!(auth.as_deref(), Some("Bearer tok-1"));
        assert!(url.starts_with("/user/repos?"));
        assert!(url.contains("per_page=100"));
        assert!(url.contains("sort=pushed"));
    }

    #[test]
    fn surfaces_http_errors_with_status_and_body() {
        let (base, server) =
            spawn_mock_github(401, serde_json::json!({"message": "Bad credentials"}));
        let err = block_on(list_repositories(&connection("tok-2"), &base)).unwrap_err();
        match err {
            GithubApiError::Status { status, body } => {
                assert_eq!(status, 401);
                assert!(body.contains("Bad credentials"));
            }
            other => panic!("expected Status error, got {other:?}"),
        }
        server.join().unwrap();
    }
}
