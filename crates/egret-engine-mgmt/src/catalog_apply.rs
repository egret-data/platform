//! Registry catalogs ↔ engines (SP3 §4.4–4.5): render an attachment for Trino,
//! attach, detach, re-apply, and the per-engine-type dispatch. The StarRocks arm
//! is the SP3b seam: today it refuses everything but detaching a catalog that
//! was never created on the FE.

use std::collections::BTreeMap;

use egret_catalog_api::{CreatedClient, NewClient};
use egret_catalog_registry::{
    plaintext_secrets, render_trino, AttachedCatalog, CatalogDef, CatalogRow, EngineType,
    LegacyCatalog, NewAttachment, StorageRender, TrinoCatalogFile, TrinoRenderInput,
    WarehouseRender,
};
use uuid::Uuid;

use crate::engines::{
    apply_built, apply_conflict, build_provision_req_with, ensure_engine_role, ApplyBuiltError,
    ApplyOp, BuildOpts, CatalogReq, PendingAttachment,
};
use crate::Engine;
use crate::EngineHttpError as CpError;
use crate::EngineMgmtState as CpState;

pub(crate) const STARROCKS_NOT_YET: &str =
    "applying registry catalogs to StarRocks engines is not supported yet";

#[derive(Debug)]
pub(crate) enum ApplyError {
    /// The engine row is gone or `deleting` (a concurrent delete).
    Conflict,
    /// Kubernetes rejected (or never answered) the apply.
    Rejected(String),
    NotSupported(&'static str),
    /// The request could not be assembled (missing warehouse, secret, …).
    Build(CpError),
}

impl ApplyError {
    pub(crate) fn message(&self) -> String {
        match self {
            ApplyError::Conflict => {
                "engine changed while the update was being applied; retry".into()
            }
            ApplyError::Rejected(m) => m.clone(),
            ApplyError::NotSupported(m) => (*m).to_string(),
            ApplyError::Build(e) => e.to_string(),
        }
    }
}

pub(crate) enum CatalogChange {
    Attach(PendingAttachment),
    Detach(Uuid),
    Reapply,
}

#[derive(Debug)]
pub(crate) enum CatalogOpError {
    Http(CpError),
    Apply(ApplyError),
}

impl From<CpError> for CatalogOpError {
    fn from(e: CpError) -> Self {
        CatalogOpError::Http(e)
    }
}

impl From<egret_catalog_registry::RegistryError> for CatalogOpError {
    fn from(e: egret_catalog_registry::RegistryError) -> Self {
        CatalogOpError::Http(e.into())
    }
}

impl From<egret_catalog_api::CatalogApiError> for CatalogOpError {
    fn from(e: egret_catalog_api::CatalogApiError) -> Self {
        CatalogOpError::Http(e.into())
    }
}

impl CatalogOpError {
    /// Today's engine-catalog routes: a rejected apply is a 400.
    pub(crate) fn into_legacy(self) -> CpError {
        match self {
            CatalogOpError::Http(e) | CatalogOpError::Apply(ApplyError::Build(e)) => e,
            CatalogOpError::Apply(ApplyError::Conflict) => apply_conflict(),
            CatalogOpError::Apply(ApplyError::Rejected(m)) => {
                CpError::BadRequest(format!("engine update failed: {m}"))
            }
            CatalogOpError::Apply(ApplyError::NotSupported(m)) => {
                CpError::BadRequest(m.to_string())
            }
        }
    }

