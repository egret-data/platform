//! An external engine: a row egret holds for a cluster it did not provision and
//! never touches (SP4d-4 Part C, Task 7).
//!
//! It exists so that a policy profile can be bound to it (`authz_service_id`),
//! so the perm compiler compiles grants for the catalogs it serves and the
//! equivalence gate knows its plugin version; and so registry catalogs can be
//! **declared** attached to it, which is what makes
//! `egret_perm_compile::affected_services_for_grant` reach its profile. Before
//! this, SP4d-3's live check had to `INSERT` the row and the attachment by hand.
//!
//! **Out of scope, by ruling:** workspace assignment of an external engine and
//! credentials of any kind. Egret stores no secret for it and never connects to
//! it.
//!
//! **The row's shape is the database's** (`engines_external_shape`, migration
//! 0184): an external row carries an address and a declared plugin version and
//! none of the provisioning state; a provisioned row carries none of the
//! external fields. **The attachment's half is this module's**: that a
//! `'declared'` attachment belongs to an external engine (and only it) is a
//! cross-table fact no CHECK can state, so [`declare_attachment_in_tx`] is the
//! only writer of one, and a test asserts the agreement across the whole table.
//!
//! **The plugin version egret can PROVE a migration on is not the version an
//! operator may DECLARE.** Registration accepts any Ranger release in
//! [`KNOWN_RANGER_PLUGIN_VERSIONS`] and says, on the response, whether egret's
//! evaluator models the (type, version) PAIR (`rangerPluginVersionModelled`; only
//! Trino@2.5.0 and StarRocks@2.8.0 are). A pair the evaluator
//! does not model is stored as declared — recording a different one would be
//! wrong — and `egret_perm_compile::engine_plugin_version` then makes the SP4d-3
//! migration gate refuse on that profile, naming the version. It is never
//! evaluated at a guessed one.

use axum::extract::Path;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_core::http::ErrorBody;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::engines::EngineDto;
use crate::{Engine, EngineHttpError as CpError, EngineMgmtState as CpState};

pub const ORIGIN_EXTERNAL: &str = "external";
pub const ORIGIN_PROVISIONED: &str = "provisioned";

/// The Ranger releases an operator may declare as installed on an external
/// engine. A closed list so a typo (`2.55.0`) is a 400 rather than a profile
/// that is quietly never provable. A release absent here is a deliberate edit,
/// not a guess.
pub const KNOWN_RANGER_PLUGIN_VERSIONS: [&str; 9] = [
    "2.0.0", "2.1.0", "2.2.0", "2.3.0", "2.4.0", "2.5.0", "2.6.0", "2.7.0", "2.8.0",
];

/// Whether egret's Ranger evaluator has been read against `engine_type` at
/// `version` (a PAIR: Trino@2.5.0 and StarRocks@2.8.0 only). The same predicate
/// the migration gate and `explain` use.
pub fn plugin_version_is_modelled(engine_type: &str, version: &str) -> bool {
    egret_perm::ranger_plugin_is_modelled(engine_type, version)
}

pub fn is_external(e: &Engine) -> bool {
    e.origin == ORIGIN_EXTERNAL
}

/// The 409 every lifecycle handler answers for an external engine (Task 8):
/// restart, rotate-secrets, pod logs and exec, the monitor, scaling — anything
/// that would reach a cluster egret does not run.
pub fn refuse_if_external(e: &Engine, action: &str) -> Result<(), CpError> {
    if is_external(e) {
        return Err(CpError::EngineIsExternal(format!(
            "engine {} is external: egret did not provision it and never touches it, so it \
             cannot {action}",
            e.name
        )));
    }
    Ok(())
}

