#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("in use")]
    InUse,
    #[error("bad request: {0}")]
    BadRequest(String),
    /// A state conflict that isn't a duplicate-name clash covered by
    /// `AlreadyExists` (e.g. a warehouse delete refused because its registry
    /// catalog is still attached to an engine).
    #[error("conflict: {0}")]
    Conflict(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    /// `delete_warehouse_releasing_catalog`'s in-transaction recompile (SP4a
    /// Task 16, D3 defect 3) failing — rolls the whole delete back, same
    /// treatment `Db` above gets, but logged as its own condition (see the
    /// `RegistryError::Compile`/`EngineHttpError` mapping this mirrors).
    #[error(transparent)]
    Compile(#[from] egret_perm::CompileError),
    /// `egret_perm::compiled_service_ids_for_catalog_via`'s own query
    /// failing before the delete this recompile accompanies even runs.
    #[error(transparent)]
    Perm(#[from] egret_perm::PermError),
}
