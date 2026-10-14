//! View governance B5: what the DEFINER audit reads from egret's catalogs.
//! It needs every view in them, and each view's security mode.
//!
//! # Where the mode is stored (live result §1 Q2)
//!
//! Trino 483 writes `properties["trino.run-as-owner"] = <owner>` on a DEFINER
//! view, and no such property on an INVOKER view. The audit reads this stored
//! property and never the statement. [`crate::browse::load_view_detail`]
//! returns only the columns, so [`load_view_run_as_owner`] reads the
//! properties separately.
//!
//! **A read that fails is an error, never a mode.** The caller reports the
//! mode as `unknown`. A missing property is INVOKER only when the view's
//! metadata was actually read.
//!
//! # Which views
//!
//! The views of every warehouse the sweep walks
//! ([`crate::lineage_sweep::governed_warehouses`]): a warehouse with a
//! lakekeeper warehouse and a registry catalog, which the view is named under.
//! Like the sweep, it skips namespaces with more than one level: the Ranger
//! resource has one schema level, so nothing can be governed there. A listing
//! that fails is returned as a failure, never as "no views".

use egret_metastore::Db;
use lakekeeper::api::iceberg::types::Prefix;
use lakekeeper::api::iceberg::v1::views::{LoadViewRequest, ViewParameters, ViewService};
use lakekeeper::api::ApiContext;
use lakekeeper::iceberg::{NamespaceIdent, TableIdent};
use uuid::Uuid;

use crate::bootstrap::system_request_metadata;
use crate::browse::{list_namespaces, list_tabulars, TabularKind};
use crate::error::CatalogEngineError;
use crate::lineage_sweep::governed_warehouses;
use crate::mount::{EngineServer, EngineState};

/// The view property Trino writes on a DEFINER view; its value is the owner.
pub const RUN_AS_OWNER_PROPERTY: &str = "trino.run-as-owner";

/// One view in egret's catalogs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogView {
    /// The registry catalog name, as the lineage names it.
    pub catalog: String,
    pub schema: String,
    pub name: String,
    /// The lakekeeper warehouse id (its REST prefix), to load the view by.
    pub prefix: String,
}

/// Every view the listing found, and every listing that failed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CatalogViews {
    pub views: Vec<CatalogView>,
    /// One line per warehouse or namespace that could not be listed. When
    /// this is not empty, `views` is incomplete.
    pub failures: Vec<String>,
}

/// The views of egret's catalogs and their security mode, as the
/// control-plane audit sees them. A trait so the route test needs no catalog.
#[async_trait::async_trait]
pub trait ViewCatalog: Send + Sync {
    /// Every view in egret's catalogs (module doc). `Err` when nothing could
    /// be listed at all.
    async fn list_views(&self) -> Result<CatalogViews, String>;

    /// The view's stored `trino.run-as-owner`: `Ok(Some(owner))` for DEFINER,
    /// `Ok(None)` for INVOKER, `Err` when the metadata could not be read.
    async fn run_as_owner(&self, view: &CatalogView) -> Result<Option<String>, String>;
}

/// A build with no embedded catalog has no egret catalogs, and so no views.
pub struct NoViewCatalog;

#[async_trait::async_trait]
impl ViewCatalog for NoViewCatalog {
    async fn list_views(&self) -> Result<CatalogViews, String> {
        Ok(CatalogViews::default())
    }

    async fn run_as_owner(&self, _view: &CatalogView) -> Result<Option<String>, String> {
        Err("the embedded catalog is not enabled".into())
    }
}

/// The real reader, over the embedded lakekeeper.
pub struct EmbeddedViewCatalog {
    db: Db,
    engine: ApiContext<EngineState>,
    system_user: String,
    only: Option<Vec<Uuid>>,
}

impl EmbeddedViewCatalog {
    pub fn new(db: Db, engine: ApiContext<EngineState>, system_user: impl Into<String>) -> Self {
        Self {
            db,
            engine,
            system_user: system_user.into(),
            only: None,
        }
    }

