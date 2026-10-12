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

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_core::http::ErrorBody;
use serde::Deserialize;
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

/// The OpenAPI fragment of the external-engine routes, merged by
/// [`crate::openapi`].
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(register_external_engine),
    components(schemas(RegisterExternalEngine))
)]
pub struct ExternalEngineApi;

pub fn routes() -> Router<CpState> {
    Router::new().route("/engines/external", post(register_external_engine))
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
