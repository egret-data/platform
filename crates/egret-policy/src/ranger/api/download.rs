//! Policy download + tags download endpoints (Ranger plugin polling protocol).

use crate::plugin_downloads::DownloadKind;
use crate::ranger::auth::Caller;
use crate::ranger::dto::{ServicePolicies, ServiceTags, TagAssignment};
use crate::ranger::error::RangerError;
use crate::ranger::routes::{
    RangerErrorBody, RangerServicePolicies, RangerServiceTags, RangerState,
};
use crate::PolicyStore;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DownloadParams {
    /// The plugin's cached policy version; a match yields `304 Not Modified`.
    #[serde(rename = "lastKnownVersion")]
    pub last_known_version: Option<i64>,
}

/// The plugin's identity on a poll (`<appId>@<host>-<service>`). Read apart
/// from [`DownloadParams`] so the documented query surface is unchanged.
#[derive(Deserialize)]
pub struct PluginParams {
    #[serde(rename = "pluginId")]
    pub plugin_id: Option<String>,
}

/// Longest `pluginId` recorded (`<appId>@<host>-<service>` is far shorter).
const MAX_PLUGIN_ID_BYTES: usize = 256;

/// Record this poll in the plugin download ledger (view governance B4).
///
/// Never changes the download: a poll with no `pluginId` records nothing, and a
/// ledger error is logged and swallowed. The caller has already resolved the
/// service, so an unknown service never reaches here.
async fn record_poll(
    store: &PolicyStore,
    service_id: i64,
    plugin_id: Option<&str>,
    kind: DownloadKind,
    last_known: Option<i64>,
    served: Option<i64>,
) {
    let Some(plugin_id) = plugin_id.filter(|p| !p.is_empty()) else {
        return;
    };
    if plugin_id.len() > MAX_PLUGIN_ID_BYTES {
        // Never log the raw value: the endpoint is anonymous, and an id can be
        // as long as the URI limit.
        tracing::warn!(
            service_id,
            id_len = plugin_id.len(),
            id_prefix = %String::from_utf8_lossy(&plugin_id.as_bytes()[..64]),
            "plugin id over {MAX_PLUGIN_ID_BYTES} bytes; not recorded"
        );
        return;
    }
    if let Err(e) = store
        .record_plugin_download(service_id, plugin_id, kind, last_known, served)
        .await
    {
        tracing::warn!(
            error = %e,
            service_id,
            plugin_id,
            "could not record a plugin download; serving the download anyway"
        );
    }
}

/// 403 unless `caller` may read `svc`: an engine credential reads only the profile its engine is bound to,
/// plus the tag service that profile names. Operators and anonymous-mode callers read anything.
/// An engine let through is recorded as having downloaded (A12).
async fn authorize_service(
    store: &PolicyStore,
    caller: &Caller,
    svc: &crate::ServiceRow,
) -> Result<(), RangerError> {
    let Caller::Engine(id) = caller else {
        return Ok(());
    };
    let Some(bound) = id.authz_service_id else {
        tracing::warn!(engine_id = %id.engine_id, requested = %svc.name, "engine credential bound to no profile asked for a service");
        return Err(RangerError::forbidden());
    };
    if bound == svc.id || store.tag_service_of(bound).await? == Some(svc.id) {
        // Clears an outstanding `engine_auth_failing` (A12); off the request path.
        crate::auth_failures::note_download(store, id.engine_id);
        return Ok(());
    }
    tracing::warn!(engine_id = %id.engine_id, requested = %svc.name, "engine credential asked for a service it is not bound to");
    Err(RangerError::forbidden())
}

