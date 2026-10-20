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
//! to previous BEFORE the apply that renders it, so both authenticate while the
//! engine's pods roll.
//!
//! **The previous one is retired only once the rollout is OBSERVED complete**,
//! never when Kubernetes merely accepts the apply: until then a pod of the old
//! template still holds the old password in its mounted copy of the Secret, and
//! deleting it would stop that pod's policy downloads (revocations and masks
//! would silently stop applying). Every apply stamps the current credential's
//! id on the pod templates and on the Secret (`egret_provisioner::rollout`);
//! [`RangerCredentialStore::retire_previous_if_rolled_out`] asks the
//! provisioner whether the pods have rolled out with THAT id, and only then
//! retires the previous one, by its exact id, in one compare-and-set on both
//! columns (current = the id the rollout was observed for, previous = the id
//! read). A rollout observed for an older credential therefore never retires
//! the previous one of a newer rotation. Each credential is rendered into its own
//! Kubernetes Secret, `{name}-rc-{tag}` (a short prefix of its id, distinct
//! between the current and previous credential), so a pod of a template that names a
//! credential can only mount that credential's content; the retired one's
//! Secret is deleted by exact name after the retire. It runs on a periodic sweep
//! ([`spawn_ranger_retire_sweeper`]) and on every read of the engine; anything
//! absent, unknown, partial or failing keeps the previous credential.
//!
//! Never by a prefix sweep (that happens only at engine delete). A rotation is
//! REFUSED while the previous credential is still pending retirement, so
//! nothing ever pushes a credential the cluster may hold out of the two-slot
//! window. This is the standing rule: never delete a secret value the cluster
//! may still hold.
//!
//! **A known dead end, left to an operator.** A row whose CURRENT credential
//! cannot be rendered (only one minted before the renderer's check existed can
//! be: `mint_password` re-draws a `--`) AND whose previous one is pending is
//! stuck: every apply is refused (409, unrenderable), so the rollout of the
//! current one never completes, and a rotation is refused (409, pending). egret
//! does not break it by overwriting the previous slot: whether the cluster
//! still holds that credential depends on the engine's history, which the row
//! does not record, so it cannot be proved safe. Such rows exist only in
//! development databases. To recover, an operator who has checked that no pod
//! still runs the previous credential clears `ranger_secret_prev_id` and
//! deletes that secret by hand (or deletes the engine).
//!
//! Unlike the Trino-only internal/monitor pair (`rotate_engine_secrets`), this
//! credential exists for every engine kind, StarRocks included.

use egret_engine_mgmt_api::credential::{
    plugin_username, EngineCredentialVerifier, EngineIdentity,
};
use egret_engine_mgmt_api::{EngineProvisioningPort, RangerRollout};
use egret_secrets::{SecretsError, WorkspaceSecretStore};
use uuid::Uuid;

use crate::{Engine, EngineHttpError as CpError, EngineMgmtState as CpState, EngineMgmtStore};

/// How many draws [`RangerCredentialStore::mint_with_a_free_tag`] makes.
const TAG_ATTEMPTS: usize = 8;

/// Whether credential `new`'s Secret tag differs from every one of `others`.
pub(crate) fn tag_is_free(new: Uuid, others: &[Option<Uuid>]) -> bool {
    use egret_engine_mgmt_api::ranger_config::ranger_secret_tag;
    let tag = ranger_secret_tag(&new.to_string());
    others
        .iter()
        .flatten()
        .all(|o| ranger_secret_tag(&o.to_string()) != tag)
}

/// How many times `ensure` re-reads the row after a lost compare-and-set.
const ENSURE_ATTEMPTS: usize = 3;

/// The 409 a provisioned engine's rotation gets while the previous credential
/// is still pending.
const ROTATION_PENDING: &str = "a ranger credential rotation is pending: the previous credential \
     stays valid until the engine's pods are observed to have rolled out with the current one; \
     retry once that rollout has completed";

/// The 409 an external engine's rotation gets while the previous credential is
/// still pending: only the operator can say the new one is installed.
const ROTATION_PENDING_EXTERNAL: &str = "a ranger credential rotation is pending: the previous \
     credential is still valid until you confirm the current one is installed on the engine with \
     POST /api/control-plane/engines/{id}/retire-previous-ranger-credential";

