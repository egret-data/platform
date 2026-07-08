//! Platform-admin guard for control-plane routes.

use egret_core::Principal;

use crate::error::CpError;

/// The platform role every control-plane (global) mutation requires.
const PLATFORM_ADMIN: &str = "platform_admin";

/// Reject any caller that does not hold the `platform_admin` platform role.
///
/// The `Auth(principal)` extractor already guarantees a valid session (401
/// otherwise); this gates that authenticated principal down to platform admins
/// only (403 otherwise).
pub fn require_platform_admin(principal: &Principal) -> Result<(), CpError> {
    if principal.platform_roles.iter().any(|r| r == PLATFORM_ADMIN) {
        Ok(())
    } else {
        Err(CpError::Forbidden)
    }
}

/// Gate a control-plane action on a specific permission key.
/// `platform_admin` always passes (superuser). Others must hold the key.
pub fn require_permission(principal: &Principal, permission: &str) -> Result<(), CpError> {
    if principal.platform_roles.iter().any(|r| r == PLATFORM_ADMIN)
        || principal.permissions.iter().any(|p| p == permission)
    {
        Ok(())
    } else {
        Err(CpError::Forbidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_core::ids::UserId;

    fn principal(roles: &[&str]) -> Principal {
        Principal {
            user_id: UserId::new(),
            email: "a@b.c".into(),
            platform_roles: roles.iter().map(|s| s.to_string()).collect(),
            permissions: vec![],
        }
    }

    fn principal_with(roles: &[&str], perms: &[&str]) -> Principal {
        Principal {
            user_id: UserId::new(),
            email: "a@b.c".into(),
            platform_roles: roles.iter().map(|s| s.to_string()).collect(),
            permissions: perms.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn admin_is_allowed() {
        assert!(require_platform_admin(&principal(&["platform_admin"])).is_ok());
    }

    #[test]
    fn non_admin_is_forbidden() {
        assert!(matches!(
            require_platform_admin(&principal(&["data_steward"])),
            Err(CpError::Forbidden)
        ));
        assert!(matches!(
            require_platform_admin(&principal(&[])),
            Err(CpError::Forbidden)
        ));
    }

    #[test]
    fn permission_allows_when_held() {
        let p = principal_with(&[], &["engine:create"]);
        assert!(require_permission(&p, "engine:create").is_ok());
    }

    #[test]
    fn permission_denies_when_missing() {
        let p = principal_with(&[], &["engine:read"]);
        assert!(require_permission(&p, "engine:create").is_err());
    }

    #[test]
    fn platform_admin_bypasses_any_permission() {
        let p = principal_with(&["platform_admin"], &[]);
        assert!(require_permission(&p, "engine:create").is_ok());
        assert!(require_permission(&p, "role:delete").is_ok());
    }
}