/// `POST /api/control-plane/engines/external`.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegisterExternalEngine {
    /// Unique across engines. Lower-case letters, digits and `-`, 1 to 63
    /// characters, starting and ending alphanumeric.
    pub name: String,
    /// `trino` or `starrocks`.
    pub r#type: String,
    /// Host or IP the engine answers on: no scheme, no port, no path.
    pub host: String,
    /// Trino's HTTP port or StarRocks' FE query port.
    pub port: u16,
    /// The Ranger plugin version installed on the engine, `x.y.z`, one of the
    /// Ranger 2.x releases. **Only Trino@2.5.0 and StarRocks@2.8.0 are
    /// modelled by egret's evaluator** (a type and version PAIR): any other is
    /// stored as declared, and a migration to a profile bound to this engine is
    /// then refused rather than evaluated with the wrong semantics.
    pub ranger_plugin_version: String,
    /// A policy profile to bind. Its type must equal `type`, no other engine may
    /// bind it, and a Trino profile needs a usable `tagService`.
    #[serde(default)]
    pub authz_service_id: Option<i64>,
    /// Registry catalogs to declare attached. Nothing is applied to the engine.
    #[serde(default)]
    pub catalogs: Vec<Uuid>,
}

/// `PATCH /api/control-plane/engines/{id}/external`: every field optional;
/// an absent field is left as it is.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpdateExternalEngine {
    /// The policy profile to bind: absent leaves the binding, `null` unbinds,
    /// a number binds (same rules as at registration).
    #[serde(default, deserialize_with = "present_or_null")]
    #[schema(value_type = Option<i64>, nullable)]
    pub authz_service_id: Option<Option<i64>>,
    /// New host (bare host: no scheme, port or path).
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    /// The Ranger plugin version installed on the engine, as at registration.
    /// The response says whether egret's evaluator models the (type, version)
    /// pair.
    #[serde(default)]
    pub ranger_plugin_version: Option<String>,
}

/// `Some(None)` for an explicit `null`, `Some(Some(v))` for a value; with
/// `#[serde(default)]` an absent key stays `None`.
fn present_or_null<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<Option<i64>>, D::Error> {
    Option::<i64>::deserialize(d).map(Some)
}

/// The OpenAPI fragment of the external-engine routes, merged by
/// [`crate::openapi`].
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(register_external_engine, update_external_engine, ranger_plugin_config),
    components(schemas(
        RegisterExternalEngine,
        UpdateExternalEngine,
        RangerPluginConfigDto,
        RangerPluginFileDto
    ))
)]
pub struct ExternalEngineApi;

pub fn routes() -> Router<CpState> {
    Router::new()
        .route("/engines/external", post(register_external_engine))
        .route("/engines/{id}/external", patch(update_external_engine))
        .route(
            "/engines/{id}/ranger-plugin-config",
            get(ranger_plugin_config),
        )
}

fn validate_name(name: &str) -> Result<(), CpError> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    if ok {
        Ok(())
    } else {
        Err(CpError::BadRequest(
            "name must be 1 to 63 lower-case letters, digits or `-`, starting and ending with \
             a letter or digit"
                .into(),
        ))
    }
}

fn validate_host(host: &str) -> Result<(), CpError> {
    let ok = !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(CpError::BadRequest(
            "host must be a bare host name or IP address: no scheme, port, path or spaces".into(),
        ))
    }
}

fn validate_plugin_version(version: &str) -> Result<(), CpError> {
    if KNOWN_RANGER_PLUGIN_VERSIONS.contains(&version) {
        Ok(())
    } else {
        Err(CpError::BadRequest(format!(
            "rangerPluginVersion {version:?} is not a Ranger 2.x release egret knows; one of \
             {}. Only {} are modelled by egret's evaluator: any other (type, version) pair is \
             accepted as declared, and a migration to its profile is then refused, never \
             evaluated with the wrong semantics",
            KNOWN_RANGER_PLUGIN_VERSIONS.join(", "),
            egret_perm::modelled_ranger_plugins_sentence(),
        )))
    }
}

/// The one writer of a `'declared'` attachment. Refuses unless `engine` is
/// external, so a declared attachment on a provisioned engine is unrepresentable
/// through this crate (the cross-table half of what migration 0184 cannot say).
pub(crate) async fn declare_attachment_in_tx(
    st: &CpState,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    engine: &Engine,
    catalog_id: Uuid,
) -> Result<(), CpError> {
    if !is_external(engine) {
        return Err(CpError::Internal);
    }
    st.engine_mgmt
        .registry()
        .insert_attachment_in_tx(
            tx,
            &egret_catalog_registry::store::NewAttachment {
                engine_id: engine.id,
                catalog_id,
                catalog_client_id: None,
                credential_secret_id: None,
                minted_client: false,
                applied_now: false,
                is_primary: false,
                created_at: None,
                apply_status: egret_catalog_registry::store::apply_status::DECLARED,
            },
        )
        .await?;
    Ok(())
}