/// The 409 an external engine's rotation gets when another rotation won.
const ROTATION_RACED: &str = "a concurrent ranger credential rotation changed this engine: \
     re-read the engine and retry";

/// The 409 a confirmed retire gets when the operator's statement does not
/// match the row.
const RETIRE_MISMATCH: &str = "the credential named as installed is not this engine's current \
     ranger credential (a later rotation, or a concurrent change): nothing was retired; install \
     the current credential and confirm it";

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

/// A fresh credential password: 32 random bytes, URL-safe base64 (43 chars).
///
/// It is rendered UNESCAPED into the plugin's security XML, so it is re-drawn
/// while it contains `--` (about one draw in a hundred): the renderer refuses
/// one, and StarRocks' FE refuses a double hyphen in its config XML. The
/// alphabet already excludes `<` and `&`. The re-draw costs well under a bit of
/// the 256 bits of entropy.
pub(crate) fn mint_password() -> String {
    loop {
        let password = crate::engines::random_internal_secret();
        if !password.contains("--") {
            return password;
        }
    }
}

/// The credential to render into an engine's plugin config.
#[derive(Clone, PartialEq, Eq)]
pub struct PluginCredential {
    /// The id of the secret that holds the password: the non-secret name of
    /// this credential generation, stamped on the pods it is rendered into.
    pub id: Uuid,
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for PluginCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginCredential")
            .field("id", &self.id)
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
    /// freshly minted one, committed before it is returned. Never touches the
    /// previous slot. A lost compare-and-set re-reads the row and tries again
    /// (bounded), so a concurrent writer's credential is adopted, not replaced.
    pub async fn ensure(&self, engine: &Engine) -> Result<PluginCredential, CpError> {
        let mut row = engine.clone();
        for _ in 0..ENSURE_ATTEMPTS {
            if let Some(found) = self.resolve_current(&row).await? {
                return Ok(found);
            }
            match self.mint_and_commit(&row, false).await? {
                Some((_, minted)) => return Ok(minted),
                None => row = self.reload(engine.id).await?,
            }
        }
        tracing::error!(engine_id = %engine.id,
            "could not settle a ranger credential: every compare-and-set lost");
        Err(CpError::Internal)
    }

    /// Mint a new current credential, demote the old current to previous, and
    /// return the new one.
    ///
    /// **Refused (409) while a rotation is pending**, i.e. while the row names a
    /// previous credential whose secret still exists: the rollout of the
    /// current one has not been observed complete yet, so the cluster may
    /// still hold the previous one, and a second rotation
    /// would have to drop it from the two-slot window. A previous id whose
    /// secret is already gone (dangling) is simply overwritten.
    ///
    /// On a lost compare-and-set (a concurrent writer changed either column
    /// since `engine` was read) this returns the WINNER's current credential,
    /// not a new one; callers that need the row must re-read it.
    pub async fn rotate(&self, engine: &Engine) -> Result<PluginCredential, CpError> {
        self.refuse_if_pending(engine, ROTATION_PENDING).await?;
        if let Some((_, minted)) = self.mint_and_commit(engine, true).await? {
            return Ok(minted);
        }
        let fresh = self.reload(engine.id).await?;
        match self.resolve_current(&fresh).await? {
            Some(winner) => Ok(winner),
            None => {
                tracing::error!(engine_id = %engine.id,
                    "lost the race to rotate a ranger credential, and the winner's is unreadable");
                Err(CpError::Internal)
            }
        }
    }

    /// [`Self::rotate`] for an EXTERNAL engine: no apply follows, the
    /// operator installs the returned credential. Returns the new current
    /// credential's id (the `credentialId` the operator later names as
    /// installed) with it. The pending refusal names the retire route, and a
    /// lost compare-and-set is a 409 (re-read and retry) — never the winner's
    /// credential handed out as if it were new.
    pub async fn rotate_external(
        &self,
        engine: &Engine,
    ) -> Result<(Uuid, PluginCredential), CpError> {
        let pending = ROTATION_PENDING_EXTERNAL.replace("{id}", &engine.id.to_string());
        self.refuse_if_pending(engine, &pending).await?;
        self.mint_and_commit(engine, true)
            .await?
            .ok_or_else(|| CpError::Conflict(ROTATION_RACED.into()))
    }

