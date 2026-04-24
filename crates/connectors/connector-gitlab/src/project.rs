//! Per-project enrichment: `GET /api/v4/projects/:id`.
//!
//! The Events API carries only a numeric `project_id`. Without the
//! project's human-readable path the report rollup falls back to the
//! root-path sentinel `/` and the render ends up printing `**/** — …`
//! for every GitLab bullet (see
//! [`crate::normalise::compose_entities`]).
//!
//! This module exposes a tiny helper the walker uses once per unique
//! `project_id` per walk to fetch `path_with_namespace`. The response
//! is intentionally minimal — we only decode the single field we
//! need so the call is resilient to GitLab's future API additions and
//! tolerant of the self-hosted-vs-`gitlab.com` shape differences.
//!
//! Best-effort semantics for 404 / 5xx / transport errors: these
//! return `Ok(None)` so the walk stays healthy and the downstream
//! normaliser falls back to a synthetic `project-<id>` label.
//!
//! **401/403 are fatal** (DAY-72 CORR-addendum-01): the walker's
//! up-front PAT validation happens once at the start of the walk, so
//! a mid-walk 401/403 on `/projects/:id` means the token was
//! revoked / rotated between validation and enrichment. Collapsing
//! that into `Ok(None)` masked the DAY-71 shape of bug: the user
//! sees bullets labelled `**project-1234** — …` instead of a clear
//! "Reconnect GitLab" prompt, and every subsequent day's sync keeps
//! silently failing the same way. We now propagate these as typed
//! `DayseamError::Auth` so `sync_runs.last_error` points the UI at
//! the reconnect flow.
//!
//! The walker emits a single Warn log per fell-back lookup so the
//! downgrade is not silent — `reports-debug` in the desktop app will
//! show which `project_id`s fell back.

use std::sync::Arc;

use connectors_sdk::{AuthStrategy, HttpClient};
use dayseam_core::{DayseamError, SourceId};
use dayseam_events::{LogSender, ProgressSender};
use reqwest::StatusCode;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

/// Subset of `GET /api/v4/projects/:id` we care about.
///
/// GitLab returns 30+ fields here; `serde`'s default "ignore unknown"
/// behaviour keeps us decoupled from upstream bloat. The field is
/// optional because GitLab sometimes returns `null` for deleted or
/// partially-indexed projects; we treat `None` the same as a 404.
#[derive(Debug, Clone, Deserialize)]
struct GitlabProject {
    path_with_namespace: Option<String>,
}

/// Fetch the `path_with_namespace` (e.g. `"company/modulo-local-infra"`)
/// for a single project id. Returns `Ok(None)` on every non-success
/// outcome — see module docs for the rationale.
///
/// `cancel`, `progress`, `logs` are threaded through so the shared
/// [`HttpClient::send`] retry loop can emit rate-limit progress the
/// same way the events walk does.
pub async fn fetch_project_path(
    http: &HttpClient,
    auth: Arc<dyn AuthStrategy>,
    base_url: &str,
    project_id: i64,
    cancel: &CancellationToken,
    progress: Option<&ProgressSender>,
    logs: Option<&LogSender>,
) -> Result<Option<String>, DayseamError> {
    let base = base_url.trim_end_matches('/');
    let url = format!("{base}/api/v4/projects/{project_id}");

    let request = http.reqwest().get(&url);
    let request = auth.authenticate(request).await?;

    let response = match http.send(request, cancel, progress, logs).await {
        Ok(resp) => resp,
        Err(DayseamError::Cancelled { code, message }) => {
            return Err(DayseamError::Cancelled { code, message });
        }
        Err(_) => {
            // Network / retry-exhausted 5xx / unexpected status. We
            // already degrade to the synthetic fallback in the
            // caller; don't tear the whole walk down for a label.
            return Ok(None);
        }
    };

    let status = response.status();
    if !status.is_success() {
        // DAY-72 CORR-addendum-01: 401/403 is a real credential
        // problem that the walk must propagate, not a pretty-label
        // miss. A rotated PAT lands here if the token was revoked
        // between walk-start PAT validation and enrichment; without
        // this branch the walk silently succeeds with synthetic
        // `project-<id>` labels on every bullet, which is the
        // DAY-71 shape. Everything else (404 = project deleted /
        // moved, 5xx = upstream hiccup we already retried, 4xx we
        // don't classify) stays best-effort `Ok(None)`.
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            let body = response.text().await.unwrap_or_default();
            return Err(crate::errors::map_status(status, body).into());
        }
        return Ok(None);
    }

    let project: GitlabProject = match response.json().await {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };

    Ok(project.path_with_namespace.filter(|s| !s.trim().is_empty()))
}

