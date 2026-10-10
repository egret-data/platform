#[derive(Debug, thiserror::Error)]
pub enum PolicyStoreError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    /// SP4a Task 8 (D4), fix round 2 (I-1): refused a hand-authored write
    /// into content `egret-perm-compile`'s recompile driver owns — either a
    /// brand-new policy in a `policy.authz_services.origin = 'generated'`
    /// profile, a specific policy whose own name carries the compiler's
    /// `perm-grant-`/`perm-rule-` prefix, or a profile-delete while such a
    /// policy still exists under it. The compiler writes `policy.authz_policies`
    /// directly and never calls the guarded `PolicyStore` write paths this
    /// refusal protects, so editing/deleting the flagged target here would be
    /// silently overwritten or resurrected by the next recompile.
    #[error("compiled profile: {0}")]
    CompilerOwned(String),
    /// SP4d-3: a human write aimed at a row egret's own plumbing owns
    /// (`managed_by = 'infra'`), or a human create/rename into a name
    /// `is_egret_infrastructure_policy_name` recognises. Mapped to 409, like
    /// [`Self::CompilerOwned`] — the request is well-formed and names a real
    /// policy or name; what is wrong is who owns it.
    #[error("{0}")]
    InfrastructureOwned(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
