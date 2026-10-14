//! Engine principals that egret itself runs as, and the usernames they reserve.
//!
//! A human's engine identity is their `users.username`, carried as
//! `preferred_username` in the engine JWT egret mints for them. A principal
//! egret runs as must therefore never be a name a human can be given, or that
//! human would hold the principal's grants.

/// The principal egret runs `EXPLAIN (TYPE IO, FORMAT JSON)` as on Trino to
/// learn a view's input tables (view governance B1, spec R5).
///
/// The compiler gives it `use` and `select` on every governed catalog and
/// excepts it from every compiled deny, mask and row filter, so it reads
/// governed data. Only egret mints a token for it, and egret's lineage client
/// sends nothing but `EXPLAIN` as it.
pub const LINEAGE_PRINCIPAL: &str = "egret-lineage";

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

    #[test]
    fn the_lineage_principal_is_reserved() {
        assert!(is_reserved_engine_username("egret-lineage"));
        assert!(!is_reserved_engine_username("egret-lineage-2"));
        assert!(!is_reserved_engine_username("alice"));
    }
}
