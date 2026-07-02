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

#[cfg(test)]
mod tests {
    use super::*;
    use egret_core::ids::UserId;

    fn principal(roles: &[&str]) -> Principal {
        Principal {
            user_id: UserId::new(),
            email: "a@b.c".into(),
            platform_roles: roles.iter().map(|s| s.to_string()).collect(),
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
}
