//! HTTP client wrapper with retry, jitter, cancellation, and progress
//! emission.
//!
//! Every HTTP-using connector goes through this wrapper — never
//! `reqwest::Client` directly — for three reasons:
//!
//! 1. **Retry uniformity.** 429 / 5xx backoff is re-implemented
//!    per-connector is how things diverge. One hand-rolled loop here
//!    means every connector gets identical retry semantics, which is
//!    what the user actually experiences.
//! 2. **Cancellation is honoured.** The retry sleep is raced against
//!    `ctx.cancel` so a cancelled run wakes up immediately instead of
//!    sitting out the backoff.
//! 3. **Progress is emitted on every retry.** Silent retries violate
//!    the "never fail silently" principle: the user sees a
//!    `ProgressPhase::InProgress { message: "Retrying after 429…" }`
//!    every time we wait.
//!
//! The retry policy itself is deliberately conservative (max 5
//! attempts, exponential base 500 ms, ±20 % jitter, capped at 30 s)
//! and tunable via [`RetryPolicy`] so connectors that know better can
//! override it.

use std::time::Duration;

use dayseam_core::{error_codes, DayseamError, LogLevel, ProgressPhase};
use dayseam_events::{LogSender, ProgressSender};
use rand::Rng;
use reqwest::{Response, StatusCode};
use tokio_util::sync::CancellationToken;

use crate::clock::{Clock, SystemClock};

/// Tunable retry behaviour. Sensible defaults match what most
/// well-behaved APIs expect; override `max_attempts` or `max_backoff`
/// for sources with stricter rate limits.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Total attempts, including the initial call. Default: 5.
    pub max_attempts: u32,
    /// Initial backoff before the first retry. Default: 500 ms.
    pub base_backoff: Duration,
    /// Ceiling for exponential backoff. Default: 30 s.
    pub max_backoff: Duration,
    /// Jitter range as a fraction of the current backoff. Default: 0.2
    /// (i.e. ±20 %).
    pub jitter_frac: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            base_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
            jitter_frac: 0.2,
        }
    }
}

impl RetryPolicy {
    /// Zero-delay policy used by tests that want to exercise retry
    /// behaviour without waiting.
    pub fn instant() -> Self {
        Self {
            max_attempts: 5,
            base_backoff: Duration::from_millis(0),
            max_backoff: Duration::from_millis(0),
            jitter_frac: 0.0,
        }
    }
}

/// The retry-aware HTTP client wrapped around [`reqwest::Client`].
/// `Clone` is cheap — both inner fields are `Arc`-backed.
#[derive(Debug, Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
    clock: std::sync::Arc<dyn Clock>,
    policy: RetryPolicy,
}

impl HttpClient {
    /// A client with sensible production defaults and a
    /// [`SystemClock`].
    pub fn new() -> Result<Self, DayseamError> {
        let inner = reqwest::Client::builder()
            .user_agent(concat!("dayseam/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| DayseamError::Network {
                code: error_codes::HTTP_TRANSPORT.to_string(),
                message: format!("failed to build http client: {e}"),
            })?;
        Ok(Self {
            inner,
            clock: std::sync::Arc::new(SystemClock),
            policy: RetryPolicy::default(),
        })
    }

