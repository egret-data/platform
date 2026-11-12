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

/// The StarRocks user the AuditLoader plugin stream-loads audit events as.
/// Reserved by migration 0218's `username_ledger` row, like [`DELEGATOR_PRINCIPAL`].
pub const QUERY_LOG_LOADER_PRINCIPAL: &str = "egret_query_log";

/// StarRocks' built-in superuser. Never an impersonation target and never a
/// user egret creates for a person (package D).
pub const STARROCKS_ROOT_USER: &str = "root";

/// The StarRocks users egret's delegator may never become (the D1b Ranger
/// row's exclusion) and that egret never creates or drops for a person (the
/// D1a user sync). Sorted, so a rendered body is byte-stable.
pub const STARROCKS_RESERVED_USERS: [&str; 3] = [
    DELEGATOR_PRINCIPAL,
    QUERY_LOG_LOADER_PRINCIPAL,
    STARROCKS_ROOT_USER,
];

/// Whether `name` is one of [`STARROCKS_RESERVED_USERS`], ignoring ASCII
/// case like the Ranger `user` resource (`ignoreCase: true`).
pub fn is_starrocks_reserved_user(name: &str) -> bool {
    STARROCKS_RESERVED_USERS
        .iter()
        .any(|r| r.eq_ignore_ascii_case(name))
}

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
    fn the_query_log_loader_principal_name() {
        assert_eq!(QUERY_LOG_LOADER_PRINCIPAL, "egret_query_log");
    }

    #[test]
    fn the_lineage_principal_is_reserved() {
        assert!(is_reserved_engine_username("egret-lineage"));
        assert!(!is_reserved_engine_username("egret-lineage-2"));
        assert!(!is_reserved_engine_username("alice"));
    }

    /// Package D: one list for the Ranger exclusion (D1b) and the user sync
    /// (D1a). Sorted, so a rendered body is byte-stable.
    #[test]
    fn the_starrocks_reserved_users_are_sorted_and_name_root_and_egrets_own() {
        assert_eq!(
            STARROCKS_RESERVED_USERS,
            ["egret_delegator", "egret_query_log", "root"]
        );
        assert!(is_starrocks_reserved_user("root"));
        assert!(is_starrocks_reserved_user("ROOT"));
        assert!(is_starrocks_reserved_user("egret_query_log"));
        assert!(!is_starrocks_reserved_user("root-2"));
        assert!(!is_starrocks_reserved_user("alice"));
    }
}
