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
use std::time::Duration;

/// `waiting_on` when no plugin has polled the service within the window.
/// Confirmation is never vacuous.
pub const NO_PLUGIN_POLLED: &str = "<no plugin has polled>";

const WAIT_STEP: Duration = Duration::from_millis(500);
/// A plugin that has not polled within three 30 s poll intervals is not waited
/// on (spec B4). Used by `wait_for_tag_version`.
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
    #[error("plugins did not confirm the tag version in time; waiting on {waiting_on:?}")]
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
        Ok(())
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
    let rows: Vec<(String, Option<i64>)> = sqlx::query_as(
        "SELECT plugin_id, last_known_version FROM policy.ranger_plugin_downloads \
         WHERE service_id = $1 AND kind = 'tags' \
           AND at >= now() - ($2 * interval '1 second') \
         ORDER BY plugin_id",
    )
    .bind(service_id)
    .bind(recent.as_secs_f64())
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

/// Poll [`plugins_hold_tag_version`] every 500 ms until every service in
/// `service_ids` is confirmed, or `deadline` passes. `service_ids` are the
/// RESOURCE services of the Trino engines bound to the affected tag service.
/// An empty list is a [`WaitError::Timeout`]: with nothing to wait on, nothing
/// is confirmed.
pub async fn wait_for_tag_version(
    store: &PolicyStore,
    service_ids: &[i64],
    version: i64,
    deadline: Duration,
) -> Result<(), WaitError> {
    let end = tokio::time::Instant::now() + deadline;
    loop {
        let mut waiting_on = Vec::new();
        for &id in service_ids {
            let s = plugins_hold_tag_version(store, id, version, RECENT).await?;
            if !s.confirmed {
                waiting_on.extend(s.waiting_on);
            }
        }
        if service_ids.is_empty() {
            waiting_on.push(NO_PLUGIN_POLLED.to_owned());
        }
        if waiting_on.is_empty() {
            return Ok(());
        }
        let now = tokio::time::Instant::now();
        if now >= end {
            return Err(WaitError::Timeout { waiting_on });
        }
        tokio::time::sleep(WAIT_STEP.min(end - now)).await;
    }
}
