//! DG frontend redesign #1 (spec §2.3): `GET
//! /governance/browse/{catalog_id}?schema=&table=` — one level of a registry
//! catalog for the Governance Data tree: its schemas, a schema's tables and
//! views, or a table's columns. Gated on `policy:read` OR `tag:read`
//! ([`require_governance_read`]): the answer reveals catalog structure.
//!
//! **Where the listing comes from.**
//! - An **egret** Iceberg catalog (one with a `warehouse_id`) is read from the
//!   embedded warehouse through [`CpState::browser`], as the warehouse admin's
//!   `namespaces` / `tables` routes read it. No engine is asked.
//! - **Any other** catalog is listed through ONE engine ([`choose_engine`]):
//!   provisioned by egret, `running`, attached to the catalog, and one egret
//!   holds an identity on that can list (a JWT-mode Trino or a StarRocks).
//!   Trino before StarRocks, then by name. The engine is asked through the
//!   [`CatalogLister`] seam with the catalog's registry name, which IS its
//!   name on every engine (SP3 D5, `migrations/0147_catalog_registry.sql`).
//!
//! **A listing that fails says so; it is never returned as empty.** When no
//! engine can be asked, or the one asked fails or misses
//! [`CpState::browse_timeout`], the answer carries `notListed` with the reason
//! and no `items` / `columns`. The engine's own error text is logged, not
//! returned.
//!
//! **Engines that are never dialled.** An EXTERNAL engine (SP4d-4) holds the
//! placeholder status `running` and egret has no credential for it, so origin
//! is checked FIRST, as `view_exposure` does. A Trino in **impersonate** mode is
//! not asked either: no internal egret identity can list a catalog there
//! (measured, `docs/en/plans/2026-10-16-dg-frontend-data-live.md` §1.1); only a
//! person's identity could.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_core::http::ErrorBody;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::CpError;
use crate::CpState;

pub fn router() -> Router<CpState> {
    Router::new().route("/governance/browse/{catalog_id}", get(browse))
}

/// The engines egret can list a catalog through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingEngineType {
    Trino,
    StarRocks,
}

/// The engine a [`CatalogLister`] is asked to list on. Carries the id (what
/// the real lister dials) and the name (what the page and the log show).
#[derive(Debug, Clone)]
pub struct ListingEngine {
    pub id: Uuid,
    pub name: String,
    pub engine_type: ListingEngineType,
}

/// What one level of a catalog holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BrowseKind {
    Schema,
    Table,
    View,
    MaterializedView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct BrowseItem {
    pub name: String,
    pub kind: BrowseKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct BrowseColumn {
    pub name: String,
    /// The type as the source spells it: an Iceberg type for an egret
    /// catalog, the engine's type name otherwise.
    pub r#type: String,
}

/// One level of a catalog, as a [`CatalogLister`] read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listing {
    /// Schemas (no schema given), or a schema's tables, views and
    /// materialized views.
    Items(Vec<BrowseItem>),
    /// A table's or a view's columns (a table given).
    Columns(Vec<BrowseColumn>),
}

/// Lists one level of a catalog on one engine. A trait so the route test
/// needs no live engine; the real Trino and StarRocks listers
/// (`egret_query::execution::catalog_listing`,
/// `egret_engine_mgmt::list_catalog_level`) are wired in `egret-bin`.
#[async_trait::async_trait]
pub trait CatalogLister: Send + Sync {
    /// List one level of `catalog` (its name as the ENGINE knows it) on
    /// `engine`: its schemas when `schema` is `None`, the schema's tables
    /// when `table` is `None`, the table's columns otherwise. `Err` is the
    /// reason, logged and never shown.
    ///
    /// `schema` and `table` are the caller's raw query values: UNTRUSTED. The
    /// route guarantees only that neither is empty and that `table` comes with
    /// `schema`; an implementation quotes them as identifiers (or escapes them
    /// as literals) before they reach a statement. A schema or table the engine
    /// does not have is an `Err`, never an empty listing.
    async fn list(
        &self,
        engine: &ListingEngine,
        catalog: &str,
        schema: Option<&str>,
        table: Option<&str>,
    ) -> Result<Listing, String>;
}

/// A lister that fails every listing, so the page shows `engine_error` rather
/// than an empty catalog — for a state with no engine to list on (the tests).
/// `egret-bin` wires the real Trino and StarRocks listers.
pub struct UnwiredCatalogLister;

