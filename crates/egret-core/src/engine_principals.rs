//! Engine principals that egret itself runs as, and the usernames they reserve.
//!
//! A human's engine identity is their `users.username`, carried as
//! `preferred_username` in the engine JWT egret mints for them. A principal
//! egret runs as must therefore never be a name a human can be given, or that
//! human would hold the principal's grants.

/// The principal egret runs `EXPLAIN (TYPE IO, FORMAT JSON)` as on Trino to
/// learn a view's input tables (view governance B1, spec R5).
///
/// On the download served to a Trino engine egret provisions in JWT mode
/// (`egret_policy::lineage_transform`), it gets `use` and `select` on every
/// governed catalog, is excepted from every deny, mask and row filter, and is
/// denied everything else, so it reads governed data. Only egret mints a token for it, and egret's lineage client
/// sends nothing but `EXPLAIN` as it.
pub const LINEAGE_PRINCIPAL: &str = "egret-lineage";

/// The Trino service user egret authenticates as for impersonate-mode
/// delegation (`egret_engine_mgmt::delegator_policy`, which re-exports it as
/// `DELEGATOR_USERNAME`). Declared here so `egret-perm-compile`, which cannot
/// depend on `egret-engine-mgmt`, can refuse it as an impersonation target.
///
/// **Not in [`RESERVED_ENGINE_USERNAMES`], but reserved all the same**:
/// migration 0209 seeds it into `username_ledger` (step 5), so no human can be
/// minted it (username-takeover spec §4.1; engine grants spec E21).
pub const DELEGATOR_PRINCIPAL: &str = "egret_delegator";

/// Usernames no human may hold (see the module doc).
pub const RESERVED_ENGINE_USERNAMES: &[&str] = &[LINEAGE_PRINCIPAL];

/// Whether `username` is reserved for a principal egret runs as. Exact match:
/// usernames are stored lowercase (`egret_auth::users::ensure_username`).
pub fn is_reserved_engine_username(username: &str) -> bool {
    RESERVED_ENGINE_USERNAMES.contains(&username)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F11 (spec C6): the delegator's name lives here so `egret-perm-compile`
    /// can refuse it as an impersonation target. It is not in
    /// `RESERVED_ENGINE_USERNAMES`: migration 0209's `username_ledger` reserves
    /// it (`egret-auth`'s `tests/username_ledger.rs`).
    #[test]
    fn the_delegator_principal_is_not_in_the_list_but_is_reserved_by_the_ledger() {
        assert_eq!(DELEGATOR_PRINCIPAL, "egret_delegator");
        assert!(!is_reserved_engine_username(DELEGATOR_PRINCIPAL));
    }

    #[test]
    fn the_lineage_principal_is_reserved() {
        assert!(is_reserved_engine_username("egret-lineage"));
        assert!(!is_reserved_engine_username("egret-lineage-2"));
        assert!(!is_reserved_engine_username("alice"));
    }
}