    /// Registry routes: a rejected apply is a 502 (spec §4.4). Used by
    /// `catalogs.rs`'s `attach_catalog` and `retry_catalog_apply`.
    pub(crate) fn into_registry(self) -> CpError {
        match self {
            CatalogOpError::Apply(ApplyError::Rejected(m)) => CpError::ClusterApplyFailed(m),
            other => other.into_legacy(),
        }
    }
}

/// One engine's result of applying a catalog change.
#[derive(Clone, Debug)]
pub(crate) struct ApplyOutcome {
    pub(crate) engine_id: Uuid,
    pub(crate) engine_name: String,
    pub(crate) applied: bool,
    pub(crate) error: Option<String>,
}

/// Apply a catalog change to one engine, by engine type.
pub(crate) async fn apply_catalog_change(
    st: &CpState,
    engine: &Engine,
    change: CatalogChange,
) -> Result<(), ApplyError> {
    match EngineType::parse(&engine.r#type) {
        Some(EngineType::Trino) => apply_trino(st, engine, change).await,
        Some(EngineType::StarRocks) => apply_starrocks(st, engine, change).await,
        None => Err(ApplyError::NotSupported("unknown engine type")),
    }
}

async fn apply_trino(
    st: &CpState,
    engine: &Engine,
    change: CatalogChange,
) -> Result<(), ApplyError> {
    let Some(prov) = st.engine_provisioner.as_ref() else {
        return Err(ApplyError::Build(CpError::BadRequest(
            "kubernetes not configured".into(),
        )));
    };
    let opts = match change {
        CatalogChange::Attach(p) => BuildOpts {
            pending_attachment: Some(p),
            ..BuildOpts::default()
        },
        CatalogChange::Detach(id) => BuildOpts {
            exclude_catalog: Some(id),
            ..BuildOpts::default()
        },
        CatalogChange::Reapply => BuildOpts::default(),
    };
    let built = build_provision_req_with(st, engine, opts)
        .await
        .map_err(ApplyError::Build)?;
    apply_built(st, prov.as_ref(), engine, &built, ApplyOp::Update)
        .await
        .map_err(|e| match e {
            ApplyBuiltError::Conflict => ApplyError::Conflict,
            ApplyBuiltError::Failed(m) => ApplyError::Rejected(m),
        })
}

/// SP3b replaces this with CREATE / ALTER / DROP CATALOG on the FE. In SP3a the
/// FE bootstrap creates only the engine's primary catalog, which cannot be
/// detached, so any other attachment was never on the FE and detaching it has
/// nothing to drop.
async fn apply_starrocks(
    _st: &CpState,
    _engine: &Engine,
    change: CatalogChange,
) -> Result<(), ApplyError> {
    match change {
        CatalogChange::Detach(_) => Ok(()),
        CatalogChange::Attach(_) | CatalogChange::Reapply => {
            Err(ApplyError::NotSupported(STARROCKS_NOT_YET))
        }
    }
}

fn catalog_def(catalog: &CatalogRow) -> Result<CatalogDef, CpError> {
    catalog
        .def()
        .map_err(|e| CpError::BadRequest(format!("catalog {}: {e}", catalog.name)))
}

/// Render an attached catalog with its inputs resolved (secrets still ids).
pub(crate) async fn render_attached(
    st: &CpState,
    a: &AttachedCatalog,
    env_prefix: &str,
) -> Result<TrinoCatalogFile, CpError> {
    let def = catalog_def(&a.catalog)?;
    let warehouse = match def.warehouse_id() {
        Some(id) => {
            let wh = st
                .catalog
                .get_warehouse(id)
                .await?
                .ok_or_else(|| CpError::BadRequest("unknown warehouse".into()))?;
            let sc = st
                .storage_registry
                .get_storage_conn(wh.storage_conn_id)
                .await?
                .ok_or_else(|| CpError::BadRequest("unknown storage connection".into()))?;
            Some((wh.name, sc.endpoint.unwrap_or_default()))
        }
        None => None,
    };
    let storage = match def.storage_connection_id() {
        Some(id) => Some(
            st.storage_registry
                .get_storage_conn(id)
                .await?
                .ok_or_else(|| CpError::BadRequest("unknown storage connection".into()))?,
        ),
        None => None,
    };
    let input = TrinoRenderInput {
        env_prefix,
        egret_url: &st.engine_egret_url,
        warehouse: warehouse.as_ref().map(|(name, ep)| WarehouseRender {
            name,
            s3_endpoint: ep,
        }),
        storage: storage.as_ref().map(|s| StorageRender {
            endpoint: s.endpoint.as_deref(),
            region: &s.region,
            path_style: s.path_style,
            access_key: &s.access_key,
            secret_key_secret_id: s.secret_ref,
        }),
        credential_secret_id: a.attachment.credential_secret_id,
    };
    render_trino(&a.catalog.name, &def, &input)
        .map_err(|e| CpError::BadRequest(format!("render catalog {}: {e}", a.catalog.name)))
}

/// The provisioner's catalog entry: rendered properties + resolved env values.
pub(crate) async fn trino_catalog_req(
    st: &CpState,
    a: &AttachedCatalog,
    env_prefix: &str,
) -> Result<CatalogReq, CpError> {
    let file = render_attached(st, a, env_prefix).await?;
    let mut env = BTreeMap::new();
    for (var, secret_id) in &file.env {
        let value = st
            .secrets
            .resolve_plaintext(None, *secret_id)
            .await
            .map_err(|e| CpError::BadRequest(format!("resolve secret: {e}")))?;
        env.insert(var.clone(), value);
    }
    Ok(CatalogReq {
        name: file.name,
        properties: file.properties,
        env,
    })
}

/// What `GET /engines/{id}/catalogs/{cid}` has always returned: an egret
/// warehouse's summary, or `{properties, secretProperties}` for the edit form.
pub(crate) async fn legacy_config_view(
    st: &CpState,
    a: &AttachedCatalog,
) -> Result<serde_json::Value, CpError> {
    let def = catalog_def(&a.catalog)?;
    if let Some(wh_id) = def.warehouse_id() {
        let wh = st
            .catalog
            .get_warehouse(wh_id)
            .await?
            .ok_or_else(|| CpError::BadRequest("unknown warehouse".into()))?;
        let endpoint = st
            .storage_registry
            .get_storage_conn(wh.storage_conn_id)
            .await?
            .and_then(|s| s.endpoint)
            .unwrap_or_default();
        return Ok(serde_json::json!({
            "warehouseId": wh.id,
            "warehouse": wh.name,
            "s3_endpoint": endpoint,
            "mintedClient": a.attachment.minted_client,
        }));
    }
    let file = render_attached(st, a, "L").await?;
    let (mut plain, secret) = file.split_secret_properties();
    plain.remove("connector.name");
    let secret: serde_json::Map<String, serde_json::Value> = secret
        .into_iter()
        .map(|(k, id)| (k, serde_json::Value::String(id.to_string())))
        .collect();
    Ok(serde_json::json!({ "properties": plain, "secretProperties": secret }))
}

/// Store a freshly minted client's credential as a named global secret.
pub(crate) async fn store_client_credential(
    st: &CpState,
    client: &CreatedClient,
) -> Result<Uuid, CpError> {
    Ok(st
        .secrets
        .create(
            None,
            &format!("catalog-client-{}", client.id),
            Some("Catalog Client credential an engine presents to egret's catalog"),
            &format!("{}:{}", client.client_id, client.client_secret),
        )
        .await?)
}

/// Move plaintext secrets out of a legacy body into named global secrets.
pub(crate) async fn extract_plaintext_secrets(
    st: &CpState,
    catalog_name: &str,
    legacy: &LegacyCatalog<'_>,
) -> Result<BTreeMap<String, Uuid>, CpError> {
    let mut out = BTreeMap::new();
    for (property, value) in plaintext_secrets(legacy) {
        let short = Uuid::new_v4().simple().to_string();
        let id = st
            .secrets
            .create(
                None,
                &format!("catalog-{catalog_name}-{property}-{}", &short[..8]),
                Some("Inline property of an engine catalog, moved into a secret"),
                &value,
            )
            .await?;
        out.insert(property, id);
    }
    Ok(out)
}

/// Delete secrets [`extract_plaintext_secrets`] just made for a request that
/// then failed before any catalog referenced them. Failures are logged.
pub(crate) async fn discard_extracted_secrets(st: &CpState, extracted: &BTreeMap<String, Uuid>) {
    for (property, id) in extracted {
        if let Err(e) = st.secrets.delete(None, *id).await {
            tracing::warn!(%property, secret_id = %id, error = %e,
                "could not delete a secret extracted for a rejected catalog request");
        }
    }
}

/// Every id a definition references must exist (global secrets, storage,
/// warehouse) — the typed parser cannot see the database.
pub(crate) async fn validate_references(st: &CpState, def: &CatalogDef) -> Result<(), CpError> {
    for id in def.secret_ids() {
        match st.secrets.get_meta(None, id).await {
            Ok(_) => {}
            Err(egret_secrets::SecretsError::NotFound) => {
                return Err(CpError::BadRequest(format!("unknown secret {id}")))
            }
            Err(e) => return Err(e.into()),
        }
    }
    if let Some(id) = def.storage_connection_id() {
        st.storage_registry
            .get_storage_conn(id)
            .await?
            .ok_or_else(|| CpError::BadRequest(format!("unknown storage connection {id}")))?;
    }
    if let Some(id) = def.warehouse_id() {
        st.catalog
            .get_warehouse(id)
            .await?
            .ok_or_else(|| CpError::BadRequest(format!("unknown warehouse {id}")))?;
    }
    Ok(())
}

fn is_primary(engine: &Engine, catalog: &CatalogRow) -> bool {
    catalog.warehouse_id == Some(engine.warehouse_id)
}

/// Retire what egret minted for an attachment the engine no longer uses: the
/// minted Catalog Client and the named secret holding its credential. Both only
/// when `minted_client` — a client (and credential) the engine did not mint
/// belongs to someone else, and its secret may be shared. Failures are logged.
pub(crate) async fn retire_minted_credential(
    st: &CpState,
    engine: &Engine,
    attachment: &egret_catalog_registry::AttachmentRow,
) {
    if !attachment.minted_client {
        return;
    }
    if let Some(client_id) = attachment.catalog_client_id {
        if let Err(e) = st.catalog.revoke_client(client_id).await {
            tracing::warn!(engine = %engine.name, %client_id, error = ?e,
                "minted catalog client could not be revoked");
        }
    }
    if let Some(secret_id) = attachment.credential_secret_id {
        match st.secrets.delete(None, secret_id).await {
            Ok(()) | Err(egret_secrets::SecretsError::NotFound) => {}
            Err(e) => tracing::warn!(engine = %engine.name, %secret_id, error = %e,
                "minted catalog client credential secret could not be deleted"),
        }
    }
}

/// Attach a registry catalog to an engine: apply first, then write the
/// attachment (SP1 Part B ordering). An egret warehouse catalog mints a Catalog
/// Client for this engine — under `role_from_client`'s role when given (today's
/// `config.clientId`), else the shared engine role.
pub(crate) async fn attach_to_engine(
    st: &CpState,
    engine: &Engine,
    catalog: &CatalogRow,
    role_from_client: Option<Uuid>,
) -> Result<(), CatalogOpError> {
    if engine.status == crate::engine_status::DELETING {
        return Err(CatalogOpError::Http(apply_conflict()));
    }
    let registry = st.engine_mgmt.registry();
    if registry
        .get_attachment(engine.id, catalog.id)
        .await?
        .is_some()
    {
        return Err(CatalogOpError::Http(CpError::Conflict(format!(
            "catalog {} is already attached to engine {}",
            catalog.name, engine.name
        ))));
    }
    let def = catalog_def(catalog)?;
    let engine_type = EngineType::parse(&engine.r#type)
        .ok_or_else(|| CpError::BadRequest(format!("unknown engine type {}", engine.r#type)))?;
    if !def.can_read(engine_type) {
        return Err(CatalogOpError::Http(CpError::BadRequest(format!(
            "a {} engine cannot read catalog {}",
            engine_type.as_str(),
            catalog.name
        ))));
    }
    if engine_type == EngineType::StarRocks {
        return Err(CatalogOpError::Apply(ApplyError::NotSupported(
            STARROCKS_NOT_YET,
        )));
    }

    let mut minted: Option<(Uuid, Uuid)> = None;
    if let Some(wh_id) = def.warehouse_id() {
        let wh = st
            .catalog
            .get_warehouse(wh_id)
            .await?
            .ok_or_else(|| CpError::BadRequest("unknown warehouse".into()))?;
        if wh.lakekeeper_warehouse_id.is_none() {
            return Err(CatalogOpError::Http(CpError::BadRequest(
                "warehouse is not provisioned yet".into(),
            )));
        }
        let role_id = match role_from_client {
            Some(cid) => {
                st.catalog
                    .list_clients(wh.id)
                    .await?
                    .into_iter()
                    .find(|c| c.id == cid)
                    .ok_or_else(|| {
                        CpError::BadRequest(
                            "config.clientId does not reference a client of this warehouse".into(),
                        )
                    })?
                    .catalog_role_id
            }
            None => ensure_engine_role(st, wh.id).await?,
        };
        let client = st
            .catalog
            .create_client(NewClient {
                name: format!("engine-{}-{}", engine.name, catalog.name),
                consumer_type: "engine".into(),
                workspace_id: wh.workspace_id,
                warehouse_id: wh.id,
                catalog_role_id: role_id,
            })
            .await?;
        match store_client_credential(st, &client).await {
            Ok(secret_id) => minted = Some((client.id, secret_id)),
            Err(e) => {
                if let Err(err) = st.catalog.revoke_client(client.id).await {
                    tracing::warn!(engine = %engine.name, client_id = %client.id, error = ?err,
                        "attach rollback: revoke failed");
                }
                return Err(e.into());
            }
        }
    }
    let pending = PendingAttachment {
        catalog_id: catalog.id,
        catalog_client_id: minted.map(|m| m.0),
        credential_secret_id: minted.map(|m| m.1),
    };

    if let Err(e) = apply_catalog_change(st, engine, CatalogChange::Attach(pending)).await {
        // Only delete egret's copy of the credential once the client that
        // holds it is actually revoked. If the failed apply got as far as the
        // engine's env Secret, a live, unrevoked client whose egret copy is
        // gone is a credential nobody can clean up; keep it and log loudly
        // instead so an operator can find and revoke it.
        if let Some((client_id, secret_id)) = minted {
            match st.catalog.revoke_client(client_id).await {
                Ok(()) => {
                    if let Err(err) = st.secrets.delete(None, secret_id).await {
                        tracing::warn!(engine = %engine.name, %secret_id, error = %err,
                            "attach rollback: secret delete failed");
                    }
                }
                Err(err) => {
                    tracing::error!(engine = %engine.name, %client_id, %secret_id, error = ?err,
                        "attach rollback: revoke failed; leaving the credential secret in place \
                         so it is not lost while the client may still be live — clean up manually");
                }
            }
        }
        return Err(CatalogOpError::Apply(e));
    }

    registry
        .insert_attachment(&NewAttachment {
            engine_id: engine.id,
            catalog_id: catalog.id,
            catalog_client_id: pending.catalog_client_id,
            credential_secret_id: pending.credential_secret_id,
            minted_client: minted.is_some(),
            applied_now: true,
            created_at: None,
        })
        .await
        .map_err(|e| {
            tracing::error!(engine = %engine.name, catalog = %catalog.name, error = %e,
                "catalog applied to the engine but the attachment could not be written; \
                 the next apply of this engine removes it again");
            CatalogOpError::from(e)
        })?;
    Ok(())
}

/// Detach: apply the engine without the catalog, then delete the attachment and
/// retire what egret minted for it. The engine's primary catalog cannot go.
pub(crate) async fn detach_from_engine(
    st: &CpState,
    engine: &Engine,
    catalog: &CatalogRow,
) -> Result<(), CatalogOpError> {
    let registry = st.engine_mgmt.registry();
    let attachment = registry
        .get_attachment(engine.id, catalog.id)
        .await?
        .ok_or(CatalogOpError::Http(CpError::NotFound))?;
    if is_primary(engine, catalog) {
        return Err(CatalogOpError::Http(CpError::BadRequest(
            "cannot delete the primary catalog".into(),
        )));
    }
    if engine.status == crate::engine_status::DELETING {
        return Err(CatalogOpError::Http(apply_conflict()));
    }
    apply_catalog_change(st, engine, CatalogChange::Detach(catalog.id))
        .await
        .map_err(CatalogOpError::Apply)?;
    registry.delete_attachment(engine.id, catalog.id).await?;
    // The cluster no longer uses these; failures are logged, the detach happened.
    retire_minted_credential(st, engine, &attachment).await;
    Ok(())
}

/// Re-apply one engine after a catalog changed; flags the attachment on a
/// rejected apply. A `deleting` engine (concurrent delete) is a 409 that
/// writes nothing — a refused apply must leave the DB unchanged, and the
/// attachment's `apply_status` says nothing true about a cluster the apply
/// never reached.
pub(crate) async fn retry_apply(
    st: &CpState,
    engine: &Engine,
    catalog_id: Uuid,
) -> Result<(), CatalogOpError> {
    if engine.status == crate::engine_status::DELETING {
        return Err(CatalogOpError::Apply(ApplyError::Conflict));
    }
    if let Err(e) = apply_catalog_change(st, engine, CatalogChange::Reapply).await {
        st.engine_mgmt
            .registry()
            .set_apply_failed(engine.id, catalog_id, &e.message())
            .await?;
        return Err(CatalogOpError::Apply(e));
    }
    Ok(())
}

/// Spec §4.5.3: apply a catalog's stored configuration to every engine that
/// attaches it, in engine-name order. A failure flags that attachment and moves
/// on — the other engines already hold the new configuration, so nothing reverts.
pub(crate) async fn propagate(
    st: &CpState,
    catalog: &CatalogRow,
) -> Result<Vec<ApplyOutcome>, CpError> {
    let mut out = Vec::new();
    for row in st
        .engine_mgmt
        .registry()
        .list_catalog_attachments(catalog.id)
        .await?
    {
        let Some(engine) = st.engine_mgmt.get_engine(row.engine_id).await? else {
            continue;
        };
        let outcome = match retry_apply(st, &engine, catalog.id).await {
            Ok(()) => ApplyOutcome {
                engine_id: engine.id,
                engine_name: engine.name.clone(),
                applied: true,
                error: None,
            },
            Err(CatalogOpError::Apply(e)) => ApplyOutcome {
                engine_id: engine.id,
                engine_name: engine.name.clone(),
                applied: false,
                error: Some(e.message()),
            },
            Err(CatalogOpError::Http(e)) => return Err(e),
        };
        out.push(outcome);
    }
    Ok(out)
}