    /// 409 `message` while the row names a previous credential whose secret
    /// still exists. A dangling previous id does not block.
    async fn refuse_if_pending(&self, engine: &Engine, message: &str) -> Result<(), CpError> {
        let Some(prev) = engine.ranger_secret_prev_id else {
            return Ok(());
        };
        match self.secrets.resolve_plaintext(None, prev).await {
            Ok(_) => Err(CpError::Conflict(message.into())),
            Err(SecretsError::NotFound) => Ok(()),
            Err(e) => Err(CpError::BadRequest(format!(
                "resolve previous ranger credential: {e}"
            ))),
        }
    }

    /// Delete the previous credential's secret and clear its column,
    /// UNCONDITIONALLY — whatever the cluster holds. Test-only (feature
    /// `test-util`), so no production path can call it. No route or sweep calls
    /// it: a provisioned engine retires through
    /// [`Self::retire_previous_if_rolled_out`], an external one through
    /// [`Self::retire_previous_confirmed`]. Failures are logged, never returned.
    #[cfg(any(test, feature = "test-util"))]
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

    /// Retire a PROVISIONED engine's previous credential once the rollout of
    /// its current one is observed complete (module doc; spec A10).
    ///
    /// `engine` is the row as last read. The provisioner is asked whether the
    /// pods run the credential the row names as current — by its id, which the
    /// apply stamped on the pod templates — and only on
    /// [`RangerRollout::Complete`] is the previous one retired: by its exact
    /// id, in one compare-and-set on BOTH columns, so a row that changed since
    /// it was read (a later rotation, an engine delete) retires nothing. The
    /// secret is deleted only when that clear won.
    ///
    /// Fails closed: an external engine (only its operator's confirmation
    /// retires), an engine being deleted (the engine delete owns its
    /// credentials), a row with no current credential, a pending rollout and a
    /// failed probe all keep the previous credential. Never returns an error
    /// and never panics: a background sweep and a read call it.
    pub async fn retire_previous_if_rolled_out(
        &self,
        engine: &Engine,
        port: &dyn EngineProvisioningPort,
    ) -> RolloutRetire {
        let Some(prev) = engine.ranger_secret_prev_id else {
            return RolloutRetire::NothingPending;
        };
        if crate::external::is_external(engine) {
            return RolloutRetire::Kept(
                "external engine: only its operator's confirmation retires the previous \
                 credential"
                    .into(),
            );
        }
        if engine.status == crate::engine_status::DELETING {
            return RolloutRetire::Kept("the engine is being deleted".into());
        }
        let Some(current) = engine.ranger_secret_id else {
            return RolloutRetire::Kept("the engine names no current credential".into());
        };
        let probe = tokio::time::timeout(
            KUBE_CALL_TIMEOUT,
            port.ranger_rollout(
                &engine.r#type,
                &engine.namespace,
                &engine.name,
                &current.to_string(),
            ),
        )
        .await
        .unwrap_or_else(|_| Err("the rollout probe timed out".into()));
        match probe {
            Ok(RangerRollout::Complete) => {}
            Ok(RangerRollout::Pending(reason)) => return RolloutRetire::Kept(reason),
            Err(err) => {
                tracing::warn!(engine = %engine.name, error = %err,
                    "could not observe the ranger credential rollout; the previous credential \
                     is kept");
                return RolloutRetire::Kept(format!("rollout probe failed: {err}"));
            }
        }
        match self
            .store
            .clear_ranger_prev_if_current(engine.id, current, prev)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                return RolloutRetire::Kept("the engine's credentials changed concurrently".into())
            }
            Err(err) => {
                tracing::warn!(engine = %engine.name, error = %err,
                    "could not clear the previous ranger credential");
                return RolloutRetire::Kept(format!("could not clear it: {err}"));
            }
        }
        match self.secrets.delete(None, prev).await {
            Ok(()) | Err(SecretsError::NotFound) => {}
            Err(err) => tracing::warn!(engine = %engine.name, secret_id = %prev, error = %err,
                "could not delete a retired ranger credential; the engine-delete sweep will"),
        }
        // Its Kubernetes Secret (`{name}-rc-{tag of prev}`): no pod mounts it, the
        // rollout of the current one's having just been observed complete.
        let deleted = tokio::time::timeout(
            KUBE_CALL_TIMEOUT,
            port.delete_ranger_secret(
                &engine.r#type,
                &engine.namespace,
                &engine.name,
                &prev.to_string(),
            ),
        )
        .await
        .unwrap_or_else(|_| Err("timed out".into()));
        if let Err(err) = deleted {
            tracing::warn!(engine = %engine.name, secret_id = %prev, error = %err,
                "could not delete a retired ranger credential's Kubernetes Secret; engine \
                 delete removes it");
        }
        tracing::info!(engine = %engine.name, secret_id = %prev,
            "the rollout of the current ranger credential is complete; the previous one is retired");
        RolloutRetire::Retired(prev)
    }

    /// Test-only (feature `test-util`), like [`Self::retire_previous`]: it
    /// is just as blind to what the cluster holds.
    ///
    /// The compare-and-set half of [`Self::retire_previous`]: clear the column
    /// only while it still names `prev`, and delete `prev` only when that clear
    /// won. A lost clear means a newer rotation owns the column (and `prev` may
    /// now be someone's current credential), so nothing is deleted.
    #[cfg(any(test, feature = "test-util"))]
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
                id,
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

    async fn reload(&self, engine_id: Uuid) -> Result<Engine, CpError> {
        self.store
            .get_engine(engine_id)
            .await?
            .ok_or(CpError::NotFound)
    }

    /// Mint a credential and record it, compare-and-set on BOTH columns as
    /// `engine` names them. `Ok(None)`: the compare-and-set lost (nothing was
    /// recorded, and the minted secret — never handed to anyone — is deleted).
    /// Deletes nothing else: `ensure` keeps the previous slot, `rotate` moves
    /// the current one into it.
    async fn mint_and_commit(
        &self,
        engine: &Engine,
        demote: bool,
    ) -> Result<Option<(Uuid, PluginCredential)>, CpError> {
        let (password, new_id) = self.mint_with_a_free_tag(engine).await?;
        let previous = if demote {
            engine.ranger_secret_id
        } else {
            engine.ranger_secret_prev_id
        };
        let committed = match self
            .store
            .set_ranger_secret_ids(
                engine.id,
                engine.ranger_secret_id,
                engine.ranger_secret_prev_id,
                new_id,
                previous,
            )
            .await
        {
            Ok(c) => c,
            Err(e) => {
                self.discard_unrecorded(engine.id, new_id).await;
                return Err(e.into());
            }
        };
        if !committed {
            self.discard_unrecorded(engine.id, new_id).await;
            return Ok(None);
        }
        Ok(Some((
            new_id,
            PluginCredential {
                id: new_id,
                username: plugin_username(engine.id),
                password,
            },
        )))
    }

    /// Mint and store a credential whose Kubernetes Secret tag
    /// ([`egret_engine_mgmt_api::ranger_config::ranger_secret_tag`]) differs
    /// from both credentials the row names: the current and previous
    /// credential of one engine are rendered into DIFFERENT Secrets, so the
    /// retire of one can never delete the other's. A colliding draw (about one
    /// in 16 million) is discarded — never handed out — and re-drawn.
    async fn mint_with_a_free_tag(&self, engine: &Engine) -> Result<(String, Uuid), CpError> {
        for _ in 0..TAG_ATTEMPTS {
            let password = mint_password();
            let id = self
                .secrets
                .create(
                    None,
                    &secret_name(engine.id),
                    Some("Ranger plugin credential"),
                    &password,
                )
                .await
                .map_err(|e| CpError::BadRequest(format!("store ranger credential: {e}")))?;
            if tag_is_free(id, &[engine.ranger_secret_id, engine.ranger_secret_prev_id]) {
                return Ok((password, id));
            }
            self.discard_unrecorded(engine.id, id).await;
        }
        tracing::error!(engine_id = %engine.id,
            "could not mint a ranger credential with a Secret tag distinct from the engine's");
        Err(CpError::Internal)
    }

    /// Mint a credential for an engine row that does not exist yet, WITHOUT
    /// recording it anywhere: the caller writes the returned id into the row
    /// it inserts (external registration, so the row and its credential commit
    /// together), and calls [`Self::discard_unrecorded`] if that insert does
    /// not commit. Nobody has been handed the password until the caller does.
    pub(crate) async fn mint_unrecorded(
        &self,
        engine_id: Uuid,
    ) -> Result<(Uuid, PluginCredential), CpError> {
        let password = mint_password();
        let id = self
            .secrets
            .create(
                None,
                &secret_name(engine_id),
                Some("Ranger plugin credential"),
                &password,
            )
            .await
            .map_err(|e| CpError::BadRequest(format!("store ranger credential: {e}")))?;
        Ok((
            id,
            PluginCredential {
                id,
                username: plugin_username(engine_id),
                password,
            },
        ))
    }

    /// The operator's confirmation that an EXTERNAL engine's plugin now runs
    /// with the credential `installed` (the `credentialId` registration or the
    /// last rotation returned): retire the previous one by its exact id.
    ///
    /// It is a statement about WHAT is installed, checked on BOTH columns in
    /// one compare-and-set (current = `installed` AND previous = the id read
    /// from the row). A confirmation that names anything but the current
    /// credential — a stale or duplicated request sent before a later rotation,
    /// whose previous credential the engine may still hold — is a 409 and
    /// deletes nothing. With the matching statement and nothing pending it is
    /// `Ok(())` (idempotent). The secret is deleted only when the clear won.
    ///
    /// Unlike the after-apply retire, a failure to clear the column is
    /// returned: the operator asked for this and must know it did not happen.
    /// Once the column is cleared the credential no longer verifies; a failed
    /// delete of its secret only leaves an orphan for the engine-delete sweep.
    pub async fn retire_previous_confirmed(
        &self,
        engine: &Engine,
        installed: Uuid,
    ) -> Result<(), CpError> {
        if engine.ranger_secret_id != Some(installed) {
            return Err(CpError::Conflict(RETIRE_MISMATCH.into()));
        }
        let Some(prev) = engine.ranger_secret_prev_id else {
            return Ok(());
        };
        if !self
            .store
            .clear_ranger_prev_if_current(engine.id, installed, prev)
            .await?
        {
            return Err(CpError::Conflict(RETIRE_MISMATCH.into()));
        }
        match self.secrets.delete(None, prev).await {
            Ok(()) | Err(SecretsError::NotFound) => {}
            Err(err) => tracing::warn!(engine_id = %engine.id, secret_id = %prev, error = %err,
                "could not delete a retired ranger credential; the engine-delete sweep will"),
        }
        Ok(())
    }

    /// Delete a credential this call minted but never recorded on the row.
    pub(crate) async fn discard_unrecorded(&self, engine_id: Uuid, id: Uuid) {
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

/// What [`RangerCredentialStore::retire_previous_if_rolled_out`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RolloutRetire {
    /// The engine has no previous credential.
    NothingPending,
    /// The previous credential, this id, was retired.
    Retired(Uuid),
    /// The previous credential is kept, for this reason.
    Kept(String),
}

