//! Governance SP2b Task 10: the gate line `main` runs,
//! `start_opa_from_config`. No database: the pool points at a port nothing
//! listens on, so a running refresher only fails and retries.

use std::sync::Arc;
use std::time::Duration;

use egret_bin::start_opa_from_config;
use egret_config::{GovernanceConfig, OpaSettings};
use egret_engine_mgmt_api::credential::{EngineCredentialVerifier, EngineIdentity};
use tokio_util::sync::CancellationToken;

struct RefuseAll;

#[async_trait::async_trait]
impl EngineCredentialVerifier for RefuseAll {
    async fn verify(&self, _: uuid::Uuid, _: &str) -> Result<Option<EngineIdentity>, String> {
        Ok(None)
    }
}

fn verifier() -> Option<Arc<dyn EngineCredentialVerifier>> {
    Some(Arc::new(RefuseAll))
}

fn unreachable_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(200))
        .connect_lazy("postgres://egret:egret@127.0.0.1:1/egret")
        .expect("lazy pool")
}

fn gate(enabled: bool) -> GovernanceConfig {
    GovernanceConfig {
        opa: OpaSettings {
            enabled,
            ..OpaSettings::default()
        },
    }
}

#[tokio::test]
async fn the_default_config_starts_opa_when_the_ranger_surface_is_on() {
    // Arrange
    let cancel = CancellationToken::new();

    // Act
    let started = start_opa_from_config(
        &GovernanceConfig::default(),
        verifier(),
        unreachable_pool(),
        &cancel,
    );
    cancel.cancel();

    // Assert: on by default since SP2b-2
    assert!(started.is_some());
}

/// A `MakeWriter` that keeps what the subscriber formats, to read the levels.
#[derive(Clone, Default)]
struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;
    fn make_writer(&'a self) -> Captured {
        self.clone()
    }
}

#[tokio::test]
async fn the_default_config_starts_nothing_without_the_ranger_surface_and_logs_no_error() {
    // Arrange
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(captured.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();

    // Act: no verifier, which is what `ranger.enabled = false` leaves
    let started = tracing::subscriber::with_default(subscriber, || {
        start_opa_from_config(
            &GovernanceConfig::default(),
            None,
            unreachable_pool(),
            &CancellationToken::new(),
        )
    });

    // Assert: a default install must not log an error for a state it is meant to be in
    assert!(started.is_none());
    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(log.contains("OPA surface idle"), "{log}");
    assert!(!log.contains("ERROR"), "{log}");
}

#[tokio::test]
async fn the_gate_off_starts_nothing_even_with_the_ranger_verifier() {
    // Act
    let started = start_opa_from_config(
        &gate(false),
        verifier(),
        unreachable_pool(),
        &CancellationToken::new(),
    );

    // Assert
    assert!(started.is_none());
}

#[tokio::test]
async fn the_gate_on_with_the_ranger_verifier_starts_and_stops_on_cancel() {
    // Arrange
    let cancel = CancellationToken::new();

    // Act
    let (_state, handle) =
        start_opa_from_config(&gate(true), verifier(), unreachable_pool(), &cancel)
            .expect("the gate is on");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let running = !handle.is_finished();
    cancel.cancel();
    let joined = tokio::time::timeout(Duration::from_secs(5), handle).await;

    // Assert
    assert!(running);
    joined
        .expect("the refresher stops when the token is cancelled")
        .expect("no panic");
}

#[tokio::test]
async fn the_gate_on_without_the_ranger_verifier_starts_nothing() {
    // Act
    let started = start_opa_from_config(
        &gate(true),
        None,
        unreachable_pool(),
        &CancellationToken::new(),
    );

    // Assert
    assert!(started.is_none());
}

/// SP2b Task 20F: `[governance.opa] batch_body_limit_bytes` reaches the
/// surface, and the config's default is the surface's.
#[tokio::test]
async fn the_configured_batch_body_limit_reaches_the_surface() {
    assert_eq!(
        egret_config::DEFAULT_OPA_BATCH_BODY_LIMIT_BYTES,
        egret_perm_compile::opa::surface::OPA_DEFAULT_BATCH_BODY_LIMIT_BYTES
    );
    let cancel = CancellationToken::new();
    let config = GovernanceConfig {
        opa: OpaSettings {
            enabled: true,
            batch_body_limit_bytes: 7 * 1024 * 1024,
            body_read_timeout_ms: 2_500,
            ..OpaSettings::default()
        },
    };

    let (state, handle) = start_opa_from_config(&config, verifier(), unreachable_pool(), &cancel)
        .expect("the gate is on");
    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;

    assert_eq!(state.batch_body_limit, 7 * 1024 * 1024);
    assert_eq!(state.body_read_timeout, Duration::from_millis(2_500));
}

/// SP2b Task 20F fix round 1: the config's limits are the surface's numbers.
#[test]
fn the_config_bounds_match_the_surface_budget() {
    use egret_perm_compile::opa::surface as s;
    assert_eq!(
        egret_config::MAX_OPA_BATCH_BODY_LIMIT_BYTES,
        s::OPA_MAX_CONCURRENT_PARSES * s::OPA_BODY_LIMIT_BYTES
    );
    assert_eq!(
        Duration::from_millis(egret_config::DEFAULT_OPA_BODY_READ_TIMEOUT_MS),
        s::OPA_DEFAULT_BODY_READ_TIMEOUT
    );
}
