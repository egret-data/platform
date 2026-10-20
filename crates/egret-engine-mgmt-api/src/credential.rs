//! The engine plugin credential contract for the Ranger surface.
//!
//! An engine's Ranger plugin authenticates to egret's `/ranger` surface with HTTP
//! Basic: username [`plugin_username`] (`engine-<uuid>`), password a per-engine
//! secret. The surface (`egret-policy`) checks it through an
//! [`EngineCredentialVerifier`]; the implementation that reads the credential
//! store lives with the engine-management code, so neither side depends on the
//! other.

use uuid::Uuid;

/// What a verified engine credential entitles its holder to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineIdentity {
    pub engine_id: Uuid,
    /// The policy profile (`policy.authz_services.id`) the engine is bound to.
    pub authz_service_id: Option<i64>,
}

/// The HTTP Basic username an engine's plugin presents.
pub fn plugin_username(engine_id: Uuid) -> String {
    format!("engine-{engine_id}")
}

/// The engine id inside a plugin username, if it is one.
pub fn engine_id_from_username(username: &str) -> Option<Uuid> {
    username
        .strip_prefix("engine-")
        .and_then(|s| Uuid::parse_str(s).ok())
}

#[async_trait::async_trait]
pub trait EngineCredentialVerifier: Send + Sync {
    /// `Ok(Some(_))` when `password` is a currently valid credential of `engine_id`;
    /// `Ok(None)` for any mismatch or an unknown engine (callers must not distinguish);
    /// `Err` only for an infrastructure failure.
    async fn verify(
        &self,
        engine_id: Uuid,
        password: &str,
    ) -> Result<Option<EngineIdentity>, String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_username_round_trips() {
        let id = Uuid::new_v4();
        assert_eq!(engine_id_from_username(&plugin_username(id)), Some(id));
    }

    #[test]
    fn a_non_engine_username_has_no_engine_id() {
        assert_eq!(engine_id_from_username("admin"), None);
        assert_eq!(engine_id_from_username("engine-"), None);
        assert_eq!(engine_id_from_username("engine-not-a-uuid"), None);
        let id = Uuid::new_v4();
        assert_eq!(engine_id_from_username(&format!("Engine-{id}")), None);
    }
}
