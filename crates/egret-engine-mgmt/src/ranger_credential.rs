//! The per-engine Ranger plugin credential (spec 2026-10-20-ranger-surface-auth
//! §4.3, A10).
//!
//! An engine's Ranger plugin authenticates to egret's `/ranger` surface with HTTP
//! Basic `engine-<uuid>` ([`plugin_username`]) and a per-engine password. The
//! password is a named secret in the GLOBAL scope of the secret store, named
//! `engine-ranger-{engine_id}-{generation}`; the engine row names it in two
//! columns:
//!
//! * `ranger_secret_id` — the credential the plugin is (or is about to be)
//!   rendered with;
//! * `ranger_secret_prev_id` — the one it was rendered with before the last
//!   rotation.
//!
//! **Both verify.** A rotation commits the new current and demotes the old one
//! to previous BEFORE the apply that renders it, so pods still running with the
//! old password keep authenticating while they roll. The previous one is deleted
//! by its exact id only after a successful apply ([`retire_previous`]), or when a
//! LATER rotation pushes it out of the two-slot window — never by a prefix sweep
//! (that happens only at engine delete, when no apply can still be in flight).
//! This is the standing rule: never delete a secret value the cluster may still
//! hold.
//!
//! Unlike the Trino-only internal/monitor pair (`rotate_engine_secrets`), this
//! credential exists for every engine kind, StarRocks included.

use egret_engine_mgmt_api::credential::{
    plugin_username, EngineCredentialVerifier, EngineIdentity,
};
use egret_secrets::{SecretsError, WorkspaceSecretStore};
use uuid::Uuid;

use crate::{Engine, EngineHttpError as CpError, EngineMgmtState as CpState, EngineMgmtStore};

/// The prefix every credential secret of `engine_id` is named with. Engine
/// delete sweeps it; nothing else may.
pub(crate) fn secret_name_prefix(engine_id: Uuid) -> String {
    format!("engine-ranger-{engine_id}-")
}

/// A per-generation suffix keeps a rotation from colliding with the name of the
/// credential it demotes (that one stays until it is retired).
fn secret_name(engine_id: Uuid) -> String {
    format!(
        "{}{}",
        secret_name_prefix(engine_id),
        Uuid::new_v4().simple()
    )
}

/// The credential to render into an engine's plugin config.
#[derive(Clone, PartialEq, Eq)]
pub struct PluginCredential {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for PluginCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginCredential")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// The credential store: the engine rows that name the credentials and the
/// secret store that holds them. Everything here runs in the secret store's
/// global (`None`) scope.
#[derive(Clone)]
pub struct RangerCredentialStore {
    store: EngineMgmtStore,
    secrets: WorkspaceSecretStore,
}

impl RangerCredentialStore {
    pub fn new(store: EngineMgmtStore, secrets: WorkspaceSecretStore) -> Self {
        Self { store, secrets }
    }

    pub fn from_state(st: &CpState) -> Self {
        Self::new(st.engine_mgmt.clone(), st.secrets.clone())
    }

    /// The credential to render into the engine's plugin config: the committed
    /// one, or — when the row has none, or its secret vanished out-of-band — a
    /// freshly minted one, committed before it is returned.
    pub async fn ensure(&self, engine: &Engine) -> Result<PluginCredential, CpError> {
        if let Some(found) = self.resolve_current(engine).await? {
            return Ok(found);
        }
        self.mint_and_commit(engine, false).await
    }

    /// Mint a new current credential, demote the old current to previous, and
    /// return the new one. A previous credential the demotion pushes out of the
    /// window is deleted (exact id).
    pub async fn rotate(&self, engine: &Engine) -> Result<PluginCredential, CpError> {
        self.mint_and_commit(engine, true).await
    }

    /// After a successful apply: delete the previous credential's secret and
    /// clear its column. Failures are logged, never returned — the apply has
    /// already succeeded, and an unretired previous credential only stays valid
    /// a little longer.
    pub async fn retire_previous(&self, engine_id: Uuid) {
        let prev = match self.store.get_engine(engine_id).await {
            Ok(Some(e)) => e.ranger_secret_prev_id,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(%engine_id, error = %err,
                    "could not read the engine to retire its previous ranger credential");
                return;
            }
        };
        if let Some(prev) = prev {
            self.retire_previous_if(engine_id, prev).await;
        }
    }