/// `GET /plugins/policies/download/{service}?lastKnownVersion=N`
///
/// Returns 304 when the plugin's cached version matches; 200 + `ServicePolicies` otherwise.
#[utoipa::path(
    get,
    path = "/ranger/service/plugins/policies/download/{service}",
    tag = "ranger",
    params(
        ("service" = String, Path, description = "Policy profile (Ranger service) name"),
        DownloadParams,
    ),
    responses(
        (status = 200, description = "The service's enabled policies + service-def (plugin download envelope)", body = RangerServicePolicies),
        (status = 304, description = "Not modified — the plugin's lastKnownVersion matches the current policy version"),
        (status = 401, description = "Missing or invalid credential (engine Basic or operator bearer) while `ranger.auth_mode` is `required`"),
        (status = 403, description = "An engine credential asked for a profile its engine is not bound to", body = RangerErrorBody),
        (status = 404, description = "No such policy profile", body = RangerErrorBody),
        (status = 500, description = "Internal error", body = RangerErrorBody),
    ),
)]
pub async fn download_policies(
    State(s): State<RangerState>,
    Extension(caller): Extension<Caller>,
    Path(service): Path<String>,
    Query(q): Query<DownloadParams>,
    Query(p): Query<PluginParams>,
) -> Result<Response, RangerError> {
    let svc = s.store.get_service(&service).await?; // 404 if unknown
    authorize_service(&s.store, &caller, &svc).await?;
    let not_modified = q.last_known_version == Some(svc.policy_version);
    record_poll(
        &s.store,
        svc.id,
        p.plugin_id.as_deref(),
        DownloadKind::Policies,
        q.last_known_version,
        (!not_modified).then_some(svc.policy_version),
    )
    .await;
    if not_modified {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }
    let def = s.store.get_service_def(&svc.r#type).await?.def_jsonb;
    let policies = s.store.enabled_policies_for_service(svc.id).await?;
    // The tag service this resource service is linked to (trino profiles carry
    // `tagService = <name>_tag`), captured before `svc.name` is moved below.
    let tag_service = svc
        .config_jsonb
        .get(egret_policy_api::TAG_SERVICE_CONFIG_KEY)
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let service_name = svc.name.clone();
    let mut sp = ServicePolicies::new(svc.name, svc.id, svc.policy_version, def, policies, None);
    // Embed the `tag` service-def + the tag service's policies as `tagPolicies` so the Trino
    // apache-ranger plugin turns on tag-based masking/row-filter evaluation and
    // its tag enricher. Without this, tag policies download fine on their own
    // service but the resource plugin never consults them → masking silently
    // no-ops.
    //
    // **SP4d debt Task 9, step 3b — this used to be `if let Ok(tsvc)`, and that
    // swallow was the third mechanism behind the same silence.** It collapsed
    // two different outcomes into one unlogged omission:
    //
    // * a `tagService` naming no profile — the shape `trigger.rs` already
    //   WARNS about (`"profile's tagService names no policy profile"`) and
    //   `usable_tag_service` already refuses an engine bind for. Omitting the
    //   block is still the right answer (a 404 here would strip the plugin of
    //   its RESOURCE policies too, and a plugin that cannot download falls back
    //   to its last cache or to deny-all), but it has to be as loud here as it
    //   is there, or the one surface the engine actually polls is the only one
    //   that says nothing.
    // * **a database error** — which is a FAIL-OPEN, not a misconfiguration: a
    //   transient failure on this one read served a complete-looking download
    //   with no `tagPolicies` block, and the plugin caches that until its next
    //   poll, so every tag mask and row filter is off in the meantime and
    //   nothing anywhere records why. The two reads just below it
    //   (`get_service_def`, `enabled_policies_for_service`) already propagate,
    //   so the swallow was also inconsistent with its own block.
    //
    // Now: `NotFound` warns and omits (the documented best-effort, made
    // audible); anything else propagates like every other read on this route.
    //
    // **⚠ Do NOT collapse this back to `if let Ok(tsvc)`.** The `Err(e)` arm is
    // reached only on a database error, so it looks dead and a future reader
    // will be tempted — that is exactly how the swallow got here. What that arm
    // buys: a transient failure on this ONE read used to serve a
    // complete-looking download with no `tagPolicies` block, which the plugin
    // then **cached** until its next poll, so every tag-addressed mask and row
    // filter was off in the meantime and nothing anywhere recorded why. A
    // fail-open on an error, not a misconfiguration. The `match` is what keeps
    // the two causes distinguishable, and keeps `cargo check` forcing a decision
    // if a new `PolicyStoreError` variant appears.
    if let Some(name) = tag_service {
        match s.store.get_service(&name).await {
            Ok(tsvc) => {
                // The `tag` service-def, NEVER `tsvc.r#type`'s. Real Ranger Admin
                // serves this block with the tag def, and the plugin relies on it:
                // its candidate filter for a tag request is a trie per resource of
                // THIS def, so a def with no `tag` resource (`trino.json` — the
                // type every tag service made through the admin API has) builds no
                // trie, stops narrowing, and applies every tag policy to every
                // tagged column. Measured live on Trino 483 / plugin 2.5.0 and in
                // `tests/ranger/src/test/java/EgretTagBlockServiceDefTest.java`.
                // `egret_perm_compile::routes::artifact_via` mirrors this line.
                let tdef = s
                    .store
                    .get_service_def(crate::TAG_DEF_NAME)
                    .await?
                    .def_jsonb;
                let tpolicies = s.store.enabled_policies_for_service(tsvc.id).await?;
                sp.tag_policies = Some(Box::new(ServicePolicies::new(
                    tsvc.name,
                    tsvc.id,
                    tsvc.policy_version,
                    tdef,
                    tpolicies,
                    None,
                )));
            }
            Err(crate::PolicyStoreError::NotFound(_)) => {
                tracing::warn!(
                    service = %service_name,
                    tag_service = %name,
                    "profile's `tagService` names no policy profile — serving this download \
                     with NO `tagPolicies` block, so every tag-addressed mask and row filter \
                     is inert for this engine"
                );
            }
            Err(e) => return Err(e.into()),
        }
    }
    // View governance R5: decided per POLLER (fix round 2, N1). Only a pod of
    // a Trino engine egret provisions in JWT mode, bound to this profile, is
    // served the `egret-lineage` transform; every other poller gets the stored
    // policies (`crate::lineage_transform::lineage_catalogs_for_poll_via`).
    let mut conn = s
        .store
        .pool()
        .acquire()
        .await
        .map_err(crate::PolicyStoreError::from)?;
    if let Some(catalogs) = crate::lineage_transform::lineage_catalogs_for_poll_via(
        &mut conn,
        sp.service_id,
        &service_name,
        p.plugin_id.as_deref(),
    )
    .await?
    {
        crate::lineage_transform::apply_lineage_transform(&mut sp, &catalogs);
    }
    Ok(Json(sp).into_response())
}

/// `GET /tags/download/{service}?lastKnownVersion=N`
///
/// Serves the real resource→tag mapping (`catalog_tag_assignments`) for the
/// service's warehouse(s) as a Ranger `ServiceTags`. Returns 304 when the
/// plugin's cached `tagVersion` matches the current one; 200 otherwise.
#[utoipa::path(
    get,
    path = "/ranger/service/tags/download/{service}",
    tag = "ranger",
    params(
        ("service" = String, Path, description = "Policy profile (Ranger service) name"),
        DownloadParams,
    ),
    responses(
        (status = 200, description = "The service's catalog tag assignments (plugin tag-download envelope)", body = RangerServiceTags),
        (status = 304, description = "Not modified — the plugin's lastKnownVersion matches the current tag version"),
        (status = 401, description = "Missing or invalid credential (engine Basic or operator bearer) while `ranger.auth_mode` is `required`"),
        (status = 403, description = "An engine credential asked for a profile its engine is not bound to", body = RangerErrorBody),
        (status = 404, description = "No such policy profile", body = RangerErrorBody),
        (status = 500, description = "Internal error", body = RangerErrorBody),
    ),
)]
pub async fn download_tags(
    State(s): State<RangerState>,
    Extension(caller): Extension<Caller>,
    Path(service): Path<String>,
    Query(q): Query<DownloadParams>,
    Query(p): Query<PluginParams>,
) -> Result<Response, RangerError> {
    let svc = s.store.get_service(&service).await?; // 404 if unknown
    authorize_service(&s.store, &caller, &svc).await?;
    let not_modified = q.last_known_version == Some(svc.tag_version);
    // Keyed by the service the poll names: Trino polls tags under its RESOURCE
    // service, not under the tag service.
    record_poll(
        &s.store,
        svc.id,
        p.plugin_id.as_deref(),
        DownloadKind::Tags,
        q.last_known_version,
        (!not_modified).then_some(svc.tag_version),
    )
    .await;
    if not_modified {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }
    let assignments = s
        .store
        .tag_assignments_for_service(svc.id)
        .await?
        .into_iter()
        .map(|r| {
            TagAssignment::new(
                r.catalog,
                r.schema_name,
                r.table_name,
                r.column_name,
                r.tag_id,
                r.tag_name,
            )
        })
        .collect();
    let tags = ServiceTags::from_assignments(svc.name, svc.tag_version, assignments);
    Ok(Json(tags).into_response())
}
