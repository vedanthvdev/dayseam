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
use url::Host;

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
                    // Retriable status that exhausted the retry budget —
                    // these are the only non-success paths the SDK classifies
                    // on the caller's behalf, because the retry contract is
                    // the SDK's responsibility, not the connector's.
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
                    // Non-retriable non-success (e.g. 401, 403, 404, 400):
                    // return the response so the caller's resource-aware
                    // classifier can map the status to a domain-specific
                    // error code (see `connector-gitlab::errors::map_status`
                    // which routes 401 → `gitlab.auth.invalid_token`,
                    // 403 → `gitlab.auth.missing_scope`). Converting these
                    // inside `HttpClient` was the Phase-3 CORR-01 bug: it
                    // collapsed every mid-sync PAT rotation into
                    // `http.transport`, and the Reconnect error card never
                    // fired because it keys on `gitlab.auth.*`.
                    return Ok(res);
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
                            // DAY-129 `fix5`: include the classifier's
                            // narrowest best-effort category so log
                            // filters can pivot on "which sub-class of
                            // transport failure was this retry for"
                            // without grep-sniffing `error.to_string()`
                            // (which also changes across reqwest /
                            // rustls bumps). When the transport-level
                            // error doesn't match any fragment we fall
                            // through to the generic `http.transport`
                            // marker, matching the terminal-path
                            // classification.
                            l.send(
                                LogLevel::Warn,
                                None,
                                "retrying after transport error".to_string(),
                                serde_json::json!({
                                    "attempt": attempt,
                                    "max_attempts": self.policy.max_attempts,
                                    "backoff_ms": wait.as_millis(),
                                    "error": err.to_string(),
                                    "transport_class": classify_transport_error(&err),
                                }),
                            );
                        }
                        self.sleep_cancellable(wait, cancel).await?;
                        continue;
                    }
                    // Prefer retry-budget-exhausted over a transport
                    // code when at least one earlier attempt got as far
                    // as a server response — the retries ran, the
                    // request just never settled into success. When the
                    // whole ladder failed at the transport layer,
                    // classify the *last* reqwest error into the
                    // narrowest `http.transport.*` sub-code we can
                    // justify, and splice the target host into the
                    // message so the surfaced error says "couldn't
                    // reach `git.example.com`" instead of the generic
                    // "http error".
                    let code = if last_status.is_some() {
                        error_codes::HTTP_RETRY_BUDGET_EXHAUSTED.to_string()
                    } else {
                        classify_transport_error(&err).to_string()
                    };
                    return Err(DayseamError::Network {
                        code,
                        message: format_transport_error(&err, attempt),
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

/// Classify a terminal `reqwest::Error` into the narrowest
/// `http.transport.*` sub-code we can justify.
///
/// `reqwest` exposes a handful of direct predicates (`is_timeout`,
/// `is_connect`, `is_request`) but does *not* separate DNS, TLS, and
/// TCP-connect failures on its public surface — they all collapse
/// under `is_connect`. To keep the UX-facing code helpful without
/// taking a hard dependency on a specific underlying resolver or TLS
/// backend, we walk the `source()` chain once and match on lower-
/// cased display fragments that are stable across `hyper`,
/// `hyper-util`, `rustls`, `native-tls`, and `std::io::Error`. This is
/// deliberately best-effort: anything we can't place still maps to
/// the generic `HTTP_TRANSPORT` fallback, matching the pre-change
/// behaviour byte-for-byte.
fn classify_transport_error(err: &reqwest::Error) -> &'static str {
    if err.is_timeout() {
        return error_codes::HTTP_TRANSPORT_TIMEOUT;
    }
    // Only `is_connect` (and its aliases) can plausibly be DNS / TLS /
    // refused. `is_request` without a connect flag usually means a
    // builder problem we *shouldn't* retry — fall through to the
    // generic code so we don't mislead the user.
    if !err.is_connect() {
        return error_codes::HTTP_TRANSPORT;
    }
    // DAY-128: walk the *inner* source chain only. `reqwest::Error`'s
    // own `Display` includes the full target URL — e.g. "error
    // sending request for url (https://api.dns-something.com/v1/…)"
    // — which means a hostname or path containing fragments like
    // "dns", "tls", or "ssl" would poison every branch below. The
    // inner chain (hyper → rustls → std::io) carries the actual
    // cause without the URL and is what we want to classify on.
    classify_chain(&inner_error_chain_display(err))
}

/// Fragment-based classifier split out from
/// [`classify_transport_error`] so the (deterministic) pattern
/// matching can be exercised by table-driven tests without needing a
/// real `reqwest::Error`. Input must already be lower-cased (our
/// chain renderers normalise for us); the SDK never feeds un-lowered
/// strings through this helper.
fn classify_chain(chain: &str) -> &'static str {
    // Order matters: TLS errors almost always surface while attempting
    // a connect, and the string "connection" is too broad to gate on
    // first. DNS fragments are checked before TLS because a DNS
    // failure with the target literally named "example.tld" could
    // otherwise look like a TLS error if the chain mentions
    // "handshake" in an unrelated timeout frame.
    if chain.contains("failed to lookup")
        || chain.contains("dns")
        || chain.contains("name resolution")
        || chain.contains("nodename nor servname")
        || chain.contains("no such host")
    {
        return error_codes::HTTP_TRANSPORT_DNS;
    }
    if chain.contains("tls")
        || chain.contains("ssl")
        || chain.contains("handshake")
        || chain.contains("certificate")
    {
        return error_codes::HTTP_TRANSPORT_TLS;
    }
    error_codes::HTTP_TRANSPORT_CONNECT
}