    /// The compare-and-set half of [`Self::retire_previous`]: clear the column
    /// only while it still names `prev`, and delete `prev` only when that clear
    /// won. A lost clear means a newer rotation owns the column (and `prev` may
    /// now be someone's current credential), so nothing is deleted.
    pub async fn retire_previous_if(&self, engine_id: Uuid, prev: Uuid) {
        match self.store.clear_ranger_prev(engine_id, prev).await {
            Ok(true) => match self.secrets.delete(None, prev).await {
                Ok(()) | Err(SecretsError::NotFound) => {}
                Err(err) => tracing::warn!(%engine_id, secret_id = %prev, error = %err,
                    "could not delete a retired ranger credential"),
            },
            Ok(false) => {}
            Err(err) => tracing::warn!(%engine_id, secret_id = %prev, error = %err,
                "could not clear the previous ranger credential"),
        }
    }

    /// The committed current credential, `None` when the row names none or its
    /// secret is gone.
    async fn resolve_current(&self, engine: &Engine) -> Result<Option<PluginCredential>, CpError> {
        let Some(id) = engine.ranger_secret_id else {
            return Ok(None);
        };
        match self.secrets.resolve_plaintext(None, id).await {
            Ok(password) => Ok(Some(PluginCredential {
                username: plugin_username(engine.id),
                password,
            })),
            // Deleted out-of-band: the caller mints a fresh one rather than
            // failing every future apply on this engine.
            Err(SecretsError::NotFound) => Ok(None),
            Err(e) => Err(CpError::BadRequest(format!(
                "resolve ranger credential: {e}"
            ))),
        }
    }

    async fn mint_and_commit(
        &self,
        engine: &Engine,
        demote: bool,
    ) -> Result<PluginCredential, CpError> {
        let password = crate::engines::random_internal_secret();
        let new_id = self
            .secrets
            .create(
                None,
                &secret_name(engine.id),
                Some("Ranger plugin credential"),
                &password,
            )
            .await
            .map_err(|e| CpError::BadRequest(format!("store ranger credential: {e}")))?;
        // `ensure` keeps the previous slot as it is; `rotate` moves the current
        // one into it.
        let demoted = if demote {
            engine.ranger_secret_id
        } else {
            engine.ranger_secret_prev_id
        };
        // CAS on the current id THIS call read, like `commit_internal_secrets`:
        // a concurrent writer wins cleanly and this call adopts its credential.
        let committed = match self
            .store
            .set_ranger_secret_ids(engine.id, engine.ranger_secret_id, new_id, demoted)
            .await
        {
            Ok(c) => c,
            Err(e) => {
                // Never recorded and never handed to a cluster: discarding it is safe.
                self.discard_unrecorded(engine.id, new_id).await;
                return Err(e.into());
            }
        };
        if !committed {
            self.discard_unrecorded(engine.id, new_id).await;
            let fresh = self
                .store
                .get_engine(engine.id)
                .await?
                .ok_or(CpError::NotFound)?;
            return match self.resolve_current(&fresh).await? {
                Some(winner) => Ok(winner),
                None => {
                    tracing::error!(engine_id = %engine.id,
                        "lost the race to record a ranger credential, and the winner's is unreadable");
                    Err(CpError::Internal)
                }
            };
        }
        // The previous credential a rotation pushed out of the two-slot window.
        // It was already superseded by `engine.ranger_secret_id` at the LAST
        // rotation; nothing can still verify with it once the row no longer
        // names it.
        if demote {
            if let Some(older) = engine
                .ranger_secret_prev_id
                .filter(|o| Some(*o) != engine.ranger_secret_id)
            {
                match self.secrets.delete(None, older).await {
                    Ok(()) | Err(SecretsError::NotFound) => {}
                    Err(e) => {
                        tracing::warn!(engine_id = %engine.id, secret_id = %older, error = %e,
                        "could not delete a superseded ranger credential")
                    }
                }
            }
        }
        Ok(PluginCredential {
            username: plugin_username(engine.id),
            password,
        })
    }

    /// Delete a credential this call minted but never recorded on the row.
    async fn discard_unrecorded(&self, engine_id: Uuid, id: Uuid) {
        if let Err(e) = self.secrets.delete(None, id).await {
            tracing::warn!(%engine_id, secret_id = %id, error = %e,
                "could not delete an unrecorded ranger credential");
        }
    }
}

