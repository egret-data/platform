//! Platform authorization gate — permission / platform-admin checks over a
//! [`Principal`]. Returns [`egret_core::Error::Forbidden`] on denial (maps to
//! 403 via the shared HTTP mapping). Callable from any crate, so seams can gate
//! without depending on `egret-controlplane`.

use egret_core::{Error, Principal};

/// The platform role that bypasses permission checks (superuser).
const PLATFORM_ADMIN: &str = "platform_admin";

/// Gate an action on a specific permission key. `platform_admin` always passes;
/// others must hold the key. `Err(Error::Forbidden)` on denial.
pub fn require_permission(principal: &Principal, permission: &str) -> Result<(), Error> {
    if principal.platform_roles.iter().any(|r| r == PLATFORM_ADMIN)
        || principal.permissions.iter().any(|p| p == permission)
    {
        Ok(())
    } else {
        Err(Error::Forbidden)
    }
}

/// Hard `platform_admin`-only gate (no bypass — it *is* the admin check).
pub fn require_platform_admin(principal: &Principal) -> Result<(), Error> {
    if principal.platform_roles.iter().any(|r| r == PLATFORM_ADMIN) {
        Ok(())
    } else {
        Err(Error::Forbidden)
    }
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

    #[test]
    fn admin_passes_platform_admin_gate() {
        assert!(require_platform_admin(&principal(&["platform_admin"], &[])).is_ok());
    }

    #[test]
    fn non_admin_forbidden_on_platform_admin_gate() {
        assert!(matches!(
            require_platform_admin(&principal(&["data_steward"], &[])),
            Err(Error::Forbidden)
        ));
        assert!(matches!(
            require_platform_admin(&principal(&[], &[])),
            Err(Error::Forbidden)
        ));
    }

    #[test]
    fn permission_allows_when_held() {
        assert!(require_permission(&principal(&[], &["engine:create"]), "engine:create").is_ok());
    }

    #[test]
    fn permission_forbidden_when_missing() {
        assert!(matches!(
            require_permission(&principal(&[], &["engine:read"]), "engine:create"),
            Err(Error::Forbidden)
        ));
    }

    #[test]
    fn platform_admin_bypasses_any_permission() {
        let p = principal(&["platform_admin"], &[]);
        assert!(require_permission(&p, "engine:create").is_ok());
        assert!(require_permission(&p, "role:delete").is_ok());
    }
}