    /// The same reader, listing only `warehouse_ids` (tests, which share one
    /// database).
    #[must_use]
    pub fn only_warehouses(mut self, warehouse_ids: &[Uuid]) -> Self {
        self.only = Some(warehouse_ids.to_vec());
        self
    }
}

#[async_trait::async_trait]
impl ViewCatalog for EmbeddedViewCatalog {
    async fn list_views(&self) -> Result<CatalogViews, String> {
        let warehouses = governed_warehouses(&self.db.pool, self.only.as_deref())
            .await
            .map_err(|e| format!("the warehouses could not be listed: {e}"))?;
        let mut out = CatalogViews::default();
        for w in warehouses {
            let namespaces =
                match list_namespaces(self.engine.clone(), &self.system_user, &w.prefix).await {
                    Ok(n) => n,
                    Err(e) => {
                        out.failures.push(format!(
                            "the namespaces of catalog {} could not be listed: {e}",
                            w.catalog
                        ));
                        continue;
                    }
                };
            for ns in namespaces {
                let [schema] = ns.parts.as_slice() else {
                    continue;
                };
                match list_tabulars(
                    self.engine.clone(),
                    &self.system_user,
                    &w.prefix,
                    std::slice::from_ref(schema),
                )
                .await
                {
                    Ok(tabulars) => out.views.extend(
                        tabulars
                            .into_iter()
                            .filter(|t| matches!(t.kind, TabularKind::View))
                            .map(|t| CatalogView {
                                catalog: w.catalog.clone(),
                                schema: schema.clone(),
                                name: t.name,
                                prefix: w.prefix.clone(),
                            }),
                    ),
                    Err(e) => out.failures.push(format!(
                        "the views of {}.{schema} could not be listed: {e}",
                        w.catalog
                    )),
                }
            }
        }
        Ok(out)
    }

    async fn run_as_owner(&self, view: &CatalogView) -> Result<Option<String>, String> {
        load_view_run_as_owner(
            self.engine.clone(),
            &self.system_user,
            &view.prefix,
            std::slice::from_ref(&view.schema),
            &view.name,
        )
        .await
        .map_err(|e| e.to_string())
    }
}

/// Load a view and read its stored [`RUN_AS_OWNER_PROPERTY`]: `Some(owner)`
/// for DEFINER, `None` for INVOKER. A property with an empty value is an
/// error: it names no owner, so it is neither mode.
pub async fn load_view_run_as_owner(
    engine_state: ApiContext<EngineState>,
    system_user: &str,
    warehouse_id: &str,
    namespace: &[String],
    view: &str,
) -> Result<Option<String>, CatalogEngineError> {
    let ident = TableIdent {
        namespace: NamespaceIdent::from_vec(namespace.to_vec())
            .map_err(|e| CatalogEngineError::BadRequest(format!("namespace ident: {e:?}")))?,
        name: view.to_string(),
    };
    let rm = system_request_metadata(system_user)
        .map_err(|e| CatalogEngineError::Internal(format!("system metadata: {e}")))?;
    let loaded = <EngineServer as ViewService<EngineState>>::load_view(
        ViewParameters {
            prefix: Some(Prefix::from(warehouse_id.to_string())),
            view: ident,
        },
        LoadViewRequest::default(),
        engine_state,
        rm,
    )
    .await
    .map_err(|e| CatalogEngineError::from_lakekeeper("load_view", e))?;
    match loaded.metadata.properties().get(RUN_AS_OWNER_PROPERTY) {
        None => Ok(None),
        Some(owner) if owner.is_empty() => Err(CatalogEngineError::Internal(format!(
            "the view's `{RUN_AS_OWNER_PROPERTY}` property is empty"
        ))),
        Some(owner) => Ok(Some(owner.clone())),
    }
}