/// [`RangerCredentialStore::ensure`] on the seam's state.
pub async fn ensure(st: &CpState, engine: &Engine) -> Result<PluginCredential, CpError> {
    RangerCredentialStore::from_state(st).ensure(engine).await
}

/// [`RangerCredentialStore::rotate`] on the seam's state.
pub async fn rotate(st: &CpState, engine: &Engine) -> Result<PluginCredential, CpError> {
    RangerCredentialStore::from_state(st).rotate(engine).await
}

/// [`RangerCredentialStore::retire_previous`] on the seam's state.
pub async fn retire_previous(st: &CpState, engine_id: Uuid) {
    RangerCredentialStore::from_state(st)
        .retire_previous(engine_id)
        .await
}

/// The verifier the `/ranger` surface consults: accepts the current OR the
/// previous credential of the engine (A10).
///
/// It does the same comparison work whatever it finds: two constant-time
/// comparisons always, against a dummy for an empty slot or an unknown engine,
/// and every slot is compared (no early return on a match), so the time does
/// not say which slot matched or whether one existed. An empty password never
/// matches. No secret value appears in an `Err` (they are logged by the caller).
#[derive(Clone)]
pub struct DbVerifier {
    creds: RangerCredentialStore,
}

impl DbVerifier {
    pub fn new(creds: RangerCredentialStore) -> Self {
        Self { creds }
    }
}

/// Compared against when a slot holds nothing. Never accepted: a match counts
/// only for a slot that resolved to a stored credential. Same length as a real
/// credential (32 bytes, URL-safe base64, no padding).
const DUMMY_CREDENTIAL: &str = "egret-no-credential-egret-no-credential-egr";

#[async_trait::async_trait]
impl EngineCredentialVerifier for DbVerifier {
    async fn verify(
        &self,
        engine_id: Uuid,
        password: &str,
    ) -> Result<Option<EngineIdentity>, String> {
        let engine = self
            .creds
            .store
            .get_engine(engine_id)
            .await
            .map_err(|e| format!("load engine: {e}"))?;
        let slots = engine.as_ref().map_or([None, None], |e| {
            [e.ranger_secret_id, e.ranger_secret_prev_id]
        });
        let mut matched = false;
        for slot in slots {
            let stored = match slot {
                Some(id) => match self.creds.secrets.resolve_plaintext(None, id).await {
                    Ok(v) => Some(v),
                    Err(SecretsError::NotFound) => None,
                    Err(e) => return Err(format!("resolve ranger credential {id}: {e}")),
                },
                None => None,
            };
            let candidate = stored.as_deref().unwrap_or(DUMMY_CREDENTIAL);
            let equal = ct_eq(password.as_bytes(), candidate.as_bytes());
            matched |= equal & stored.is_some();
        }
        let accepted = matched & !password.is_empty();
        Ok(match engine {
            Some(e) if accepted => Some(EngineIdentity {
                engine_id: e.id,
                authz_service_id: e.authz_service_id,
            }),
            _ => None,
        })
    }
}

/// Constant-time equality. The work depends only on `presented`'s length (the
/// caller's own input), never on `stored`'s, and a length mismatch is folded
/// into the result instead of returning early.
fn ct_eq(presented: &[u8], stored: &[u8]) -> bool {
    let mut diff = presented.len() ^ stored.len();
    for (i, x) in presented.iter().enumerate() {
        let y = stored.get(i).copied().unwrap_or(0);
        diff |= usize::from(x ^ y);
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_is_plain_equality() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(ct_eq(b"", b""));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
        assert!(!ct_eq(b"abcd", b"abc"));
        assert!(!ct_eq(b"", b"a"));
        // A prefix padded with the zero byte the loop substitutes is not equal.
        assert!(!ct_eq(b"ab\0", b"ab"));
    }

    #[test]
    fn the_dummy_has_the_length_of_a_real_credential() {
        assert_eq!(
            DUMMY_CREDENTIAL.len(),
            crate::engines::random_internal_secret().len()
        );
    }

    #[test]
    fn credential_names_share_the_engine_prefix_and_differ_per_generation() {
        let id = Uuid::new_v4();
        let (a, b) = (secret_name(id), secret_name(id));
        assert!(a.starts_with(&secret_name_prefix(id)));
        assert!(b.starts_with(&secret_name_prefix(id)));
        assert_ne!(a, b);
    }
}