/// Log a Warn line noting that a project lookup fell back to the
/// synthetic label. Kept separate so the walker can call it without
/// needing to know the log schema the rest of the walk uses.
pub fn emit_project_lookup_warning(
    logs: &LogSender,
    source_id: SourceId,
    project_id: i64,
    reason: &str,
) {
    logs.send(
        dayseam_core::LogLevel::Warn,
        Some(source_id),
        format!("GitLab project {project_id}: falling back to synthetic repo label ({reason})"),
        serde_json::json!({
            "code": "gitlab.project.lookup_fallback",
            "project_id": project_id,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use connectors_sdk::{PatAuth, RetryPolicy};

    fn http_for_tests() -> HttpClient {
        HttpClient::new()
            .expect("HttpClient::new")
            .with_policy(RetryPolicy::instant())
    }

    fn auth_for_tests() -> Arc<dyn AuthStrategy> {
        Arc::new(PatAuth::gitlab("test-pat", "dayseam.gitlab", "acme"))
    }

    #[tokio::test]
    async fn fetch_project_path_returns_path_on_200() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/42"))
            .and(header("PRIVATE-TOKEN", "test-pat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 42,
                "name": "modulo-local-infra",
                "path_with_namespace": "company/modulo-local-infra",
                "web_url": "https://gitlab.example/company/modulo-local-infra"
            })))
            .mount(&server)
            .await;

        let got = fetch_project_path(
            &http_for_tests(),
            auth_for_tests(),
            &server.uri(),
            42,
            &CancellationToken::new(),
            None,
            None,
        )
        .await
        .expect("200 should not be an error");
        assert_eq!(got.as_deref(), Some("company/modulo-local-infra"));
    }

    #[tokio::test]
    async fn fetch_project_path_returns_none_on_404() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/9999"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let got = fetch_project_path(
            &http_for_tests(),
            auth_for_tests(),
            &server.uri(),
            9999,
            &CancellationToken::new(),
            None,
            None,
        )
        .await
        .expect("404 must degrade to Ok(None), not propagate");
        assert_eq!(got, None);
    }

    /// DAY-72 CORR-addendum-01: 401 / 403 on `/projects/:id` means
    /// the PAT was revoked / rotated / scope-downgraded between
    /// walk-start validation and enrichment. The previous behaviour
    /// (collapse into `Ok(None)` + synthetic `project-<id>` label)
    /// masked a real credential problem — the user would see
    /// `**project-1234** — …` bullets on every GitLab event with no
    /// reconnect prompt. We now propagate as a typed
    /// `DayseamError::Auth` so `sync_runs.last_error` points the UI
    /// at the reconnect flow.
    #[tokio::test]
    async fn fetch_project_path_propagates_401_as_auth_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/42"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let err = fetch_project_path(
            &http_for_tests(),
            auth_for_tests(),
            &server.uri(),
            42,
            &CancellationToken::new(),
            None,
            None,
        )
        .await
        .expect_err("401 must propagate as auth error, not Ok(None)");
        assert!(
            matches!(err, DayseamError::Auth { .. }),
            "expected DayseamError::Auth, got {err:?}"
        );
        let code = match &err {
            DayseamError::Auth { code, .. } => code.clone(),
            _ => unreachable!(),
        };
        assert_eq!(code, dayseam_core::error_codes::GITLAB_AUTH_INVALID_TOKEN);
    }

    #[tokio::test]
    async fn fetch_project_path_propagates_403_as_auth_error() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/42"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let err = fetch_project_path(
            &http_for_tests(),
            auth_for_tests(),
            &server.uri(),
            42,
            &CancellationToken::new(),
            None,
            None,
        )
        .await
        .expect_err("403 must propagate as auth error, not Ok(None)");
        assert!(
            matches!(err, DayseamError::Auth { .. }),
            "expected DayseamError::Auth, got {err:?}"
        );
        let code = match &err {
            DayseamError::Auth { code, .. } => code.clone(),
            _ => unreachable!(),
        };
        assert_eq!(code, dayseam_core::error_codes::GITLAB_AUTH_MISSING_SCOPE);
    }

    #[tokio::test]
    async fn fetch_project_path_returns_none_when_field_missing() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v4/projects/42"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": 42
            })))
            .mount(&server)
            .await;

        let got = fetch_project_path(
            &http_for_tests(),
            auth_for_tests(),
            &server.uri(),
            42,
            &CancellationToken::new(),
            None,
            None,
        )
        .await
        .expect("missing field must degrade to Ok(None)");
        assert_eq!(got, None);
    }

    #[tokio::test]
    async fn fetch_project_path_returns_none_on_transport_error() {
        // Port 1 is reliably unbound; the HttpClient retries exhaust
        // quickly with `RetryPolicy::instant` and we swallow.
        let got = fetch_project_path(
            &http_for_tests(),
            auth_for_tests(),
            "http://127.0.0.1:1",
            42,
            &CancellationToken::new(),
            None,
            None,
        )
        .await
        .expect("transport error must degrade to Ok(None)");
        assert_eq!(got, None);
    }
}