/// The bound on each Kubernetes call of a retire (the probe, the Secret
/// delete): a hung API server keeps the credential, it never stalls the sweep.
const KUBE_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// How often [`spawn_ranger_retire_sweeper`] looks at the engines whose
/// previous credential is pending.
pub const RANGER_RETIRE_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// One pass of the sweep: every provisioned engine whose previous credential is
/// pending (selected in SQL) gets [`RangerCredentialStore::retire_previous_if_rolled_out`].
/// Returns how many were retired. Every replica may run it: the retire is a
/// compare-and-set, so two passes over one engine retire it once.
pub async fn retire_rolled_out_previous_credentials(st: &CpState) -> usize {
    let Some(port) = st.engine_provisioner.as_ref() else {
        return 0;
    };
    let engines = match st.engine_mgmt.list_engines_with_pending_ranger_prev().await {
        Ok(engines) => engines,
        Err(err) => {
            tracing::warn!(error = %err,
                "could not list the engines to retire previous ranger credentials");
            return 0;
        }
    };
    let creds = RangerCredentialStore::from_state(st);
    let mut retired = 0;
    for engine in engines {
        // One task per engine: a panic in one engine's pass is caught at its
        // join and never ends the sweep (or the loop that runs it).
        let (creds, port, name) = (creds.clone(), port.clone(), engine.name.clone());
        let pass = tokio::spawn(async move {
            creds
                .retire_previous_if_rolled_out(&engine, port.as_ref())
                .await
        });
        match pass.await {
            Ok(RolloutRetire::Retired(_)) => retired += 1,
            Ok(RolloutRetire::Kept(reason)) => tracing::debug!(engine = %name, %reason,
                "the previous ranger credential is kept: rollout not observed complete"),
            Ok(RolloutRetire::NothingPending) => {}
            Err(err) => tracing::error!(engine = %name, error = %err,
                "the ranger credential retire pass failed; the previous credential is kept"),
        }
    }
    retired
}