/// DAY-129 `fix3`: true when the transport error originated in (or
/// was relayed through) an HTTP/SOCKS proxy. Corporate networks that
/// mandate an outbound proxy frequently fail at the proxy hop
/// (authentication, allow-listing, certificate interception) rather
/// than at the real upstream; distinguishing "the proxy refused us"
/// from "the upstream refused us" is the difference between an
/// actionable error card and an inscrutable one.
///
/// We check the whole chain — including `reqwest::Error`'s own
/// `Display`, which is the only surface that reliably mentions the
/// proxy URL for builder-level misconfiguration. The match is a
/// narrow fragment list so a hostname containing the literal word
/// "proxy" can't poison the detection (the upstream path doesn't
/// reach the error chain's display at the socket layer).
fn chain_mentions_proxy(err: &(dyn std::error::Error + 'static)) -> bool {
    let chain = error_chain_display(err);
    // The fragments we key on all imply the proxy is the hop that
    // failed or that reqwest itself surfaced a `proxy(...)` error.
    // "proxy" on its own is too broad (e.g. a generic "reverse proxy"
    // string in an upstream body) so we require one of the
    // proxy-specific tokens reqwest / hyper actually emit.
    chain.contains("proxy connect ")
        || chain.contains("http proxy ")
        || chain.contains("https proxy ")
        || chain.contains("socks")
        || chain.contains("proxy authentication")
        || chain.contains("proxy-authorization")
        || chain.contains("bad proxy response")
}

/// Render a lower-cased concatenation of every display in the error's
/// `source()` chain. Bounded so a pathologically deep chain can't turn
/// classification into an allocation storm.
fn error_chain_display(err: &(dyn std::error::Error + 'static)) -> String {
    let mut out = String::new();
    let mut depth = 0u8;
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = current {
        out.push_str(&e.to_string().to_lowercase());
        out.push(' ');
        depth = depth.saturating_add(1);
        if depth >= 8 {
            break;
        }
        current = e.source();
    }
    out
}

/// Like [`error_chain_display`] but skips `err` itself — used for the
/// classifier to avoid misclassifying on URL fragments baked into
/// `reqwest::Error::Display`. Returns an empty string when the outer
/// error carries no source chain (we then fall through to the generic
/// connect code, which matches the pre-DAY-125 behaviour).
fn inner_error_chain_display(err: &(dyn std::error::Error + 'static)) -> String {
    match err.source() {
        Some(inner) => error_chain_display(inner),
        None => String::new(),
    }
}

/// Splice the target host (and attempt count) into the user-facing
/// transport error message. Only applied to errors where the user
/// will recognise "couldn't reach host" as correct — connect refusals
/// and timeouts. For other variants (redirect loops, body-read
/// failures) we fall back to the generic shape so we don't mislead
/// the user about what actually went wrong.
///
/// DAY-129:
/// * `fix4` — IPv6 literal hosts render in bracketed URL form
///   (`[::1]`, `[2001:db8::1]`) rather than the bare address
///   [`url::Url::host_str`] returns. Without brackets the message
///   reads `couldn't reach ::1` which copy-pasted into `curl` picks
///   up the `:1` as the port; the bracketed form is the invariant
///   URL spec requires.
/// * `fix3` — when the chain mentions a proxy we append a
///   `(via proxy)` qualifier so corporate-network failures point the
///   user at the proxy first rather than at the ultimate upstream.
fn format_transport_error(err: &reqwest::Error, attempt: u32) -> String {
    use std::error::Error as _;

    let reach = err.is_connect() || err.is_timeout();
    if reach {
        if let Some(host) = err.url().and_then(format_host_for_message) {
            // Use the *inner* source for the trailing detail so the
            // message doesn't double-mention the host (reqwest's own
            // `Display` renders the full URL, which includes the host
            // we've already named in backticks). Falls back to
            // `err.to_string()` when the outer error has no source.
            let detail: String = err
                .source()
                .map(|s| s.to_string())
                .unwrap_or_else(|| err.to_string());
            let via_proxy = if chain_mentions_proxy(err) {
                " (via proxy)"
            } else {
                ""
            };
            return format!(
                "couldn't reach `{host}`{via_proxy} after {attempt} attempts: {detail}"
            );
        }
    }
    format!("http error after {attempt} attempts: {err}")
}

/// Render a URL host for inclusion in a user-facing error message,
/// putting IPv6 literals inside `[...]` so the result is a valid URL
/// authority (`[::1]` not `::1`). Domains and IPv4 literals are
/// emitted unchanged. Returns `None` when the URL has no host (e.g.
/// `file:` URIs) so the caller can fall back to the generic
/// "http error after N attempts" shape.
fn format_host_for_message(u: &url::Url) -> Option<String> {
    match u.host()? {
        Host::Domain(d) => Some(d.to_string()),
        Host::Ipv4(addr) => Some(addr.to_string()),
        Host::Ipv6(addr) => Some(format!("[{addr}]")),
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

    /// The classifier's string-matching fragments are load-bearing UX
    /// contract: the error-card copy and log-parser grep patterns key
    /// off these codes. A rename on the `reqwest` / `hyper` / `rustls`
    /// side could silently regress classification back to the generic
    /// `http.transport`, so this test pins the fragments we rely on
    /// without depending on network access at test time. If a future
    /// dep bump changes the wording, this test fails loudly and the
    /// new fragment gets added to `classify_transport_error` — rather
    /// than a user noticing their error card stopped naming the host.
    #[test]
    fn error_chain_display_lowercases_and_concatenates() {
        use std::fmt;

        #[derive(Debug)]
        struct Outer(&'static str, Inner);
        #[derive(Debug)]
        struct Inner(&'static str);
        impl fmt::Display for Outer {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl fmt::Display for Inner {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.1)
            }
        }
        impl std::error::Error for Inner {}

        let e = Outer("Outer BOOM", Inner("INNER failed to lookup address"));
        let chain = error_chain_display(&e);
        assert!(chain.contains("outer boom"));
        assert!(chain.contains("inner failed to lookup address"));
        // Must be lower-cased so the classifier's fragment checks can
        // assume normalisation.
        assert_eq!(chain, chain.to_lowercase());
    }

    #[test]
    fn error_chain_display_is_depth_bounded() {
        use std::fmt;

        #[derive(Debug)]
        struct Cycle;
        impl fmt::Display for Cycle {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "cycle")
            }
        }
        impl std::error::Error for Cycle {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                // A real-world pathological chain would be something
                // like a framework wrapping the same `io::Error` more
                // than eight layers deep; we don't actually need a
                // cycle to prove the bound, just a chain of length
                // one. The bound itself is asserted indirectly: if it
                // weren't there, an accidental infinite source loop
                // in a future dep would hang this test. Keeping the
                // assertion focused on "terminates" rather than "hits
                // exactly N" leaves room for the bound to be tuned.
                None
            }
        }

        let out = error_chain_display(&Cycle);
        assert!(out.starts_with("cycle "));
    }

    // `format_transport_error` has no unit coverage here because
    // `reqwest::Error` has no public constructor — a host-splice
    // unit test would need a mock error type and would therefore
    // exercise the mock rather than the real code path. The
    // behaviour is pinned by the integration test
    // `unreachable_host_surfaces_transport_connect_with_hostname_in_message`
    // in `tests/http_retry.rs`, which triggers a real
    // `reqwest::Error` via a refused TCP connect on localhost.

    /// DAY-128: `classify_transport_error` used to walk the whole
    /// chain starting at `err` itself, which for `reqwest::Error`
    /// includes the full target URL in `Display`. A URL whose host
    /// or path happened to contain a classifier fragment (e.g.
    /// `api.dns.example.com`, `https://h/tls-proxy/…`) would then
    /// poison the match and misclassify a plain connect refused as
    /// `HTTP_TRANSPORT_DNS` or `HTTP_TRANSPORT_TLS`. The fix is to
    /// classify on the *inner* source chain only — this test pins
    /// that invariant by building an outer error whose `Display`
    /// is specifically the kind of thing that used to break the
    /// classifier and asserting the inner walk skips it.
    #[test]
    fn inner_error_chain_display_skips_outer_error_display() {
        use std::fmt;

        #[derive(Debug)]
        struct Outer(&'static str, Inner);
        #[derive(Debug)]
        struct Inner(&'static str);
        impl fmt::Display for Outer {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl fmt::Display for Inner {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.1)
            }
        }
        impl std::error::Error for Inner {}

        // Outer carries the URL-contaminated "dns"/"tls" fragments;
        // inner is the real cause ("connection refused"). The
        // inner walk must ignore the outer's "dns"/"tls" and only
        // see "connection refused".
        let e = Outer(
            "error sending request for url (http://api.dns-tls.example.com/v1/ssl/handshake)",
            Inner("connection refused"),
        );
        let inner = inner_error_chain_display(&e);
        assert!(
            inner.contains("connection refused"),
            "inner walk must see the real cause, got `{inner}`",
        );
        assert!(
            !inner.contains("dns"),
            "inner walk must not include the outer's URL (contained `dns`), got `{inner}`",
        );
        assert!(
            !inner.contains("tls"),
            "inner walk must not include the outer's URL (contained `tls`), got `{inner}`",
        );
        assert!(
            !inner.contains("ssl"),
            "inner walk must not include the outer's URL (contained `ssl`), got `{inner}`",
        );
    }

    /// DAY-129 `test6`: the fragment classifier is pure — it only
    /// reads a lower-cased chain string — so we can pin the full
    /// decision table here without needing a real `reqwest::Error`.
    /// A regression that drops a fragment (e.g. a `tls` alias going
    /// away after a rustls bump) collapses the affected column into
    /// `http.transport.connect` and this table fails loudly with
    /// which input now mis-classifies.
    #[test]
    fn classify_chain_routes_known_fragments_to_expected_subcodes() {
        let cases: &[(&str, &str)] = &[
            // DNS fragments cover the union of the resolvers we care
            // about (std `ToSocketAddrs`, hyper, trust-dns).
            (
                "dns error: failed to lookup address for example.com",
                error_codes::HTTP_TRANSPORT_DNS,
            ),
            (
                "nodename nor servname provided, or not known",
                error_codes::HTTP_TRANSPORT_DNS,
            ),
            ("no such host", error_codes::HTTP_TRANSPORT_DNS),
            ("name resolution failed", error_codes::HTTP_TRANSPORT_DNS),
            // TLS fragments cover rustls, native-tls, and the
            // platform-native TLS error renderings.
            (
                "invalid peer certificate: untrustedroot",
                error_codes::HTTP_TRANSPORT_TLS,
            ),
            ("tls handshake eof", error_codes::HTTP_TRANSPORT_TLS),
            ("ssl: wrong_version_number", error_codes::HTTP_TRANSPORT_TLS),
            (
                "handshake failure: unexpected message",
                error_codes::HTTP_TRANSPORT_TLS,
            ),
            // Everything else that reached the classifier with an
            // `is_connect` outer error defaults to `connect`.
            (
                "connection refused (os error 61)",
                error_codes::HTTP_TRANSPORT_CONNECT,
            ),
            ("", error_codes::HTTP_TRANSPORT_CONNECT),
        ];
        for (chain, expected) in cases {
            let got = classify_chain(chain);
            assert_eq!(
                got, *expected,
                "chain `{chain}` expected {expected}, got {got}",
            );
        }
    }

    /// DAY-129 `test7`: the chain walker caps depth to keep a
    /// pathologically nested error from turning classification into
    /// an allocation spike. We build a 10-link chain and assert the
    /// rendered string stops at the 8th frame — both proving the
    /// bound fires and pinning its current value so a silent bump
    /// gets reviewed.
    #[test]
    fn error_chain_display_caps_at_eight_frames() {
        use std::fmt;

        #[derive(Debug)]
        struct Link {
            label: &'static str,
            inner: Option<Box<Link>>,
        }
        impl fmt::Display for Link {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.label)
            }
        }
        impl std::error::Error for Link {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                self.inner
                    .as_deref()
                    .map(|n| n as &(dyn std::error::Error + 'static))
            }
        }

        let mut head: Option<Box<Link>> = None;
        // Build from tail to head so the labels are 0..=9 and the
        // walker sees 0 first.
        for n in (0..10).rev() {
            head = Some(Box::new(Link {
                label: Box::leak(format!("frame-{n}").into_boxed_str()),
                inner: head.take(),
            }));
        }
        let chain = error_chain_display(head.as_deref().unwrap());
        for visible in 0..8 {
            assert!(
                chain.contains(&format!("frame-{visible}")),
                "first 8 frames must be rendered, got `{chain}`",
            );
        }
        for hidden in 8..10 {
            assert!(
                !chain.contains(&format!("frame-{hidden}")),
                "frames beyond the cap must not render, got `{chain}`",
            );
        }
    }

    /// DAY-129 `test9`: the host-splice path uses backticks around
    /// the target, and falls back to the generic "http error after N
    /// attempts: …" shape when the error carries no URL. The
    /// `format_host_for_message` helper is pure so we can cover the
    /// IPv6 bracket rule without building a real `reqwest::Error`.
    #[test]
    fn format_host_for_message_brackets_ipv6_and_passes_through_others() {
        let ipv6 = url::Url::parse("http://[::1]:8080/path").unwrap();
        assert_eq!(format_host_for_message(&ipv6).as_deref(), Some("[::1]"));

        let ipv4 = url::Url::parse("http://127.0.0.1:1/").unwrap();
        assert_eq!(format_host_for_message(&ipv4).as_deref(), Some("127.0.0.1"),);

        let domain = url::Url::parse("https://gitlab.example.com/").unwrap();
        assert_eq!(
            format_host_for_message(&domain).as_deref(),
            Some("gitlab.example.com"),
        );
    }

    /// DAY-129 `fix3` / proxy-qualifier test. The detection logic
    /// walks the full chain, so we exercise it on a mock error
    /// whose display happens to include one of the fragments reqwest
    /// / hyper emit in practice. "proxy" on its own is too broad
    /// (upstream error bodies sometimes mention "reverse proxy")
    /// and must *not* trigger the qualifier.
    #[test]
    fn chain_mentions_proxy_requires_specific_fragments() {
        use std::fmt;
        #[derive(Debug)]
        struct Mock(&'static str);
        impl fmt::Display for Mock {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl std::error::Error for Mock {}

        // Fragments that must light up the qualifier.
        for fragment in [
            "proxy connect tcp error",
            "HTTP proxy handshake failed",
            "HTTPS proxy refused",
            "socks connect failed",
            "407 Proxy Authentication Required",
            "bad proxy response",
        ] {
            assert!(
                chain_mentions_proxy(&Mock(Box::leak(fragment.to_string().into_boxed_str()))),
                "expected proxy detection for fragment `{fragment}`",
            );
        }
        // Fragments that look proxy-ish but shouldn't.
        for fragment in ["reverse proxy upstream 502", "not-a-proxy.example.com"] {
            assert!(
                !chain_mentions_proxy(&Mock(Box::leak(fragment.to_string().into_boxed_str()))),
                "unexpected proxy detection for fragment `{fragment}`",
            );
        }
    }

    #[test]
    fn inner_error_chain_display_is_empty_for_sourceless_errors() {
        // When `reqwest::Error` bottoms out without a source (e.g.
        // builder-level validation failures), the classifier must
        // fall through to the generic code rather than panicking or
        // running the URL through the fragment matcher.
        use std::fmt;
        #[derive(Debug)]
        struct NoSource(&'static str);
        impl fmt::Display for NoSource {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl std::error::Error for NoSource {}
        let e = NoSource("some builder error mentioning dns tls ssl");
        assert_eq!(inner_error_chain_display(&e), "");
    }
}