#[async_trait::async_trait]
impl CatalogLister for UnwiredCatalogLister {
    async fn list(
        &self,
        _engine: &ListingEngine,
        _catalog: &str,
        _schema: Option<&str>,
        _table: Option<&str>,
    ) -> Result<Listing, String> {
        Err("no catalog lister is wired".to_string())
    }
}

/// Where the listing was read.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowseSource {
    /// egret's embedded warehouse.
    Egret,
    /// The named engine.
    Engine { name: String },
}

/// One engine the catalog is attached to.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BrowseEngine {
    pub name: String,
    /// `trino`, `starrocks` or `risingwave`.
    pub r#type: String,
    /// `provisioned` or `external`.
    pub origin: String,
    /// `provisioning`, `running`, `failed` or `deleting`. An external engine's
    /// `running` is a placeholder (migration 0184).
    pub status: String,
}

/// Why a level was not listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotListedReason {
    NoEngineAttached,
    /// Every attached engine is external: egret never connects to them.
    OnlyExternalEngines,
    /// No engine egret provisioned for this catalog is running.
    EngineNotRunning,
    /// The engine, or the warehouse, did not answer within the deadline.
    Timeout,
    /// The listing failed, or no running engine is one egret can list
    /// through (an impersonate-mode Trino, a RisingWave).
    EngineError,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct NotListed {
    pub reason: NotListedReason,
    /// A sentence for the page. Never the engine's own error text.
    pub detail: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrowseResponse {
    /// Where the listing was read; `null` when nothing could be asked.
    pub source: Option<BrowseSource>,
    /// Every engine the catalog is attached to, by name.
    pub engines: Vec<BrowseEngine>,
    /// Schemas, or a schema's tables and views. `null` when `table` is given
    /// or the level was not listed.
    pub items: Option<Vec<BrowseItem>>,
    /// The table's columns, when `table` is given and it was listed.
    pub columns: Option<Vec<BrowseColumn>>,
    /// Set exactly when the level could not be listed.
    pub not_listed: Option<NotListed>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct BrowseQuery {
    /// The schema to list (dotted for a nested egret namespace).
    pub schema: Option<String>,
    /// The table or view whose columns to list; needs `schema`.
    pub table: Option<String>,
}

/// `policy:read` OR `tag:read`: the two permissions that read Governance
/// (spec §2.3, "Permissions"). A PDP failure is not read as a refusal.
pub(crate) async fn require_governance_read(
    st: &CpState,
    p: &egret_core::Principal,
) -> Result<(), CpError> {
    match egret_core::authz::require(&*st.pdp, p, "policy:read").await {
        Ok(()) => Ok(()),
        Err(egret_core::Error::Forbidden) => {
            Ok(egret_core::authz::require(&*st.pdp, p, "tag:read").await?)
        }
        Err(e) => Err(e.into()),
    }
}

#[utoipa::path(
    get,
    path = "/api/control-plane/governance/browse/{catalog_id}",
    tag = "control-plane-governance",
    params(
        ("catalog_id" = Uuid, Path, description = "Registry catalog id"),
        BrowseQuery,
    ),
    responses(
        (status = 200, description = "One level of the catalog, or why it was not listed", body = BrowseResponse),
        (status = 400, description = "`table` without `schema`, or an empty `schema` or `table`", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:read and tag:read permission", body = ErrorBody),
        (status = 404, description = "No such catalog, or (egret catalog) no such schema or table", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn browse(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(catalog_id): Path<Uuid>,
    Query(q): Query<BrowseQuery>,
) -> Result<Json<BrowseResponse>, CpError> {
    require_governance_read(&st, &p).await?;
    if q.table.is_some() && q.schema.is_none() {
        return Err(CpError::BadRequest("`table` needs `schema`".to_string()));
    }
    if q.schema.as_deref() == Some("") || q.table.as_deref() == Some("") {
        return Err(CpError::BadRequest(
            "`schema` and `table` must not be empty".to_string(),
        ));
    }
    let catalog = load_catalog(&st, catalog_id)
        .await?
        .ok_or(CpError::NotFound)?;
    let attached = attached_engines(&st, catalog_id).await?;
    let engines = attached
        .iter()
        .map(|e| BrowseEngine {
            name: e.name.clone(),
            r#type: e.engine_type.clone(),
            origin: e.origin.clone(),
            status: e.status.clone(),
        })
        .collect();

    let level = Level {
        schema: q.schema.as_deref(),
        table: q.table.as_deref(),
    };
    let (source, listed) = match catalog.warehouse_id {
        Some(warehouse_id) => (
            Some(BrowseSource::Egret),
            list_egret(&st, warehouse_id, level).await?,
        ),
        None => match choose_engine(&attached) {
            Err(not_listed) => (None, Err(not_listed)),
            Ok(engine) => {
                let listed = list_on_engine(&st, &engine, &catalog.name, level).await;
                (Some(BrowseSource::Engine { name: engine.name }), listed)
            }
        },
    };
    let (items, columns, not_listed) = match listed {
        Ok(Listing::Items(items)) => (Some(items), None, None),
        Ok(Listing::Columns(columns)) => (None, Some(columns), None),
        Err(not_listed) => (None, None, Some(not_listed)),
    };
    Ok(Json(BrowseResponse {
        source,
        engines,
        items,
        columns,
        not_listed,
    }))
}

/// The level asked for.
#[derive(Clone, Copy)]
struct Level<'a> {
    schema: Option<&'a str>,
    table: Option<&'a str>,
}

#[derive(sqlx::FromRow)]
struct CatalogRow {
    /// The catalog's SQL name on every engine (SP3 D5).
    name: String,
    /// Set exactly for an egret Iceberg catalog
    /// (`catalogs_warehouse_mirrors_config`, migration 0147).
    warehouse_id: Option<Uuid>,
}

async fn load_catalog(st: &CpState, id: Uuid) -> Result<Option<CatalogRow>, CpError> {
    sqlx::query_as::<_, CatalogRow>(
        "SELECT name, warehouse_id FROM registry.catalogs WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&st.db.pool)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "governance browse: read catalog");
        CpError::Internal
    })
}

#[derive(sqlx::FromRow)]
struct AttachedEngine {
    id: Uuid,
    name: String,
    engine_type: String,
    origin: String,
    status: String,
    /// `spec.trinoConfig.delegationMode`; `None` when absent.
    delegation_mode: Option<String>,
}

async fn attached_engines(st: &CpState, catalog_id: Uuid) -> Result<Vec<AttachedEngine>, CpError> {
    sqlx::query_as::<_, AttachedEngine>(
        "SELECT e.id, e.name, e.type AS engine_type, e.origin, e.status, \
                e.spec->'trinoConfig'->>'delegationMode' AS delegation_mode \
           FROM registry.engine_catalog_attachments a \
           JOIN engine_mgmt.engines e ON e.id = a.engine_id \
          WHERE a.catalog_id = $1 ORDER BY e.name",
    )
    .bind(catalog_id)
    .fetch_all(&st.db.pool)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "governance browse: read attached engines");
        CpError::Internal
    })
}

