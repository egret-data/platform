#[derive(Debug, thiserror::Error)]
pub enum PolicyStoreError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    /// SP4a Task 8 (D4): refused a hand-authored write into a profile
    /// `policy.authz_services.origin = 'generated'` — compiled by
    /// `egret-perm-compile`'s recompile driver, which writes
    /// `policy.authz_policies` directly and never calls the guarded
    /// `PolicyStore` write paths this refusal protects. Editing here would be
    /// silently overwritten by the next recompile.
    #[error("compiled profile: {0}")]
    GeneratedProfile(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
