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
async fn the_default_config_starts_nothing() {
    // Act
    let started = start_opa_from_config(
        &GovernanceConfig::default(),
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