/// `engine_mgmt.engines.status` of an engine whose pods all reported ready
/// (`migrations/0145_engine_status_check.sql`).
const RUNNING: &str = "running";

/// `engine_mgmt.engines.origin` of an engine egret does not run
/// (`migrations/0184_engine_external.sql`).
const ORIGIN_EXTERNAL: &str = "external";

/// The engine type egret can list `e` through, or why it cannot.
///
/// A Trino lists only in JWT delegation mode, the mode the lineage transform
/// (and so `egret-lineage`'s `use`/`select`) is served to
/// (`egret_policy::lineage_transform::eligible_bound_engines_via`). A Trino
/// with no `delegationMode` is in impersonate mode
/// (`egret_engine_mgmt::EngineRow::delegation_mode`).
fn listing_type(e: &AttachedEngine) -> Result<ListingEngineType, String> {
    match e.engine_type.as_str() {
        "trino" if e.delegation_mode.as_deref() == Some("jwt") => Ok(ListingEngineType::Trino),
        "trino" => Err(format!(
            "{} runs in impersonate mode; egret has no identity there that can list catalogs",
            e.name
        )),
        "starrocks" => Ok(ListingEngineType::StarRocks),
        other => Err(format!(
            "{} is a {other} engine; egret lists catalogs only through Trino or StarRocks",
            e.name
        )),
    }
}

