//! DAY-115 / SF-1. Process-wide `tracing` subscriber bootstrap.
//!
//! Without this the global dispatcher stays a no-op and every
//! `tracing::error!`, `warn!`, `info!`, and `debug!` in the application
//! is silently discarded in production. That reinstated the original
//! F-10 silent-panic bug even after DAY-113 added
//! [`dayseam_core::runtime::supervised_spawn`]: the supervisor's
//! `tracing::error!` on a panicking task landed on a null subscriber,
//! so the process looked healthy while the task had actually died.
//!
//! The subscriber installed here writes to stderr with an env-filter
//! default of `dayseam_desktop=info,dayseam=info,warn`. `RUST_LOG`
//! overrides the default so developers and CI can dial the level up
//! without a recompile (see `tracing_subscriber::EnvFilter`).
//!
//! `init` uses `try_init` so a second caller (e.g. a test that has
//! already installed its own subscriber via `tracing-test`) cannot
//! panic the process.

/// Install the process-wide subscriber. Idempotent: subsequent calls
/// are no-ops because `tracing_subscriber::fmt::init` uses the global
/// `set_default` guard, which refuses a second install.
pub fn init() {
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("dayseam_desktop=info,dayseam=info,warn"));

    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression guarantee for SF-1: calling `init` leaves a
    /// non-null global dispatcher installed. Pre-fix the desktop
    /// binary never called into `tracing_subscriber` at all, so
    /// `has_been_set()` returned `false` and every `error!` call was
    /// dropped on the floor.
    #[test]
    fn init_installs_a_global_dispatcher() {
        init();
        assert!(
            tracing::dispatcher::has_been_set(),
            "tracing_init::init must install a global subscriber so \
             panic logs in supervised_spawn are not silently dropped"
        );
    }

    /// `init` must be safe to call more than once — tests that
    /// themselves install a subscriber should not race with
    /// `main`'s call. `try_init` is what enforces this; this test
    /// pins that contract so a later refactor to `.init()` (which
    /// panics on double-install) fails loudly in CI rather than at
    /// runtime.
    #[test]
    fn init_is_idempotent() {
        init();
        init();
        init();
    }
}