    /// Builder hook used by tests and by future specialised connectors
    /// that need a custom retry policy.
    pub fn with_clock(mut self, clock: std::sync::Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Builder hook to override the retry policy.
    pub fn with_policy(mut self, policy: RetryPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Borrow the inner `reqwest::Client` to construct a
    /// `RequestBuilder`. The caller is responsible for calling
    /// [`HttpClient::send`] on the built request so retry semantics
    /// apply.
    pub fn reqwest(&self) -> &reqwest::Client {
        &self.inner
    }

    /// Send `request` with the configured retry policy. Emits
    /// progress on every backoff so the UI always shows *why* a sync
    /// is taking longer than expected.
    ///
    /// `progress` is optional so non-connector callers (future internal
    /// probes, tests) can opt out of progress events; when `None`, the
    /// retry loop still runs but emits no events.
    pub async fn send(
        &self,
        request: reqwest::RequestBuilder,
        cancel: &CancellationToken,
        progress: Option<&ProgressSender>,
        logs: Option<&LogSender>,
    ) -> Result<Response, DayseamError> {
        let mut attempt: u32 = 0;
        let mut last_status: Option<StatusCode> = None;

        loop {
            if cancel.is_cancelled() {
                return Err(DayseamError::Cancelled {
                    code: error_codes::RUN_CANCELLED_BY_USER.to_string(),
                    message: "HTTP call cancelled before send".to_string(),
                });
            }

            // RequestBuilder::try_clone returns None when the body is
            // not cloneable (streamed). Connectors that need to retry
            // such a request must provide a fresh builder per attempt.
            let this_attempt = request.try_clone().ok_or_else(|| DayseamError::Internal {
                code: "connectors_sdk.http.uncloneable_request".to_string(),
                message: "request body is not cloneable; retries disabled".to_string(),
            })?;

            let send_fut = this_attempt.send();
            let response = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    return Err(DayseamError::Cancelled {
                        code: error_codes::RUN_CANCELLED_BY_USER.to_string(),
                        message: "HTTP call cancelled in-flight".to_string(),
                    });
                }
                res = send_fut => res,
            };

            attempt = attempt.saturating_add(1);

            match response {
                Ok(res) => {
                    let status = res.status();
                    last_status = Some(status);
                    if status.is_success() {
                        return Ok(res);
                    }
                    if Self::is_retriable_status(status) && attempt < self.policy.max_attempts {
                        let wait = self.compute_backoff(attempt, retry_after_header(&res));
                        if let Some(p) = progress {
                            p.send(
                                None,
                                ProgressPhase::InProgress {
                                    completed: attempt,
                                    total: Some(self.policy.max_attempts),
                                    message: format!(
                                        "Upstream returned {status}, retrying in {}ms…",
                                        wait.as_millis()
                                    ),
                                },
                            );
                        }
                        if let Some(l) = logs {
                            l.send(
                                LogLevel::Warn,
                                None,
                                format!("retrying after status {status}"),
                                serde_json::json!({
                                    "attempt": attempt,
                                    "max_attempts": self.policy.max_attempts,
                                    "backoff_ms": wait.as_millis(),
                                }),
                            );
                        }
                        self.sleep_cancellable(wait, cancel).await?;
                        continue;
                    }
                    // Not retriable, or budget exhausted.
                    if status == StatusCode::TOO_MANY_REQUESTS {
                        return Err(DayseamError::RateLimited {
                            code: error_codes::HTTP_RETRY_BUDGET_EXHAUSTED.to_string(),
                            retry_after_secs: retry_after_header(&res)
                                .unwrap_or(Duration::from_secs(0))
                                .as_secs(),
                        });
                    }
                    if Self::is_retriable_status(status) {
                        return Err(DayseamError::Network {
                            code: error_codes::HTTP_RETRY_BUDGET_EXHAUSTED.to_string(),
                            message: format!("upstream returned {status} after {attempt} attempts"),
                        });
                    }
                    return Err(DayseamError::Network {
                        code: error_codes::HTTP_TRANSPORT.to_string(),
                        message: format!("upstream returned {status}"),
                    });
                }
                Err(err) => {
                    let retriable = Self::is_retriable_transport(&err);
                    if retriable && attempt < self.policy.max_attempts {
                        let wait = self.compute_backoff(attempt, None);
                        if let Some(p) = progress {
                            p.send(
                                None,
                                ProgressPhase::InProgress {
                                    completed: attempt,
                                    total: Some(self.policy.max_attempts),
                                    message: format!(
                                        "Transport error ({err}), retrying in {}ms…",
                                        wait.as_millis()
                                    ),
                                },
                            );
                        }
                        if let Some(l) = logs {
                            l.send(
                                LogLevel::Warn,
                                None,
                                "retrying after transport error".to_string(),
                                serde_json::json!({
                                    "attempt": attempt,
                                    "max_attempts": self.policy.max_attempts,
                                    "backoff_ms": wait.as_millis(),
                                    "error": err.to_string(),
                                }),
                            );
                        }
                        self.sleep_cancellable(wait, cancel).await?;
                        continue;
                    }
                    return Err(DayseamError::Network {
                        code: if last_status.is_some() {
                            error_codes::HTTP_RETRY_BUDGET_EXHAUSTED.to_string()
                        } else {
                            error_codes::HTTP_TRANSPORT.to_string()
                        },
                        message: format!("http error after {attempt} attempts: {err}"),
                    });
                }
            }
        }
    }

    fn is_retriable_status(status: StatusCode) -> bool {
        matches!(
            status,
            StatusCode::TOO_MANY_REQUESTS
                | StatusCode::INTERNAL_SERVER_ERROR
                | StatusCode::BAD_GATEWAY
                | StatusCode::SERVICE_UNAVAILABLE
                | StatusCode::GATEWAY_TIMEOUT
        )
    }

    fn is_retriable_transport(err: &reqwest::Error) -> bool {
        // Timeouts and connection resets are retry-worthy; a request
        // the user built badly (builder errors, redirect loops) is not.
        err.is_timeout() || err.is_connect() || err.is_request()
    }

    /// Hard ceiling applied when a server returns a pathological
    /// `Retry-After` (e.g. a day). The exponential `max_backoff` on
    /// [`RetryPolicy`] is the ceiling for *our* computed wait, not for
    /// an explicit server instruction — hammering an API that asked
    /// for a longer pause is exactly the anti-social behaviour retry
    /// headers exist to prevent. Five minutes is long enough to
    /// accommodate real rate-limit windows and short enough that a
    /// misconfigured endpoint can't stall a sync indefinitely.
    const MAX_RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