/// The engine to list through: provisioned, running, attached, and one egret
/// can list on; Trino before StarRocks, then by name. Or why there is none.
fn choose_engine(attached: &[AttachedEngine]) -> Result<ListingEngine, NotListed> {
    if attached.is_empty() {
        return Err(NotListed {
            reason: NotListedReason::NoEngineAttached,
            detail: "this catalog is not attached to any engine".to_string(),
        });
    }
    // Origin FIRST: an external engine's `running` is a placeholder.
    let provisioned: Vec<&AttachedEngine> = attached
        .iter()
        .filter(|e| e.origin != ORIGIN_EXTERNAL)
        .collect();
    if provisioned.is_empty() {
        return Err(NotListed {
            reason: NotListedReason::OnlyExternalEngines,
            detail: format!(
                "this catalog is attached only to external engines ({}); egret never \
                 connects to an engine it does not run",
                names(attached.iter())
            ),
        });
    }
    let running: Vec<&AttachedEngine> = provisioned
        .iter()
        .copied()
        .filter(|e| e.status == RUNNING)
        .collect();
    if running.is_empty() {
        let states: Vec<String> = provisioned
            .iter()
            .map(|e| format!("{} ({})", e.name, e.status))
            .collect();
        return Err(NotListed {
            reason: NotListedReason::EngineNotRunning,
            detail: format!(
                "no engine egret runs for this catalog is running: {}",
                states.join(", ")
            ),
        });
    }
    let mut reasons = Vec::new();
    let mut listable = Vec::new();
    for e in running {
        match listing_type(e) {
            Ok(t) => listable.push((t, e)),
            Err(reason) => reasons.push(reason),
        }
    }
    // `attached` is sorted by name, and the sort is stable.
    listable.sort_by_key(|(t, _)| *t != ListingEngineType::Trino);
    match listable.first() {
        Some((engine_type, e)) => Ok(ListingEngine {
            id: e.id,
            name: e.name.clone(),
            engine_type: *engine_type,
        }),
        None => Err(NotListed {
            reason: NotListedReason::EngineError,
            detail: reasons.join("; "),
        }),
    }
}