/// Run [`retire_rolled_out_previous_credentials`] every `period` until
/// `shutdown` resolves. A pass longer than the period does not cause a burst.
pub fn spawn_ranger_retire_sweeper(
    st: CpState,
    period: std::time::Duration,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::pin!(shutdown);
        let mut tick = tokio::time::interval(period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                _ = tick.tick() => {
                    retire_rolled_out_previous_credentials(&st).await;
                }
            }
        }
    })
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

    /// The current and previous credential of one engine never share a
    /// Kubernetes Secret: a new credential whose tag equals either slot's is
    /// refused (and re-drawn by `mint_with_a_free_tag`).
    #[test]
    fn a_tag_shared_with_either_slot_is_not_free() {
        let id = |s: &str| Uuid::parse_str(s).unwrap();
        let current = id("0b5ad7e2-0000-4000-8000-000000000001");
        let previous = id("a1b2c3d4-0000-4000-8000-000000000002");
        let same_as_current = id("0b5ad7ff-1111-4111-8111-111111111111");
        let same_as_previous = id("a1b2c3ee-2222-4222-8222-222222222222");
        let fresh = id("99999999-3333-4333-8333-333333333333");
        let slots = [Some(current), Some(previous)];
        assert!(!tag_is_free(same_as_current, &slots));
        assert!(!tag_is_free(same_as_previous, &slots));
        assert!(tag_is_free(fresh, &slots));
        assert!(tag_is_free(same_as_current, &[None, Some(previous)]));
    }

    #[test]
    fn the_dummy_has_the_length_of_a_real_credential() {
        assert_eq!(DUMMY_CREDENTIAL.len(), mint_password().len());
    }

    #[test]
    fn credential_names_share_the_engine_prefix_and_differ_per_generation() {
        let id = Uuid::new_v4();
        let (a, b) = (secret_name(id), secret_name(id));
        assert!(a.starts_with(&secret_name_prefix(id)));
        assert!(b.starts_with(&secret_name_prefix(id)));
        assert_ne!(a, b);
    }

    /// The password is rendered unescaped into the plugin's security XML. URL-
    /// safe base64 never carries `<` or `&`, but about one 43-character draw in
    /// a hundred contains `--`, which the renderer refuses (StarRocks' FE and
    /// the renderer's debug assertion). 5000 draws without the re-draw would
    /// all but certainly hit one.
    #[test]
    fn a_minted_password_never_needs_escaping_in_the_plugin_xml() {
        for _ in 0..5000 {
            let p = mint_password();
            assert!(!p.contains("--"), "{p}");
            assert!(!p.contains('<') && !p.contains('&'));
            assert_eq!(p.len(), DUMMY_CREDENTIAL.len());
        }
    }
}