/// Declare `catalog` attached to the external `engine` and recompile the
/// profile it binds, in ONE transaction that refuses a pair the attachment
/// creates (the before-snapshot is taken before the row is written).
pub(crate) async fn declare_attached_and_recompile(
    st: &CpState,
    engine: &Engine,
    catalog: &egret_catalog_registry::store::CatalogRow,
) -> Result<(), CpError> {
    let def = catalog
        .def()
        .map_err(|e| CpError::BadRequest(format!("catalog {}: {e}", catalog.name)))?;
    crate::catalog_apply::check_engine_supports(engine, &catalog.name, &def).map_err(
        |e| match e {
            crate::catalog_apply::CatalogOpError::Http(e) => e,
            crate::catalog_apply::CatalogOpError::Apply(_) => CpError::Internal,
        },
    )?;
    let mut tx = st
        .engine_mgmt
        .pool()
        .begin()
        .await
        .map_err(crate::http_error::db_error)?;
    let before =
        crate::profile_compiler::snapshot_overlaps(st, &mut tx, engine.authz_service_id).await?;
    declare_attachment_in_tx(st, &mut tx, engine, catalog.id).await?;
    if let Some(sid) = engine.authz_service_id {
        crate::profile_compiler::recompile_refusing_created_overlaps(
            st,
            &mut tx,
            sid,
            before.as_ref(),
        )
        .await?;
    }
    tx.commit().await.map_err(crate::http_error::db_error)?;
    Ok(())
}

/// Remove the declared attachment of `catalog` from the external `engine` and
/// recompile the profile it binds, in ONE transaction: egret's rows only, no
/// DDL, no render, no credential to retire (a declared attachment holds none).
/// Takes the same before-snapshot as the declare path so the two cannot drift;
/// a detach can only remove pairs, so the refusal never fires on it today.
pub(crate) async fn detach_declared_and_recompile(
    st: &CpState,
    engine: &Engine,
    catalog: &egret_catalog_registry::store::CatalogRow,
) -> Result<(), CpError> {
    if !is_external(engine) {
        return Err(CpError::Internal);
    }
    let registry = st.engine_mgmt.registry();
    registry
        .get_attachment(engine.id, catalog.id)
        .await?
        .ok_or(CpError::NotFound)?;
    let mut tx = st
        .engine_mgmt
        .pool()
        .begin()
        .await
        .map_err(crate::http_error::db_error)?;
    let before =
        crate::profile_compiler::snapshot_overlaps(st, &mut tx, engine.authz_service_id).await?;
    if !registry
        .delete_attachment_in_tx(&mut tx, engine.id, catalog.id)
        .await?
    {
        return Err(CpError::NotFound);
    }
    if let Some(sid) = engine.authz_service_id {
        crate::profile_compiler::recompile_refusing_created_overlaps(
            st,
            &mut tx,
            sid,
            before.as_ref(),
        )
        .await?;
    }
    tx.commit().await.map_err(crate::http_error::db_error)?;
    Ok(())
}