fn names<'a>(engines: impl Iterator<Item = &'a AttachedEngine>) -> String {
    engines
        .map(|e| e.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Seconds as the page shows them: `15`, or `0.3` below a whole second.
fn seconds(d: std::time::Duration) -> String {
    if d.subsec_millis() == 0 {
        d.as_secs().to_string()
    } else {
        format!("{:.1}", d.as_secs_f64())
    }
}

/// Ask `engine` under [`CpState::browse_timeout`]. A failure, a missed
/// deadline, or an answer at the wrong level is a [`NotListed`]; the detail
/// goes to the log.
async fn list_on_engine(
    st: &CpState,
    engine: &ListingEngine,
    catalog: &str,
    level: Level<'_>,
) -> Result<Listing, NotListed> {
    let asked = st
        .catalog_lister
        .list(engine, catalog, level.schema, level.table);
    let failed = || NotListed {
        reason: NotListedReason::EngineError,
        detail: format!(
            "egret could not list this catalog on engine {}; the error is in the server log",
            engine.name
        ),
    };
    match tokio::time::timeout(st.browse_timeout, asked).await {
        Err(_) => {
            tracing::warn!(engine = %engine.name, catalog, timeout = ?st.browse_timeout,
                "governance browse: the engine did not answer before the deadline");
            Err(NotListed {
                reason: NotListedReason::Timeout,
                detail: format!(
                    "engine {} did not answer within {} s",
                    engine.name,
                    seconds(st.browse_timeout)
                ),
            })
        }
        Ok(Err(e)) => {
            tracing::warn!(engine = %engine.name, catalog, error = %e,
                "governance browse: the engine's listing failed");
            Err(failed())
        }
        Ok(Ok(listing)) => match (&listing, level.table) {
            (Listing::Items(_), None) | (Listing::Columns(_), Some(_)) => Ok(listing),
            _ => {
                tracing::error!(engine = %engine.name, catalog,
                    "governance browse: the lister answered at the wrong level");
                Err(failed())
            }
        },
    }
}

/// The detail when egret's own warehouse could not be read.
const WAREHOUSE_UNREADABLE: &str =
    "egret could not read this catalog's warehouse; the error is in the server log";

/// One level of an egret catalog, from its warehouse, under
/// [`CpState::browse_timeout`]. A namespace, table or view that does not exist
/// is a 404, as on the warehouse admin's routes; any other failure is a
/// [`NotListed`].
async fn list_egret(
    st: &CpState,
    warehouse_id: Uuid,
    level: Level<'_>,
) -> Result<Result<Listing, NotListed>, CpError> {
    let Some(browser) = st.browser.as_ref() else {
        tracing::error!("governance browse: no warehouse browser is wired");
        return Err(CpError::Internal);
    };
    let warehouse = st.storage.get_warehouse(warehouse_id).await?;
    let Some(lk) = warehouse.and_then(|w| w.lakekeeper_warehouse_id) else {
        return Ok(Err(NotListed {
            reason: NotListedReason::EngineError,
            detail: "this catalog's warehouse is not provisioned in egret's catalog".to_string(),
        }));
    };
    let read = read_warehouse(&**browser, &lk, level);
    match tokio::time::timeout(st.browse_timeout, read).await {
        Err(_) => {
            tracing::warn!(warehouse = %lk, timeout = ?st.browse_timeout,
                "governance browse: the warehouse did not answer before the deadline");
            Ok(Err(NotListed {
                reason: NotListedReason::Timeout,
                detail: format!(
                    "egret's catalog did not answer within {} s",
                    seconds(st.browse_timeout)
                ),
            }))
        }
        Ok(Ok(Some(listing))) => Ok(Ok(listing)),
        Ok(Ok(None)) => Err(CpError::NotFound),
        Ok(Err(egret_catalog_engine::CatalogEngineError::NotFound(_))) => Err(CpError::NotFound),
        Ok(Err(e)) => {
            tracing::warn!(warehouse = %lk, error = %e,
                "governance browse: the warehouse listing failed");
            Ok(Err(NotListed {
                reason: NotListedReason::EngineError,
                detail: WAREHOUSE_UNREADABLE.to_string(),
            }))
        }
    }
}

/// One level of the lakekeeper warehouse `lk`; `None` when the table or view
/// asked for is not in its namespace. A schema is a namespace, its levels
/// joined with `.`, and split back the same way (`warehouse_admin::parse_ns`).
async fn read_warehouse(
    browser: &dyn egret_catalog_engine::WarehouseBrowser,
    lk: &str,
    level: Level<'_>,
) -> Result<Option<Listing>, egret_catalog_engine::CatalogEngineError> {
    let Some(schema) = level.schema else {
        let mut items: Vec<BrowseItem> = browser
            .namespaces(lk)
            .await?
            .into_iter()
            .map(|n| BrowseItem {
                name: n.parts.join("."),
                kind: BrowseKind::Schema,
            })
            .collect();
        items.sort_by(|a, b| a.name.cmp(&b.name));
        return Ok(Some(Listing::Items(items)));
    };
    let ns: Vec<String> = schema.split('.').map(String::from).collect();
    let tabulars = browser.tabulars(lk, &ns).await?;
    let Some(table) = level.table else {
        let mut items: Vec<BrowseItem> = tabulars
            .into_iter()
            .map(|t| BrowseItem {
                kind: if t.kind == "view" {
                    BrowseKind::View
                } else {
                    BrowseKind::Table
                },
                name: t.name,
            })
            .collect();
        items.sort_by(|a, b| a.name.cmp(&b.name));
        return Ok(Some(Listing::Items(items)));
    };
    // The kind decides which read applies: a view has no table metadata.
    let Some(found) = tabulars.into_iter().find(|t| t.name == table) else {
        return Ok(None);
    };
    let columns = if found.kind == "view" {
        browser.view_columns(lk, &ns, table).await?
    } else {
        browser.schema(lk, &ns, table).await?.columns
    };
    Ok(Some(Listing::Columns(
        columns
            .into_iter()
            .map(|c| BrowseColumn {
                name: c.name,
                r#type: c.type_,
            })
            .collect(),
    )))
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(browse),
    components(schemas(
        BrowseResponse,
        BrowseSource,
        BrowseEngine,
        BrowseItem,
        BrowseKind,
        BrowseColumn,
        NotListed,
        NotListedReason,
        ErrorBody
    )),
    tags((name = "control-plane-governance", description = "Governance Data: browse a catalog (DG frontend redesign #1)")),
)]
pub struct ControlPlaneGovernanceBrowseApi;
