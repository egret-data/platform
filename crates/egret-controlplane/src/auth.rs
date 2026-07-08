//! Platform-admin guard for control-plane routes.
//!
//! The gate logic now lives in [`egret_auth::authz`] so any crate can gate
//! without depending on `egret-controlplane`. These are thin adapters that
//! preserve the `Result<(), CpError>` signature the call sites expect —
//! `egret_core::Error::Forbidden` maps to `CpError::Forbidden` (→ 403) via the
//! existing `From<egret_core::Error>` impl in `crate::error`.

use egret_core::Principal;

use crate::error::CpError;

/// Reject any caller lacking the `platform_admin` role. Delegates to `egret-auth`.
pub fn require_platform_admin(principal: &Principal) -> Result<(), CpError> {
    egret_auth::require_platform_admin(principal).map_err(CpError::from)
}

/// Gate a control-plane action on a permission key. `platform_admin` always
/// passes. Delegates to `egret-auth`.
pub fn require_permission(principal: &Principal, permission: &str) -> Result<(), CpError> {
    egret_auth::require_permission(principal, permission).map_err(CpError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_core::ids::UserId;

    fn principal(roles: &[&str], perms: &[&str]) -> Principal {
        Principal {
            user_id: UserId::new(),
            email: "a@b.c".into(),
            platform_roles: roles.iter().map(|s| s.to_string()).collect(),
            permissions: perms.iter().map(|s| s.to_string()).collect(),
        }
    }

    // The delegators must preserve the 403 mapping: a denied gate → CpError::Forbidden.
    #[test]
    fn delegator_maps_denial_to_cp_forbidden() {
        assert!(matches!(
            require_platform_admin(&principal(&[], &[])),
            Err(CpError::Forbidden)
        ));
        assert!(matches!(
            require_permission(&principal(&[], &["engine:read"]), "engine:create"),
            Err(CpError::Forbidden)
        ));
    }

    #[test]
    fn delegator_allows_when_authorized() {
        assert!(require_platform_admin(&principal(&["platform_admin"], &[])).is_ok());
        assert!(require_permission(&principal(&[], &["engine:create"]), "engine:create").is_ok());
    }
}