/// `PATCH /api/control-plane/engines/{id}/external`.
#[utoipa::path(
    patch,
    path = "/api/control-plane/engines/{id}/external",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    request_body = UpdateExternalEngine,
    responses(
        (status = 200, description = "Updated. Egret's row only: nothing is sent to the engine, a provisioner or Kubernetes. The policy profile it was or is bound to were recompiled in the same transaction. `rangerPluginVersionModelled` is as at registration", body = EngineDto),
        (status = 400, description = "The engine is not external; nothing to update; invalid host, port or plugin version; or `authzServiceId` naming no profile, one of the wrong type, or a Trino profile with no usable tag service", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:update permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 409, description = "The engine changed concurrently (e.g. a delete); the profile is bound to another engine; or the change would create an order-ambiguous mask or row-filter pair (`code: rule_overlap`). Nothing was written", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update_external_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(b): Json<UpdateExternalEngine>,
) -> Result<Json<EngineDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:update").await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    if !is_external(&engine) {
        return Err(CpError::BadRequest(format!(
            "engine {} is provisioned: update it with PUT /api/control-plane/engines/{{id}}",
            engine.name
        )));
    }
    if b.authz_service_id.is_none()
        && b.host.is_none()
        && b.port.is_none()
        && b.ranger_plugin_version.is_none()
    {
        return Err(CpError::BadRequest(
            "nothing to update: send authzServiceId, host, port or rangerPluginVersion".into(),
        ));
    }
    if let Some(host) = &b.host {
        validate_host(host)?;
    }
    if b.port == Some(0) {
        return Err(CpError::BadRequest("port must be 1 to 65535".into()));
    }
    if let Some(v) = &b.ranger_plugin_version {
        validate_plugin_version(v)?;
    }
    let old_profile = engine.authz_service_id;
    let new_profile = b.authz_service_id.unwrap_or(old_profile);
    if let Some(sid) = new_profile {
        if Some(sid) != old_profile {
            crate::engines::check_profile_bindable(&st, sid, &engine.r#type, &engine.name).await?;
        }
    }
    let host = b.host.clone().or(engine.external_host.clone());
    let port = b.port.map(i32::from).or(engine.external_port);
    let version = b
        .ranger_plugin_version
        .clone()
        .or(engine.ranger_plugin_version.clone());

    let mut tx = st
        .engine_mgmt
        .pool()
        .begin()
        .await
        .map_err(crate::http_error::db_error)?;
    // Taken BEFORE the write: a rebind changes which catalogs, hence which tag
    // assignments, the new profile's unit resolves.
    let touched: Vec<i64> = new_profile.into_iter().chain(old_profile).collect();
    let before = crate::profile_compiler::snapshot_overlaps_of(&st, &mut tx, &touched).await?;
    let updated = sqlx::query_as::<_, Engine>(
        "UPDATE engine_mgmt.engines \
            SET external_host = $2, external_port = $3, ranger_plugin_version = $4, \
                authz_service_id = $5 \
          WHERE id = $1 AND origin = 'external' AND status = $6 \
            AND authz_service_id IS NOT DISTINCT FROM $7 \
        RETURNING id, name, type, namespace, spec, status, authz_service_id, delegator_secret_id, root_secret_id, tls_secret_id, coordinator_cert, tls_cert_secret_ref, internal_secret_id, monitor_secret_id, created_at, origin, external_host, external_port, ranger_plugin_version",
    )
    .bind(engine.id)
    .bind(&host)
    .bind(port)
    .bind(&version)
    .bind(new_profile)
    .bind(&engine.status)
    .bind(old_profile)
    .fetch_optional(&mut *tx)
    .await
    .map_err(crate::http_error::db_error)?
    .ok_or_else(|| {
        CpError::Conflict("engine changed while it was being updated; retry".into())
    })?;
    // The new profile first, then the one it replaced (it lost this engine's
    // catalogs). Each refuses a pair the change created.
    for sid in touched.iter().copied() {
        crate::profile_compiler::recompile_refusing_created_overlaps(
            &st,
            &mut tx,
            sid,
            before.as_ref(),
        )
        .await?;
    }
    tx.commit().await.map_err(crate::http_error::db_error)?;
    tracing::info!(engine = %updated.name, "external engine updated; nothing was sent to the cluster");
    Ok(Json(updated.into()))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/engines/external",
    tag = "control-plane-engines",
    request_body = RegisterExternalEngine,
    responses(
        (status = 201, description = "Registered. Nothing was provisioned: egret holds a row (`origin: external`), optionally bound to a policy profile, with the given catalogs declared attached (`applyStatus: declared`). The bound profile was recompiled in the same transaction, so its download carries the catalogs' grants at once. `rangerPluginVersionModelled` is false when egret's evaluator has not been read against the (type, declared version) pair (only Trino@2.5.0 and StarRocks@2.8.0 are): a migration to that profile is then refused, never evaluated at a guessed version", body = EngineDto),
        (status = 400, description = "Invalid name, type, host, port or plugin version; a catalog the engine cannot read; `authzServiceId` naming no policy profile or a Trino profile with no usable tag service; or Kubernetes-free registration was malformed", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:create permission", body = ErrorBody),
        (status = 404, description = "A catalog id in `catalogs` names no registry catalog (the message names it); nothing was written", body = ErrorBody),
        (status = 409, description = "The name is taken, the profile has another type or is bound to another engine, or binding it would create an order-ambiguous mask or row-filter pair (`code: rule_overlap`); nothing was written", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn register_external_engine(
    State(st): State<CpState>,
    Auth(p): Auth,
    Json(b): Json<RegisterExternalEngine>,
) -> Result<(StatusCode, Json<EngineDto>), CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:create").await?;
    validate_name(&b.name)?;
    if b.r#type != "trino" && b.r#type != "starrocks" {
        return Err(CpError::BadRequest(
            "type must be `trino` or `starrocks`: those are the engines a Ranger plugin governs \
             here"
                .into(),
        ));
    }
    validate_host(&b.host)?;
    if b.port == 0 {
        return Err(CpError::BadRequest("port must be 1 to 65535".into()));
    }
    validate_plugin_version(&b.ranger_plugin_version)?;
    if let Some(sid) = b.authz_service_id {
        crate::engines::check_profile_bindable(&st, sid, &b.r#type, &b.name).await?;
    }
    // Every catalog resolved BEFORE anything is written, so an unknown id is a
    // 404 that names it and leaves no row behind.
    let registry = st.engine_mgmt.registry();
    let mut catalogs: Vec<egret_catalog_registry::store::CatalogRow> =
        Vec::with_capacity(b.catalogs.len());
    for id in &b.catalogs {
        if catalogs.iter().any(|c| c.id == *id) {
            return Err(CpError::BadRequest(format!("catalog {id} is listed twice")));
        }
        let catalog = registry
            .get_catalog(*id)
            .await?
            .ok_or_else(|| CpError::NotFoundNamed(format!("no registry catalog has id {id}")))?;
        let def = catalog
            .def()
            .map_err(|e| CpError::BadRequest(format!("catalog {}: {e}", catalog.name)))?;
        crate::catalog_apply::check_engine_supports_type(&b.r#type, &catalog.name, &def)?;
        catalogs.push(catalog);
    }

    // ONE transaction: the row, the bind, the declared attachments, the
    // recompile and the overlap refusal commit together or not at all. A
    // registration that fails halfway leaves no engine claiming a profile the
    // compiler never filled.
    let mut tx = st
        .engine_mgmt
        .pool()
        .begin()
        .await
        .map_err(crate::http_error::db_error)?;
    let mut engine = insert_external_row_in_tx(&mut tx, &b).await?;
    // Taken after the row exists and BEFORE the bind and the attachments: they
    // are what change which catalogs, hence which tag assignments, the profile's
    // unit resolves, so a pair they complete must not read as already stored.
    let before =
        crate::profile_compiler::snapshot_overlaps(&st, &mut tx, b.authz_service_id).await?;
    if let Some(sid) = b.authz_service_id {
        if !st
            .engine_mgmt
            .set_engine_authz_service_in_tx(&mut tx, engine.id, Some(sid), &engine.status)
            .await?
        {
            return Err(CpError::Conflict(
                "engine changed while it was being registered; retry".into(),
            ));
        }
        engine.authz_service_id = Some(sid);
    }
    for catalog in &catalogs {
        declare_attachment_in_tx(&st, &mut tx, &engine, catalog.id).await?;
    }
    if let Some(sid) = b.authz_service_id {
        crate::profile_compiler::recompile_refusing_created_overlaps(
            &st,
            &mut tx,
            sid,
            before.as_ref(),
        )
        .await?;
    }
    tx.commit().await.map_err(crate::http_error::db_error)?;
    tracing::info!(
        engine = %engine.name,
        engine_type = %engine.r#type,
        service_id = ?engine.authz_service_id,
        catalogs = catalogs.len(),
        "external engine registered; nothing was provisioned"
    );
    Ok((StatusCode::CREATED, Json(engine.into())))
}

async fn insert_external_row_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    b: &RegisterExternalEngine,
) -> Result<Engine, CpError> {
    sqlx::query_as::<_, Engine>(
        "INSERT INTO engine_mgmt.engines \
           (name, type, namespace, spec, status, origin, external_host, external_port, \
            ranger_plugin_version) \
         VALUES ($1, $2, '', '{}'::jsonb, 'running', 'external', $3, $4, $5) \
         RETURNING id, name, type, namespace, spec, status, authz_service_id, delegator_secret_id, root_secret_id, tls_secret_id, coordinator_cert, tls_cert_secret_ref, internal_secret_id, monitor_secret_id, created_at, origin, external_host, external_port, ranger_plugin_version",
    )
    .bind(&b.name)
    .bind(&b.r#type)
    .bind(&b.host)
    .bind(i32::from(b.port))
    .bind(&b.ranger_plugin_version)
    .fetch_one(&mut **tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.constraint() == Some("engines_name_key") => {
            CpError::AlreadyExists(format!("an engine named {} already exists", b.name))
        }
        _ => crate::http_error::db_error(e),
    })
}

/// One file the admin of an external engine installs.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RangerPluginFileDto {
    /// File name; for StarRocks `fe.conf (add this line)` is a line to add to
    /// the FE's existing `fe.conf`, not a file to replace it with.
    pub name: String,
    pub content: String,
}

/// The Ranger plugin configuration an external engine's admin installs.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RangerPluginConfigDto {
    /// The bound policy profile: the Ranger service name the plugin asks for.
    pub service_name: String,
    /// `{public_url}/service/plugins/policies/download/{serviceName}`: where the
    /// plugin's download lands. The files below carry `public_url` itself (the
    /// plugin appends the path).
    pub download_url: String,
    /// `anonymous`, or `bearer-configured` (egret requires a bearer token on
    /// `/service`; the token is never shown here).
    pub auth_mode: String,
    /// The plugin version the engine was DECLARED to run.
    pub ranger_plugin_version: Option<String>,
    /// Whether egret's evaluator models the (type, version) pair. When `false`,
    /// migrations into this profile are refused.
    pub ranger_plugin_version_modelled: bool,
    /// Things that make the install not work, or not safe, as configured.
    pub warnings: Vec<String>,
    /// What egret's `explain` and migration gate assume about the plugin and
    /// cannot check on an external engine.
    pub assumptions: Vec<String>,
    /// What the admin must supply themselves. egret renders no credential and
    /// invents none.
    pub admin_must_supply: Vec<String>,
    pub files: Vec<RangerPluginFileDto>,
}

const IN_CLUSTER_DEFAULT_URL: &str = "http://egret:8080";

/// Everything in the response that is a judgement about the deployment, as a
/// pure function of the facts (so each branch is tested without a server).
struct PluginConfigFacts<'a> {
    engine_type: &'a str,
    public_url: &'a str,
    bearer_configured: bool,
    ranger_enabled: bool,
    version: Option<&'a str>,
}

