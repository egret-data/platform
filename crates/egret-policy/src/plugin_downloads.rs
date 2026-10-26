//! The plugin download ledger (view governance B4).
//!
//! egret IS the Ranger Admin, so every plugin poll reaches `download_policies`
//! / `download_tags`. Each poll that names a known service and carries a
//! `pluginId` is recorded in `policy.ranger_plugin_downloads`, keyed by
//! `(service the poll names, plugin id, kind)`.
//!
//! "Plugin P holds tag version V" means a poll from P reported
//! `lastKnownVersion >= V`. The version egret SERVED on a 200 is stored in
//! `served_version` but never counts: the plugin may fail to apply it.
//!
//! The download endpoints are anonymous to anyone who can reach egret, so a
//! forged poll can end a hold early. It cannot change what the real plugin
//! applies on its next poll.
use crate::{PolicyStore, PolicyStoreError};
use std::sync::atomic::Ordering;
use std::time::Duration;

/// `waiting_on` when no plugin has polled the service within the window.
/// Confirmation is never vacuous.
pub const NO_PLUGIN_POLLED: &str = "<no plugin has polled>";

/// Rows silent for this long are pruned.
const RETENTION: Duration = Duration::from_secs(24 * 3600);
const PRUNE_EVERY: u64 = 64;

const WAIT_STEP: Duration = Duration::from_millis(500);
/// A plugin that has not polled within three 30 s poll intervals is not waited
/// on (spec B4). Used by `wait_for_versions`.
const RECENT: Duration = Duration::from_secs(90);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadKind {
    Policies,
    Tags,
}