    fn compute_backoff(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        if let Some(ra) = retry_after {
            // Honour the server's hint verbatim, only clipping at the
            // absolute safety ceiling. Do *not* clamp to
            // `policy.max_backoff`: that turns a "back off for 60s"
            // response into a 30s wait and a second 429.
            return ra.min(Self::MAX_RETRY_AFTER);
        }
        if self.policy.base_backoff.is_zero() {
            return Duration::ZERO;
        }
        let exp = self
            .policy
            .base_backoff
            .saturating_mul(1u32 << attempt.min(16));
        let capped = exp.min(self.policy.max_backoff);
        self.apply_jitter(capped)
    }

    fn apply_jitter(&self, base: Duration) -> Duration {
        let frac = self.policy.jitter_frac.clamp(0.0, 1.0);
        if frac == 0.0 {
            return base;
        }
        let mut rng = rand::thread_rng();
        let delta = rng.gen_range(-frac..frac);
        let multiplier = (1.0 + delta).max(0.0);
        let secs = base.as_secs_f64() * multiplier;
        Duration::from_secs_f64(secs.max(0.0))
    }

    async fn sleep_cancellable(
        &self,
        wait: Duration,
        cancel: &CancellationToken,
    ) -> Result<(), DayseamError> {
        if wait.is_zero() {
            // Still yield so the runtime sees the cancellation if it
            // arrived between attempts.
            tokio::task::yield_now().await;
            if cancel.is_cancelled() {
                return Err(DayseamError::Cancelled {
                    code: error_codes::RUN_CANCELLED_BY_USER.to_string(),
                    message: "cancelled between retries".to_string(),
                });
            }
            return Ok(());
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(DayseamError::Cancelled {
                code: error_codes::RUN_CANCELLED_BY_USER.to_string(),
                message: "cancelled during retry backoff".to_string(),
            }),
            _ = self.clock.sleep(wait) => Ok(()),
        }
    }
}

/// Best-effort parse of the `Retry-After` header (seconds form).
fn retry_after_header(res: &Response) -> Option<Duration> {
    res.headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retriable_status_covers_429_and_5xx() {
        assert!(HttpClient::is_retriable_status(
            StatusCode::TOO_MANY_REQUESTS
        ));
        assert!(HttpClient::is_retriable_status(
            StatusCode::INTERNAL_SERVER_ERROR
        ));
        assert!(HttpClient::is_retriable_status(StatusCode::BAD_GATEWAY));
        assert!(HttpClient::is_retriable_status(
            StatusCode::SERVICE_UNAVAILABLE
        ));
        assert!(HttpClient::is_retriable_status(StatusCode::GATEWAY_TIMEOUT));
        assert!(!HttpClient::is_retriable_status(StatusCode::OK));
        assert!(!HttpClient::is_retriable_status(StatusCode::UNAUTHORIZED));
        assert!(!HttpClient::is_retriable_status(StatusCode::NOT_FOUND));
    }

    #[test]
    fn compute_backoff_respects_retry_after() {
        let c = HttpClient::new().expect("build");
        let wait = c.compute_backoff(1, Some(Duration::from_secs(7)));
        assert_eq!(wait, Duration::from_secs(7));
    }

    #[test]
    fn compute_backoff_honours_retry_after_beyond_max_backoff() {
        // If the server asks for 60s, a 30s `max_backoff` must not
        // reduce the wait — otherwise we'd immediately re-hit the
        // rate limit.
        let c = HttpClient::new().expect("build").with_policy(RetryPolicy {
            max_attempts: 5,
            base_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
            jitter_frac: 0.0,
        });
        let wait = c.compute_backoff(1, Some(Duration::from_secs(60)));
        assert_eq!(wait, Duration::from_secs(60));
    }

    #[test]
    fn compute_backoff_clips_pathological_retry_after_at_safety_ceiling() {
        // A malicious / misconfigured server asking for 1 day must not
        // stall the sync forever.
        let c = HttpClient::new().expect("build");
        let wait = c.compute_backoff(1, Some(Duration::from_secs(86_400)));
        assert_eq!(wait, Duration::from_secs(5 * 60));
    }

    #[test]
    fn compute_backoff_caps_at_max() {
        let c = HttpClient::new().expect("build").with_policy(RetryPolicy {
            max_attempts: 10,
            base_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(2),
            jitter_frac: 0.0,
        });
        let wait = c.compute_backoff(10, None);
        assert_eq!(wait, Duration::from_secs(2));
    }

    #[test]
    fn compute_backoff_with_instant_policy_is_zero() {
        let c = HttpClient::new()
            .expect("build")
            .with_policy(RetryPolicy::instant());
        assert_eq!(c.compute_backoff(3, None), Duration::ZERO);
    }
}