/// Whether `raw` parses as a URL whose host is `localhost`, an address in
/// `127.0.0.0/8` or `::1`. Parsed, never substring-matched: `localhost.example.com`
/// is an ordinary name.
fn is_loopback_url(raw: &str) -> bool {
    use url::Host;
    match url::Url::parse(raw)
        .ok()
        .and_then(|u| u.host().map(|h| h.to_owned()))
    {
        Some(Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

fn plugin_config_judgements(
    f: &PluginConfigFacts<'_>,
) -> (&'static str, Vec<String>, Vec<String>, bool) {
    let mut warnings = Vec::new();
    let mut must_supply = vec![
        "A network path from the engine to the download URL.".to_string(),
        "The files below, installed by you: egret never connects to this engine.".to_string(),
    ];
    let host_part = f.public_url.trim_end_matches('/');
    if host_part == IN_CLUSTER_DEFAULT_URL || host_part.contains(".svc") {
        warnings.push(format!(
            "The download URL ({host_part}) is an in-cluster address: an engine outside the \
             cluster cannot reach it. Set `[catalog] public_url` to an address the engine can reach."
        ));
    }
    if is_loopback_url(host_part) {
        warnings.push(format!(
            "The download URL ({host_part}) is a loopback address: no external engine can reach \
             it. It comes from `[catalog] public_url`, which defaults to localhost on a fresh \
             install; set it to an address the engine can reach."
        ));
    }
    let auth_mode = if f.bearer_configured {
        warnings.push(
            "egret requires a bearer token on /service, and a stock Ranger plugin cannot send \
             it: the download will be refused (401) unless something in front of egret adds the \
             token or exempts this engine."
                .to_string(),
        );
        must_supply.push(
            "A way to present egret's bearer token on the download (a proxy that adds it). \
             egret does not show or issue the token here."
                .to_string(),
        );
        "bearer-configured"
    } else {
        warnings.push(
            "The download is anonymous: anyone who can reach this URL can download every \
             profile's policies. Restrict it at the network level."
                .to_string(),
        );
        "anonymous"
    };
    if !f.ranger_enabled {
        warnings.push(
            "Ranger is disabled on this egret (`[ranger] enabled = false`): /service is not \
             served and the plugin has nothing to download."
                .to_string(),
        );
    }
    let modelled = f
        .version
        .is_some_and(|v| plugin_version_is_modelled(f.engine_type, v));
    if !modelled {
        warnings.push(format!(
            "egret's evaluator models only {}: for plugin version {} on {}, migrations into this \
             profile are refused.",
            egret_perm::modelled_ranger_plugins_sentence(),
            f.version.unwrap_or("(none declared)"),
            f.engine_type
        ));
    }
    (auth_mode, warnings, must_supply, modelled)
}

/// `GET /api/control-plane/engines/{id}/ranger-plugin-config`.
#[utoipa::path(
    get,
    path = "/api/control-plane/engines/{id}/ranger-plugin-config",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 200, description = "The Ranger plugin files to install on this external engine, rendered by the same code that renders the files egret mounts into its own engines. No credential is included or minted", body = RangerPluginConfigDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:read permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 409, description = "The engine is provisioned (egret mounts these files itself), or it is bound to no policy profile (bind one first)", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn ranger_plugin_config(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<RangerPluginConfigDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:read").await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    if !is_external(&engine) {
        return Err(CpError::Conflict(format!(
            "engine {} is provisioned: egret mounts its Ranger plugin files itself",
            engine.name
        )));
    }
    let Some(service_name) =
        crate::engines::resolve_ranger_service_name(&st, engine.authz_service_id).await?
    else {
        return Err(CpError::Conflict(format!(
            "engine {} is bound to no policy profile: bind a profile first",
            engine.name
        )));
    };
    let kind = egret_engine_mgmt_api::EngineKind::parse(&engine.r#type).ok_or_else(|| {
        tracing::error!(engine = %engine.name, r#type = %engine.r#type, "external engine of unknown type");
        CpError::Internal
    })?;
    let rendered = egret_provisioner::ranger_plugin_files(
        kind,
        &service_name,
        &st.engine_egret_url,
        egret_engine_mgmt_api::ranger_config::RANGER_POLL_INTERVAL_MS.into(),
    );
    let (auth_mode, warnings, admin_must_supply, modelled) =
        plugin_config_judgements(&PluginConfigFacts {
            engine_type: &engine.r#type,
            public_url: &st.engine_egret_url,
            bearer_configured: st.ranger_bearer_configured,
            ranger_enabled: st.ranger_enabled,
            version: engine.ranger_plugin_version.as_deref(),
        });
    Ok(Json(RangerPluginConfigDto {
        download_url: format!(
            "{}/service/plugins/policies/download/{service_name}",
            st.engine_egret_url.trim_end_matches('/')
        ),
        service_name,
        auth_mode: auth_mode.into(),
        ranger_plugin_version: engine.ranger_plugin_version.clone(),
        ranger_plugin_version_modelled: modelled,
        warnings,
        assumptions: egret_perm::EXTERNAL_PLUGIN_ASSUMPTIONS
            .iter()
            .map(|s| s.to_string())
            .collect(),
        admin_must_supply,
        files: rendered
            .files
            .into_iter()
            .map(|(name, content)| RangerPluginFileDto { name, content })
            .collect(),
    }))
}

#[cfg(test)]
mod plugin_config_tests {
    use super::*;

    fn facts<'a>(
        url: &'a str,
        bearer: bool,
        enabled: bool,
        v: Option<&'a str>,
    ) -> PluginConfigFacts<'a> {
        PluginConfigFacts {
            engine_type: "starrocks",
            public_url: url,
            bearer_configured: bearer,
            ranger_enabled: enabled,
            version: v,
        }
    }

    #[test]
    fn the_in_cluster_default_and_svc_urls_warn() {
        for url in [
            "http://egret:8080",
            "http://egret:8080/",
            "http://egret.data.svc:8080",
        ] {
            let (_, w, _, _) = plugin_config_judgements(&facts(url, false, true, Some("2.8.0")));
            assert!(w.iter().any(|m| m.contains("public_url")), "{url}");
        }
        let (_, w, _, _) = plugin_config_judgements(&facts(
            "https://egret.example.com",
            false,
            true,
            Some("2.8.0"),
        ));
        assert!(!w.iter().any(|m| m.contains("public_url")));
    }

    #[test]
    fn a_loopback_download_url_warns_and_names_the_setting() {
        for url in [
            "http://localhost:8080",
            "http://LocalHost:8080/",
            "http://127.0.0.1:8080",
            "http://127.1.2.3",
            "http://[::1]:8080",
        ] {
            let (_, w, _, _) = plugin_config_judgements(&facts(url, false, true, Some("2.8.0")));
            assert!(
                w.iter()
                    .any(|m| m.contains("loopback") && m.contains("[catalog] public_url")),
                "{url}: {w:?}"
            );
        }
    }

    #[test]
    fn a_host_that_only_contains_localhost_or_a_loopback_looking_name_does_not_warn() {
        for url in [
            "https://localhost.example.com",
            "https://mylocalhost:8080",
            "https://128.0.0.1",
            "https://egret.example.com/127.0.0.1",
            "https://[::2]:8080",
            "not a url",
        ] {
            let (_, w, _, _) = plugin_config_judgements(&facts(url, false, true, Some("2.8.0")));
            assert!(!w.iter().any(|m| m.contains("loopback")), "{url}: {w:?}");
        }
    }

    #[test]
    fn bearer_configured_says_a_stock_plugin_cannot_send_it_and_never_shows_a_token() {
        let (mode, w, must, _) =
            plugin_config_judgements(&facts("https://e.example.com", true, true, Some("2.8.0")));
        assert_eq!(mode, "bearer-configured");
        assert!(w.iter().any(|m| m.contains("cannot send")));
        assert!(must.iter().any(|m| m.contains("bearer token")));
        assert!(!w.iter().any(|m| m.contains("anonymous")));
    }

    #[test]
    fn anonymous_says_anyone_who_can_reach_it_downloads_every_profile() {
        let (mode, w, _, _) =
            plugin_config_judgements(&facts("https://e.example.com", false, true, Some("2.8.0")));
        assert_eq!(mode, "anonymous");
        assert!(w.iter().any(|m| m.contains("every profile's policies")));
    }

    #[test]
    fn an_unmodelled_pair_or_disabled_ranger_is_stated() {
        let (_, w, _, modelled) =
            plugin_config_judgements(&facts("https://e.example.com", false, false, Some("2.4.0")));
        assert!(!modelled);
        assert!(w
            .iter()
            .any(|m| m.contains("refused") && m.contains("2.4.0")));
        assert!(w.iter().any(|m| m.contains("Ranger is disabled")));
        let (_, _, _, modelled) =
            plugin_config_judgements(&facts("https://e.example.com", false, true, Some("2.8.0")));
        assert!(modelled);
    }
}