impl DownloadKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Policies => "policies",
            Self::Tags => "tags",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldState {
    /// Every plugin seen recently holds the version, and at least one was seen.
    pub confirmed: bool,
    /// Plugin ids still below the version, or [`NO_PLUGIN_POLLED`].
    pub waiting_on: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum WaitError {
    #[error("plugins did not confirm the version in time; waiting on {waiting_on:?}")]
    Timeout { waiting_on: Vec<String> },
    #[error(transparent)]
    Store(#[from] PolicyStoreError),
}

impl PolicyStore {
    /// Upsert what `plugin_id` just asked for. `last_known_version` is what the
    /// plugin reported; `served_version` is what a 200 carried (`None` on a
    /// 304, which keeps the earlier value).
    pub async fn record_plugin_download(
        &self,
        service_id: i64,
        plugin_id: &str,
        kind: DownloadKind,
        last_known_version: Option<i64>,
        served_version: Option<i64>,
    ) -> Result<(), PolicyStoreError> {
        sqlx::query(
            "INSERT INTO policy.ranger_plugin_downloads \
                 (service_id, plugin_id, kind, last_known_version, served_version, at) \
             VALUES ($1, $2, $3, $4, $5, now()) \
             ON CONFLICT (service_id, plugin_id, kind) DO UPDATE SET \
                 last_known_version = EXCLUDED.last_known_version, \
                 served_version = COALESCE(EXCLUDED.served_version, \
                                           policy.ranger_plugin_downloads.served_version), \
                 at = now()",
        )
        .bind(service_id)
        .bind(plugin_id)
        .bind(kind.as_str())
        .bind(last_known_version)
        .bind(served_version)
        .execute(self.pool())
        .await?;
        // Retention without a scheduler: every PRUNE_EVERY-th upsert through
        // this store also deletes this service's long-silent rows. One bounded
        // statement (the PK prefix is `service_id`), run on 1 of 64 polls, so
        // it stays off the p99 of the common path. Forged ids cannot pile up
        // unboundedly; a failed prune is only a missed cleanup.
        if self
            .download_ticks
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(PRUNE_EVERY)
        {
            if let Err(e) = self.prune_stale_plugin_downloads(service_id).await {
                tracing::warn!(error = %e, service_id, "could not prune stale plugin downloads");
            }
        }
        Ok(())
    }

    /// Delete this service's ledger rows not touched for [`RETENTION`]. Far
    /// older than the confirmation window, so no live hold can depend on them.
    pub async fn prune_stale_plugin_downloads(
        &self,
        service_id: i64,
    ) -> Result<u64, PolicyStoreError> {
        let r = sqlx::query(
            "DELETE FROM policy.ranger_plugin_downloads \
             WHERE service_id = $1 AND at < now() - ($2 * interval '1 second')",
        )
        .bind(service_id)
        .bind(RETENTION.as_secs_f64())
        .execute(self.pool())
        .await?;
        Ok(r.rows_affected())
    }
}

/// Does every plugin that polled `service_id`'s tags within `recent` hold tag
/// version `version`? `service_id` is the service the plugin POLLS: for Trino
/// that is its resource service, not the tag service.
pub async fn plugins_hold_tag_version(
    store: &PolicyStore,
    service_id: i64,
    version: i64,
    recent: Duration,
) -> Result<HoldState, PolicyStoreError> {
    plugins_hold_version(store, DownloadKind::Tags, service_id, version, recent).await
}

/// Does every plugin that polled `service_id`'s policies within `recent` hold
/// policy version `version`? The StarRocks plugin reports its
/// `lastKnownVersion` on each policy poll, against the profile's
/// `policy_version`, which moves only when a compiled body changes.
pub async fn plugins_hold_policy_version(
    store: &PolicyStore,
    service_id: i64,
    version: i64,
    recent: Duration,
) -> Result<HoldState, PolicyStoreError> {
    plugins_hold_version(store, DownloadKind::Policies, service_id, version, recent).await
}

async fn plugins_hold_version(
    store: &PolicyStore,
    kind: DownloadKind,
    service_id: i64,
    version: i64,
    recent: Duration,
) -> Result<HoldState, PolicyStoreError> {
    let rows: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT plugin_id, last_known_version FROM policy.ranger_plugin_downloads \
         WHERE service_id = $1 AND kind = $3 \
           AND at >= now() - ($2 * interval '1 second') \
         ORDER BY plugin_id",
    )
    .bind(service_id)
    .bind(recent.as_secs_f64())
    .bind(kind.as_str())
    .fetch_all(store.pool())
    .await?;
    if rows.is_empty() {
        return Ok(HoldState {
            confirmed: false,
            waiting_on: vec![NO_PLUGIN_POLLED.to_owned()],
        });
    }
    let waiting_on: Vec<String> = rows
        .into_iter()
        .filter(|(_, held)| !held.is_some_and(|h| h >= version))
        .map(|(id, _)| id)
        .collect();
    Ok(HoldState {
        confirmed: waiting_on.is_empty(),
        waiting_on,
    })
}

/// [`wait_for_versions`] over tag versions only, for the view hold. An empty
/// slice is a [`WaitError::Timeout`]: with nothing to wait on, nothing is
/// confirmed.
pub async fn wait_for_tag_version(
    store: &PolicyStore,
    required: &[(i64, i64)],
    deadline: Duration,
) -> Result<(), WaitError> {
    if required.is_empty() {
        return Err(WaitError::Timeout {
            waiting_on: vec![NO_PLUGIN_POLLED.to_owned()],
        });
    }
    wait_for_versions(store, required, &[], deadline).await
}

/// Poll every 500 ms until each pair of `tags` is confirmed by
/// [`plugins_hold_tag_version`] and each pair of `policies` by
/// [`plugins_hold_policy_version`], or `deadline` passes.
///
/// Each pair is `(polled service id, that service's own counter)`. Tag
/// counters are per service: `download_tags` serves the POLLED (resource)
/// service's own `tag_version`, and `bump_bound_tag_versions` bumps each
/// resource profile and each tag service as a separate counter. So a tag V
/// must be the counter `download_tags` serves for that resource service, never
/// the tag service's counter and never one number shared across services; a
/// policy V is that profile's `policy_version`. A V that is too low confirms
/// before the plugin has the change; too high never confirms.
///
/// An empty list is skipped: with both empty this returns at once. The
/// timeout's `waiting_on` lists plugin ids, prefixed `tags:` / `policies:`
/// only when one id is behind on both.
pub async fn wait_for_versions(
    store: &PolicyStore,
    tags: &[(i64, i64)],
    policies: &[(i64, i64)],
    deadline: Duration,
) -> Result<(), WaitError> {
    let end = tokio::time::Instant::now() + deadline;
    loop {
        let mut behind_tags = Vec::new();
        for &(id, version) in tags {
            let s = plugins_hold_tag_version(store, id, version, RECENT).await?;
            behind_tags.extend(s.waiting_on);
        }
        let mut behind_policies = Vec::new();
        for &(id, version) in policies {
            let s = plugins_hold_policy_version(store, id, version, RECENT).await?;
            behind_policies.extend(s.waiting_on);
        }
        if behind_tags.is_empty() && behind_policies.is_empty() {
            return Ok(());
        }
        let now = tokio::time::Instant::now();
        if now >= end {
            return Err(WaitError::Timeout {
                waiting_on: waiting_on(behind_tags, behind_policies),
            });
        }
        tokio::time::sleep(WAIT_STEP.min(end - now)).await;
    }
}

/// The ids behind on tags and on policies, deduplicated, an id behind on both
/// listed once per kind with its kind as a prefix.
fn waiting_on(mut tags: Vec<String>, mut policies: Vec<String>) -> Vec<String> {
    tags.sort();
    tags.dedup();
    policies.sort();
    policies.dedup();
    let mut out = Vec::new();
    for id in &tags {
        if policies.contains(id) {
            out.push(format!("tags:{id}"));
        } else {
            out.push(id.clone());
        }
    }
    for id in &policies {
        if tags.contains(id) {
            out.push(format!("policies:{id}"));
        } else {
            out.push(id.clone());
        }
    }
    out
}
