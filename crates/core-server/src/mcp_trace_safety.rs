//! Process-wide tracing boundary for untrusted MCP protocol traffic.
//!
//! rmcp 3.0.0 formats complete requests, responses, notifications, and peer
//! metadata in tracing events. The Host therefore owns a fixed target allowlist
//! and explicitly denies the entire `rmcp` target tree. Environment filters are
//! intentionally not composed into this subscriber, so `RUST_LOG` cannot
//! re-enable protocol bodies. Development builds additionally open the
//! `mycopilot_core::tools::web_retry` target for the web tools' internal retry
//! diagnostics; the allowance is fixed at compile time and is never read from
//! the environment.

use std::sync::OnceLock;

use tracing::level_filters::LevelFilter;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

static INSTALL_RESULT: OnceLock<Result<(), &'static str>> = OnceLock::new();

fn host_targets() -> Targets {
    let targets = Targets::new()
        .with_default(LevelFilter::OFF)
        .with_target("rmcp", LevelFilter::OFF)
        .with_target("core_server", LevelFilter::INFO)
        .with_target("mycopilot_core_server", LevelFilter::INFO)
        .with_target("mycopilot_core", LevelFilter::INFO)
        .with_target("mycopilot_mcp_client", LevelFilter::INFO);
    // Development builds additionally surface the web tools' internal retry diagnostics. The
    // longest matching target prefix wins, so this widens only the dedicated retry target; every
    // other level, including the rmcp denial, stays byte-for-byte identical in both profiles.
    if cfg!(debug_assertions) {
        return targets.with_target("mycopilot_core::tools::web_retry", LevelFilter::DEBUG);
    }
    targets
}

/// Installs the non-overridable Host tracing policy before any MCP connection
/// can start.
///
/// A pre-existing global subscriber is treated as a startup error: the Host
/// cannot prove that it excludes rmcp protocol bodies, so it fails closed
/// instead of continuing with an unknown logging policy.
pub fn install_mcp_safe_tracing() -> Result<(), &'static str> {
    *INSTALL_RESULT.get_or_init(|| {
        let format = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_filter(host_targets());
        tracing_subscriber::registry()
            .with(format)
            .try_init()
            .map_err(|_| "the MCP-safe tracing policy could not be installed")
    })
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

    struct CaptureGuard {
        output: Arc<Mutex<Vec<u8>>>,
    }

    impl io::Write for CaptureGuard {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.output
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedWriter {
        type Writer = CaptureGuard;

        fn make_writer(&'a self) -> Self::Writer {
            CaptureGuard {
                output: Arc::clone(&self.0),
            }
        }
    }

    #[test]
    fn rmcp_protocol_targets_are_denied_while_host_events_remain_observable() {
        const CANARY: &str = "RMCP_PROTOCOL_TRACE_SECRET_CANARY";
        let writer = CapturedWriter::default();
        let output = Arc::clone(&writer.0);
        let format = tracing_subscriber::fmt::layer()
            .without_time()
            .with_ansi(false)
            .with_writer(writer)
            .with_filter(host_targets());
        let subscriber = tracing_subscriber::registry().with(format);

        tracing::subscriber::with_default(subscriber, || {
            tracing::error!(target: "rmcp::service", protocol_body = CANARY);
            tracing::error!(target: "rmcp::transport::async_rw", response = CANARY);
            tracing::info!(target: "mycopilot_core_server::lifecycle", "safe host event");
        });

        let rendered = String::from_utf8(
            output
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone(),
        )
        .expect("captured tracing output should be UTF-8");
        assert!(!rendered.contains(CANARY));
        assert!(rendered.contains("safe host event"));
    }

    #[test]
    fn web_retry_diagnostics_are_visible_only_in_development_builds() {
        const RETRY_MESSAGE: &str = "web tool retry: retry abandoned";
        const RMCP_DEBUG_CANARY: &str = "RMCP_DEBUG_PROTOCOL_SECRET_CANARY";
        let writer = CapturedWriter::default();
        let output = Arc::clone(&writer.0);
        let format = tracing_subscriber::fmt::layer()
            .without_time()
            .with_ansi(false)
            .with_writer(writer)
            .with_filter(host_targets());
        let subscriber = tracing_subscriber::registry().with(format);

        tracing::subscriber::with_default(subscriber, || {
            tracing::debug!(
                target: "mycopilot_core::tools::web_retry",
                attempt = 2,
                cause = "budget-exhausted",
                "{RETRY_MESSAGE}"
            );
            tracing::debug!(target: "rmcp::service", protocol_body = RMCP_DEBUG_CANARY);
        });

        let rendered = String::from_utf8(
            output
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone(),
        )
        .expect("captured tracing output should be UTF-8");
        assert!(!rendered.contains(RMCP_DEBUG_CANARY));
        if cfg!(debug_assertions) {
            assert!(rendered.contains(RETRY_MESSAGE));
        } else {
            assert!(!rendered.contains(RETRY_MESSAGE));
        }
    }
}
