//! Registry catalogs ↔ engines (SP3 §4.4–4.5): render an attachment for Trino
//! or StarRocks, attach, detach, re-apply, and the per-engine-type dispatch.
//!
//! The two engine types differ in more than their rendered text (SP3b §1): a
//! Trino apply writes a Kubernetes object and re-renders every catalog every
//! time, so it is idempotent and prunes what it omits; a StarRocks apply is
//! one `CREATE`/`ALTER`/`DROP CATALOG` statement against a LIVE FE, which is
//! not idempotent, prunes nothing, and cannot be reached at all for the five
//! to ten minutes after the engine is provisioned. Everything asymmetric below
//! — attach ordering (D1), the read before the create (D3), the compensating
//! drop (D7) — follows from that.

use std::collections::BTreeMap;

use egret_catalog_api::{CreatedClient, NewClient};
use egret_catalog_registry::{
    render_starrocks, render_trino, AttachedCatalog, AttachmentRow, CatalogDef, CatalogRow,
    EngineType, NewAttachment, RenderError, StarRocksCatalogDdl, StarRocksRenderInput,
    StatementForm, StorageRender, TrinoCatalogFile, TrinoRenderInput, WarehouseRender,
};
use egret_storage::StorageConn;
use uuid::Uuid;

use crate::engines::{
    apply_built, apply_conflict, build_provision_req_with, ensure_engine_role, ApplyBuiltError,
    ApplyOp, BuildOpts, CatalogReq, PendingAttachment,
};
use crate::starrocks_ddl::{
    compare_to_fe, fe_target, show_catalogs, show_create_catalog, Drift, FeTarget,
};
use crate::Engine;
use crate::EngineHttpError as CpError;
use crate::EngineMgmtState as CpState;

#[derive(Debug)]
pub(crate) enum ApplyError {
    /// The engine row is gone or `deleting` (a concurrent delete).
    Conflict,
    /// The engine (Kubernetes, or a StarRocks FE) rejected the change, or
    /// never answered. Already scrubbed: a StarRocks FE error reaches this
    /// through `FeError::scrubbed` (D6), never through `Display`.
    Rejected(String),
    NotSupported(&'static str),
    /// The request could not be assembled (missing warehouse, secret, …).
    Build(CpError),
    /// The engine ACCEPTED the change but egret could not record it — the
    /// `set_apply_applied` write failed. Distinct from every variant above
    /// because it is not a statement about the engine: for StarRocks the
    /// catalog this apply created has already been dropped again (D7) by the
    /// time this is returned, so the engine and the registry agree once more.
    ///
    /// Two causes, and the callers must survive BOTH (fix round 1 review, F5 —
    /// this doc used to claim only the first): the attachment row VANISHED
    /// (`RegistryError::NotFound`, the race D7 is about), or an ordinary
    /// database failure hit the write while the row is still there. So the
    /// attach path removes the row best-effort before giving back what it
    /// minted — otherwise the second case leaves a `pending` attachment naming
    /// a credential secret that was just deleted — and `retry_apply` does NOT
    /// flag this on the row, because the row's status write is the thing that
    /// just failed.
    Unrecorded(CpError),
}

impl ApplyError {
    pub(crate) fn message(&self) -> String {
        match self {
            ApplyError::Conflict => {
                "engine changed while the update was being applied; retry".into()
            }
            ApplyError::Rejected(m) => m.clone(),
            ApplyError::NotSupported(m) => (*m).to_string(),
            ApplyError::Build(e) | ApplyError::Unrecorded(e) => e.to_string(),
        }
    }
}

pub(crate) enum CatalogChange {
    Attach(PendingAttachment),
    Detach(Uuid),
    /// Re-apply ONE attachment. Trino cannot express that — `apply_trino`
    /// re-renders the whole engine whatever changed, because a Trino apply is
    /// the entire ConfigMap either way — but a StarRocks apply is one
    /// statement about one catalog, and a Retry on catalog A must not restate
    /// catalog B or rewrite its `apply_status`. The whole-engine StarRocks
    /// convergence is the bootstrap's (D2), and it calls
    /// [`converge_attachment`] directly rather than coming through here.
    ReapplyOne(Uuid),
}

#[derive(Debug)]
pub(crate) enum CatalogOpError {
    Http(CpError),
    Apply(ApplyError),
}

impl From<CpError> for CatalogOpError {
    fn from(e: CpError) -> Self {
        CatalogOpError::Http(e)
    }
}

impl From<egret_catalog_registry::RegistryError> for CatalogOpError {
    fn from(e: egret_catalog_registry::RegistryError) -> Self {
        CatalogOpError::Http(e.into())
    }
}

impl From<egret_catalog_api::CatalogApiError> for CatalogOpError {
    fn from(e: egret_catalog_api::CatalogApiError) -> Self {
        CatalogOpError::Http(e.into())
    }
}

impl CatalogOpError {
    /// Today's engine-catalog routes: a rejected apply is a 400.
    pub(crate) fn into_legacy(self) -> CpError {
        match self {
            CatalogOpError::Http(e)
            | CatalogOpError::Apply(ApplyError::Build(e))
            | CatalogOpError::Apply(ApplyError::Unrecorded(e)) => e,
            CatalogOpError::Apply(ApplyError::Conflict) => apply_conflict(),
            CatalogOpError::Apply(ApplyError::Rejected(m)) => {
                CpError::BadRequest(format!("engine update failed: {m}"))
            }
            CatalogOpError::Apply(ApplyError::NotSupported(m)) => {
                CpError::BadRequest(m.to_string())
            }
        }
    }

    /// Registry routes: a rejected apply is a 502 (spec §4.4). Used by
    /// `catalogs.rs`'s `attach_catalog` and `retry_catalog_apply`.
    pub(crate) fn into_registry(self) -> CpError {
        match self {
            CatalogOpError::Apply(ApplyError::Rejected(m)) => CpError::ClusterApplyFailed(m),
            other => other.into_legacy(),
        }
    }
}

/// One engine's result of applying a catalog change.
#[derive(Clone, Debug)]
pub(crate) struct ApplyOutcome {
    pub(crate) engine_id: Uuid,
    pub(crate) engine_name: String,
    pub(crate) applied: bool,
    /// The engine was deliberately not touched (it is external); `error`
    /// carries the reason. Neither applied nor failed.
    pub(crate) skipped: bool,
    pub(crate) error: Option<String>,
}

/// Apply a catalog change to one engine, by engine type.
pub(crate) async fn apply_catalog_change(
    st: &CpState,
    engine: &Engine,
    change: CatalogChange,
) -> Result<(), ApplyError> {
    // The chokepoint under attach, detach, retry and propagate: whichever route
    // got here, an external engine's cluster is never rendered or applied to.
    // `Build` is not flagged on the attachment row, which is right: nothing was
    // said to the engine.
    crate::external::refuse_if_external(engine, "apply a catalog to it")
        .map_err(ApplyError::Build)?;
    match EngineType::parse(&engine.r#type) {
        Some(EngineType::Trino) => apply_trino(st, engine, change).await,
        Some(EngineType::StarRocks) => apply_starrocks(st, engine, change).await,
        None => Err(ApplyError::NotSupported("unknown engine type")),
    }
}

async fn apply_trino(
    st: &CpState,
    engine: &Engine,
    change: CatalogChange,
) -> Result<(), ApplyError> {
    let Some(prov) = st.engine_provisioner.as_ref() else {
        return Err(ApplyError::Build(CpError::BadRequest(
            "kubernetes not configured".into(),
        )));
    };
    let opts = match change {
        CatalogChange::Attach(p) => BuildOpts {
            pending_attachment: Some(p),
            ..BuildOpts::default()
        },
        CatalogChange::Detach(id) => BuildOpts {
            exclude_catalog: Some(id),
            ..BuildOpts::default()
        },
        // A Trino apply is the whole ConfigMap, so "re-apply this one
        // attachment" and "re-apply everything" are the same request.
        CatalogChange::ReapplyOne(_) => BuildOpts::default(),
    };
    let built = build_provision_req_with(st, engine, opts)
        .await
        .map_err(ApplyError::Build)?;
    apply_built(st, prov.clone(), engine, &built, ApplyOp::Update)
        .await
        .map_err(|e| match e {
            ApplyBuiltError::Conflict | ApplyBuiltError::CredentialChanged => ApplyError::Conflict,
            ApplyBuiltError::Failed(m) => ApplyError::Rejected(m),
        })
}

/// CREATE / ALTER / DROP CATALOG on the engine's live FE (SP3b). Every
/// statement goes through one [`FeTarget`], so D6's pinned certificate and the
/// FE root password cannot be forgotten on one path and passed on another the
/// way they were before SP3b.
async fn apply_starrocks(
    st: &CpState,
    engine: &Engine,
    change: CatalogChange,
) -> Result<(), ApplyError> {
    let fe = fe_target(st, engine).await.map_err(ApplyError::Build)?;
    let registry = st.engine_mgmt.registry();
    match change {
        CatalogChange::Attach(p) => {
            let catalog = load_catalog(st, p.catalog_id).await?;
            // The row `attach_to_engine` has just written, as this apply must
            // render it: the credential it minted is what the statement
            // carries, and it is not readable back through `get_attachment`
            // any more cheaply than this.
            let a = AttachedCatalog {
                catalog,
                attachment: AttachmentRow::pending(
                    engine.id,
                    p.catalog_id,
                    p.catalog_client_id,
                    p.credential_secret_id,
                ),
            };
            converge_and_record(st, engine, &fe, &a, false).await
        }
        CatalogChange::ReapplyOne(catalog_id) => {
            let attachment = registry
                .get_attachment(engine.id, catalog_id)
                .await
                .map_err(|e| ApplyError::Build(e.into()))?
                .ok_or(ApplyError::Build(CpError::NotFound))?;
            // `applied_at` is the only durable record that THIS engine's FE
            // once held this catalog under egret's hand. An attachment that
            // has never converged is treated exactly like a fresh attach, so
            // a catalog of the same name it did not create is still refused
            // (D3) rather than silently adopted by an ALTER.
            let already_ours = attachment.applied_at.is_some();
            let a = AttachedCatalog {
                catalog: load_catalog(st, catalog_id).await?,
                attachment,
            };
            converge_and_record(st, engine, &fe, &a, already_ours).await
        }
        CatalogChange::Detach(catalog_id) => {
            let catalog = load_catalog(st, catalog_id).await?;
            // `drop_ddl`, not a full render: a detach must work even for a
            // catalog whose CURRENT definition no longer renders (a blanked
            // JDBC driver, a deleted secret), or a row could become
            // impossible to clean up. `DROP … IF EXISTS` makes a catalog the
            // FE never had a success (D7).
            let sql = drop_ddl(&catalog.name).drop_sql();
            st.starrocks_ddl
                .execute(&fe, &sql)
                .await
                // The DROP carries no secret of its own, which is all the
                // empty map here says. The CONNECTION's secrets — the root
                // password a refused login echoes — are not this call site's
                // to remember: `scrubbed` takes the `&FeTarget` and reads them
                // itself (final whole-branch review, F2; before that fix this
                // line passed `BTreeMap::new()` alone and the comment reasoned
                // only about the statement).
                .map_err(|e| ApplyError::Rejected(e.scrubbed(&fe, &BTreeMap::new())))
        }
    }
}

/// [`converge_attachment`], then settle the attachment's `apply_status`.
/// Split from the convergence itself because the FAILURE of that write is
/// what D7 is about: the FE has a catalog egret is about to forget, and
/// nothing on a StarRocks FE prunes it.
async fn converge_and_record(
    st: &CpState,
    engine: &Engine,
    fe: &FeTarget,
    a: &AttachedCatalog,
    already_ours: bool,
) -> Result<(), ApplyError> {
    let converged = converge_attachment(st, engine, fe, a, already_ours).await?;
    if let Err(e) = st
        .engine_mgmt
        .registry()
        .set_apply_applied(engine.id, a.catalog.id)
        .await
    {
        if converged == Converged::Created {
            compensate_created_catalog(st, engine, fe, &a.catalog.name).await;
        }
        tracing::error!(engine = %engine.name, catalog = %a.catalog.name, error = %e,
            "the catalog reached the FE but the attachment could not be settled");
        return Err(ApplyError::Unrecorded(e.into()));
    }
    Ok(())
}

/// Bring EVERY attachment of a StarRocks engine onto its FE, in catalog-name
/// order, recording `applied` / `failed` per attachment (D2). Never returns an
/// error: each attachment's outcome is its own row, and one failure must not
/// hide — or abandon — the others. An attachment the loop never reaches is left
/// exactly as it was, which for a freshly created engine means `pending`: it
/// has not been attempted, and `failed` would be a claim about the FE that
/// nothing measured.
///
/// Re-entrant. This is what the bootstrap runs once the FE is ready, and a
/// Retry (`POST /engines/{id}/catalogs/{cid}/apply`) runs the same
/// [`converge_attachment`] for one row — including the engine's PRIMARY
/// catalog, which is an attachment like any other. An engine whose bootstrap
/// failed is therefore converged one Retry per catalog, where before it could
/// only be fixed by re-creating the engine.
///
/// `fe` and `consecutive_read_errors` are the caller's: the bootstrap already
/// resolved the one [`FeTarget`] its root-login probe used (D6 — one target for
/// every statement), and its read-error counter is per bootstrap, not per loop,
/// so this borrows it instead of starting a fresh count.
pub(crate) async fn converge_starrocks_catalogs(
    st: &CpState,
    engine: &Engine,
    fe: &FeTarget,
    consecutive_read_errors: &mut u32,
) {
    // Never converge an external engine's FE: its attachments are declared, and
    // egret does not run DDL on a cluster it did not provision.
    if crate::external::is_external(engine) {
        return;
    }
    let registry = st.engine_mgmt.registry();
    let mut attachments = match registry.list_engine_attachments(engine.id).await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(engine = %engine.name, error = %e,
                "starrocks convergence: could not read the engine's attachments");
            return;
        }
    };
    // Name order (D2). The store's own order is primary-first, then attachment
    // time — Trino's env-var numbering depends on it, so it is re-sorted here
    // rather than changed there.
    attachments.sort_by(|a, b| a.catalog.name.cmp(&b.catalog.name));

    for a in &attachments {
        // The checkpoint that used to guard one statement now guards each
        // catalog: a delete part-way through stops at the next one.
        if !crate::engines::engine_still_current(
            &st.engine_mgmt,
            engine.id,
            &engine.name,
            consecutive_read_errors,
        )
        .await
        {
            return;
        }
        // An attachment that has converged before is the engine's own catalog,
        // so a match becomes an ALTER rather than a no-op — the same rule
        // `ReapplyOne` applies, and for the same reason: a rotated secret is
        // invisible to the drift comparison. `applied_at`, not `apply_status`:
        // a row that failed its last apply may still have converged earlier.
        let already_ours = a.attachment.applied_at.is_some();
        let outcome = converge_attachment(st, engine, fe, a, already_ours).await;
        let write = match &outcome {
            Ok(_) => registry.set_apply_applied(engine.id, a.catalog.id).await,
            Err(e) => {
                registry
                    .set_apply_failed(engine.id, a.catalog.id, &e.message())
                    .await
            }
        };
        match write {
            Ok(()) => {}
            // D7, the same half-write this function's caller-side twin
            // (`converge_and_record`) compensates: the row VANISHED between
            // the read that listed it and the write that would settle it — a
            // concurrent detach — so the FE now holds a catalog egret has
            // forgotten, and nothing on a StarRocks FE prunes it. Only for a
            // catalog this pass CREATED: dropping one it altered, or found
            // already matching, would destroy a catalog that was there before.
            Err(egret_catalog_registry::RegistryError::NotFound(_))
                if outcome.as_ref().ok() == Some(&Converged::Created) =>
            {
                compensate_created_catalog(st, engine, fe, &a.catalog.name).await;
            }
            Err(e) => {
                tracing::warn!(engine = %engine.name, catalog = %a.catalog.name, error = %e,
                    "starrocks convergence: the outcome could not be recorded");
            }
        }
    }
}

/// What [`converge_attachment`] actually did — the difference matters only to
/// D7's compensation: dropping a catalog egret CREATED restores the state the
/// apply started from, while dropping one it merely ALTERed (or found already
/// matching) would destroy a catalog that was there before and is still in
/// use.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Converged {
    Created,
    Altered,
    AlreadyMatching,
}

/// Bring one catalog on the FE in line with the registry (D3): absent →
/// CREATE; present and matching → nothing; present and DIFFERENT → refuse,
/// naming the keys, FE untouched. `already_ours` (an attachment that has
/// converged before) turns a match into an ALTER instead of a no-op, because a
/// rotated secret is invisible to the comparison — `SHOW CREATE CATALOG`
/// deletes `password` and masks the rest — and `CREATE … IF NOT EXISTS`, which
/// the renderer deliberately does not emit, would make that rotation a silent
/// no-op.
///
/// `pub(crate)`: the bootstrap's whole-engine convergence (D2, Task 5) calls
/// this per attachment rather than going through [`CatalogChange`].
pub(crate) async fn converge_attachment(
    st: &CpState,
    engine: &Engine,
    fe: &FeTarget,
    a: &AttachedCatalog,
    already_ours: bool,
) -> Result<Converged, ApplyError> {
    let name = a.catalog.name.clone();
    let ddl = render_starrocks_attached(st, a)
        .await
        .map_err(ApplyError::Build)?;
    // The STATEMENT's secrets. The connection's — the FE root password every
    // statement below logs in with — are added by `FeError::scrubbed` from the
    // `&FeTarget` it is handed, not here: this function is the second place
    // that half was forgotten (final whole-branch review, F2), and the fix was
    // to stop asking call sites to remember it. It mattered: the leak was
    // closed only by a minted root password being 32 unbroken characters,
    // which `redact_opaque_runs` catches by SHAPE, and a short one would have
    // fallen straight into the documented residual.
    let secrets = resolve_ddl_secrets(st, &ddl)
        .await
        .map_err(ApplyError::Build)?;
    let runner = st.starrocks_ddl.as_ref();
    let present = show_catalogs(runner, fe)
        .await
        .map_err(|e| ApplyError::Rejected(e.scrubbed(fe, &secrets)))?
        .contains(&name);

    // `None` from the second read means the FE said "Unknown catalog" — the
    // catalog was dropped between the two reads. That is ABSENT, not drifted
    // (fix round 1 review, F7): `.unwrap_or_default()` used to turn it into an
    // empty property map, which `compare_to_fe` reads as EVERY key differing,
    // so a fresh attach was refused with "a catalog named X already exists on
    // this engine with different properties … detach it or drop it on the
    // engine" — about a catalog that does not exist, with a remedy the
    // operator cannot perform. The later, more specific read wins.
    let fe_props = if present {
        show_create_catalog(runner, fe, &name)
            .await
            .map_err(|e| ApplyError::Rejected(e.scrubbed(fe, &secrets)))?
    } else {
        None
    };

    let (sql, outcome) = if let Some(fe_props) = fe_props {
        match compare_to_fe(&ddl, &fe_props) {
            Drift::Different(keys) if !already_ours => {
                // D3: a catalog of this name egret did not create — the
                // `egret` catalog the Warehouse UI told operators to make by
                // hand, or one left by a pre-SP3a engine. Key NAMES only: a
                // value could be anything the FE holds.
                return Err(ApplyError::Rejected(format!(
                    "a catalog named {name} already exists on this engine with different \
                     properties ({}); detach it or drop it on the engine before attaching",
                    keys.join(", ")
                )));
            }
            // Nothing to send, and nothing to compensate later.
            Drift::Matching if !already_ours => return Ok(Converged::AlreadyMatching),
            // Everything else ALTERs, and the two cases it covers share one
            // reason: WHAT EGRET CANNOT SEE MAY STILL HAVE CHANGED.
            //
            //   * `already_ours` — this attachment converged before, so the FE
            //     holds a catalog egret created. Its secrets may have rotated
            //     since, and the comparison is blind to that by construction
            //     (`SHOW CREATE CATALOG` DELETES `password` and masks the
            //     rest), so `Matching` here means "nothing I can see changed",
            //     not "nothing changed".
            //   * `Drift::Unverifiable` — every plain key agreed, but at least
            //     one key the FE MASKS (`xx******yy`) is set, so `Matching`
            //     cannot be concluded at all. Task 3's `Drift` doc is explicit
            //     that a caller must not fold this into `Matching`.
            //
            // An `ALTER … SET` is safe to run unconditionally — it rewrites
            // properties on an existing definition and destroys no data — so
            // it is what both get.
            //
            // THE DIVERGENCE FROM D3'S LETTER, stated where it happens rather
            // than left to a report (fix round 1 review, F4). D3 names two
            // present-states, matching and different; `Unverifiable` is a
            // third, and on a catalog egret did NOT create this arm ALTERs it
            // instead of refusing. That is deliberate:
            //   - it is narrower than it sounds. The catalog D3 actually names
            //     — the hand-made `egret` warehouse catalog — never reaches
            //     here: `iceberg.catalog.credential` is `PropValue::Secret`,
            //     so it is excluded from `comparable()`, and the Egret
            //     metastore branch renders no `aws.s3.*`. Reaching
            //     `Unverifiable` needs a storage-backed catalog whose masked
            //     `aws.s3.access_key` is PRESENT while every plain key already
            //     equals what egret would render — i.e. the same warehouse,
            //     URI, endpoint and bucket.
            //   - refusing on "cannot tell" would make a legitimate re-attach
            //     of an S3-backed catalog permanently impossible, because
            //     nothing an operator can do makes a masked value comparable.
            // `an_unverifiable_catalog_is_altered_not_refused` pins it.
            _ => (
                ddl.alter_sql(&secrets).map_err(|e| render_err(&name, e))?,
                Converged::Altered,
            ),
        }
    } else {
        (
            ddl.create_sql(&secrets).map_err(|e| render_err(&name, e))?,
            Converged::Created,
        )
    };

    match runner.execute(fe, &sql).await {
        Ok(()) => Ok(outcome),
        Err(e) => {
            // D6: what the FE said, with every secret this statement carries
            // removed — and never the statement itself. `redacted()` is the
            // only form of a statement that may ever be logged, and it is told
            // WHICH statement failed: `outcome` is the one thing that knows
            // whether `sql` above was the CREATE or the ALTER, and a log
            // naming a statement that was never sent is worse than no log
            // (final whole-branch review, F4).
            let form = match outcome {
                Converged::Altered => StatementForm::Alter,
                _ => StatementForm::Create,
            };
            tracing::warn!(engine = %engine.name, catalog = %name,
                statement = %ddl.redacted(form), "starrocks catalog DDL failed");
            Err(ApplyError::Rejected(e.scrubbed(fe, &secrets)))
        }
    }
}

/// D7: the DDL ran but egret could not record it, so egret undoes its own
/// write. SP3a's "the next apply of this engine removes it again" is true of a
/// Trino ConfigMap and false here, where nothing prunes anything. Best effort
/// and logged — the operation this compensates has already failed, and a drop
/// that fails too leaves a catalog an operator must remove by hand, which is
/// what the log line says.
async fn compensate_created_catalog(st: &CpState, engine: &Engine, fe: &FeTarget, name: &str) {
    let sql = drop_ddl(name).drop_sql();
    if let Err(e) = st.starrocks_ddl.execute(fe, &sql).await {
        tracing::error!(engine = %engine.name, catalog = %name,
            // Same as the detach above: the DROP carries no secret of its
            // own, and the connection's come from the `&FeTarget`.
            error = %e.scrubbed(fe, &BTreeMap::new()),
            "created a catalog egret could not record, and the compensating DROP failed too — \
             drop it on the engine by hand");
    }
}

/// A statement-shaped handle for a catalog egret only needs to NAME.
/// `drop_sql` reads nothing but the name, so this renders exactly what a full
/// render would — while needing neither the definition nor its secrets, which
/// is what lets a detach (and D7's compensation) work for a catalog whose
/// definition no longer renders.
fn drop_ddl(name: &str) -> StarRocksCatalogDdl {
    StarRocksCatalogDdl {
        name: name.to_string(),
        properties: BTreeMap::new(),
    }
}

fn render_err(name: &str, e: RenderError) -> ApplyError {
    ApplyError::Build(CpError::BadRequest(format!("render catalog {name}: {e}")))
}

async fn load_catalog(st: &CpState, id: Uuid) -> Result<CatalogRow, ApplyError> {
    st.engine_mgmt
        .registry()
        .get_catalog(id)
        .await
        .map_err(|e| ApplyError::Build(e.into()))?
        .ok_or(ApplyError::Build(CpError::NotFound))
}

/// The plaintext of every secret a statement will interpolate. Resolved once,
/// then used for BOTH the statement and the scrubbing of whatever the FE says
/// about it — so the scrub can never be given a smaller map than the statement
/// carried.
async fn resolve_ddl_secrets(
    st: &CpState,
    ddl: &StarRocksCatalogDdl,
) -> Result<BTreeMap<Uuid, String>, CpError> {
    let mut out = BTreeMap::new();
    for id in ddl.secret_ids() {
        let value = st
            .secrets
            .resolve_plaintext(None, id)
            .await
            .map_err(|e| CpError::BadRequest(format!("resolve secret: {e}")))?;
        out.insert(id, value);
    }
    Ok(out)
}

fn catalog_def(catalog: &CatalogRow) -> Result<CatalogDef, CpError> {
    catalog
        .def()
        .map_err(|e| CpError::BadRequest(format!("catalog {}: {e}", catalog.name)))
}

/// The database-backed inputs a render needs, resolved once: the egret
/// warehouse this catalog mirrors (its name and its storage endpoint) and the
/// storage connection it names. Shared by both renderers so a Trino and a
/// StarRocks render of the same row can never disagree about what they were
/// given — only about how they spell it.
async fn resolve_render_inputs(
    st: &CpState,
    def: &CatalogDef,
) -> Result<(Option<(String, String)>, Option<StorageConn>), CpError> {
    let warehouse = match def.warehouse_id() {
        Some(id) => {
            let wh = st
                .catalog
                .get_warehouse(id)
                .await?
                .ok_or_else(|| CpError::BadRequest("unknown warehouse".into()))?;
            let sc = st
                .storage_registry
                .get_storage_conn(wh.storage_conn_id)
                .await?
                .ok_or_else(|| CpError::BadRequest("unknown storage connection".into()))?;
            Some((wh.name, sc.endpoint.unwrap_or_default()))
        }
        None => None,
    };
    let storage = match def.storage_connection_id() {
        Some(id) => Some(
            st.storage_registry
                .get_storage_conn(id)
                .await?
                .ok_or_else(|| CpError::BadRequest("unknown storage connection".into()))?,
        ),
        None => None,
    };
    Ok((warehouse, storage))
}

/// Render an attached catalog with its inputs resolved (secrets still ids).
pub(crate) async fn render_attached(
    st: &CpState,
    a: &AttachedCatalog,
    env_prefix: &str,
) -> Result<TrinoCatalogFile, CpError> {
    let def = catalog_def(&a.catalog)?;
    let (warehouse, storage) = resolve_render_inputs(st, &def).await?;
    let input = TrinoRenderInput {
        env_prefix,
        egret_url: &st.engine_egret_url,
        warehouse: warehouse.as_ref().map(|(name, ep)| WarehouseRender {
            name,
            s3_endpoint: ep,
        }),
        storage: storage.as_ref().map(|s| StorageRender {
            endpoint: s.endpoint.as_deref(),
            region: &s.region,
            path_style: s.path_style,
            access_key: &s.access_key,
            secret_key_secret_id: s.secret_ref,
        }),
        credential_secret_id: a.attachment.credential_secret_id,
    };
    render_trino(&a.catalog.name, &def, &input)
        .map_err(|e| CpError::BadRequest(format!("render catalog {}: {e}", a.catalog.name)))
}

/// [`render_attached`]'s StarRocks twin: the same inputs, plus this
/// deployment's JDBC driver table (D5), producing one `CREATE`/`ALTER`/`DROP`
/// statement's worth of properties. Secret-valued properties are still ids
/// here — [`resolve_ddl_secrets`] turns them into plaintext only inside
/// `create_sql`/`alter_sql`, so a `Debug` or a log of this value cannot leak
/// one (D6).
///
/// `pub(crate)`: the bootstrap (Task 5) renders the same way.
pub(crate) async fn render_starrocks_attached(
    st: &CpState,
    a: &AttachedCatalog,
) -> Result<StarRocksCatalogDdl, CpError> {
    let def = catalog_def(&a.catalog)?;
    let (warehouse, storage) = resolve_render_inputs(st, &def).await?;
    let input = StarRocksRenderInput {
        egret_url: &st.engine_egret_url,
        warehouse: warehouse.as_ref().map(|(name, ep)| WarehouseRender {
            name,
            s3_endpoint: ep,
        }),
        storage: storage.as_ref().map(|s| StorageRender {
            endpoint: s.endpoint.as_deref(),
            region: &s.region,
            path_style: s.path_style,
            access_key: &s.access_key,
            secret_key_secret_id: s.secret_ref,
        }),
        credential_secret_id: a.attachment.credential_secret_id,
        drivers: &st.sr_jdbc_drivers,
    };
    render_starrocks(&a.catalog.name, &def, &input)
        .map_err(|e| CpError::BadRequest(format!("render catalog {}: {e}", a.catalog.name)))
}

/// The provisioner's catalog entry: rendered properties + resolved env values.
pub(crate) async fn trino_catalog_req(
    st: &CpState,
    a: &AttachedCatalog,
    env_prefix: &str,
) -> Result<CatalogReq, CpError> {
    let file = render_attached(st, a, env_prefix).await?;
    let mut env = BTreeMap::new();
    for (var, secret_id) in &file.env {
        let value = st
            .secrets
            .resolve_plaintext(None, *secret_id)
            .await
            .map_err(|e| CpError::BadRequest(format!("resolve secret: {e}")))?;
        env.insert(var.clone(), value);
    }
    Ok(CatalogReq {
        name: file.name,
        properties: file.properties,
        env,
    })
}

/// Store a freshly minted client's credential as a named global secret.
pub(crate) async fn store_client_credential(
    st: &CpState,
    client: &CreatedClient,
) -> Result<Uuid, CpError> {
    Ok(st
        .secrets
        .create_system(
            &format!("catalog-client-{}", client.id),
            Some("Catalog Client credential an engine presents to egret's catalog"),
            &format!("{}:{}", client.client_id, client.client_secret),
        )
        .await?)
}

/// Every id a definition references must exist (global secrets, storage,
/// warehouse) — the typed parser cannot see the database.
pub(crate) async fn validate_references(st: &CpState, def: &CatalogDef) -> Result<(), CpError> {
    for id in def.secret_ids() {
        match st.secrets.get_meta(None, id).await {
            Ok(_) => {}
            Err(egret_secrets::SecretsError::NotFound) => {
                return Err(CpError::BadRequest(format!("unknown secret {id}")))
            }
            Err(e) => return Err(e.into()),
        }
    }
    if let Some(id) = def.storage_connection_id() {
        st.storage_registry
            .get_storage_conn(id)
            .await?
            .ok_or_else(|| CpError::BadRequest(format!("unknown storage connection {id}")))?;
    }
    if let Some(id) = def.warehouse_id() {
        st.catalog
            .get_warehouse(id)
            .await?
            .ok_or_else(|| CpError::BadRequest(format!("unknown warehouse {id}")))?;
    }
    Ok(())
}

/// Whether this is the engine's primary catalog. Since SP3c-1 it is a column
/// on the attachment (`is_primary`, migration 0150), not a guess from
/// `engines.warehouse_id`. SP3c-2 Task 5 made that attachment the SOURCE of
/// the engine's warehouse rather than a second opinion about it
/// (`engines::primary_warehouse`), so nothing compares the two any more.
fn is_primary(attachment: &egret_catalog_registry::AttachmentRow) -> bool {
    attachment.is_primary
}

/// Retire what egret minted for an attachment the engine no longer uses: the
/// minted Catalog Client and the named secret holding its credential. Both only
/// when `minted_client` — a client (and credential) the engine did not mint
/// belongs to someone else, and its secret may be shared. Failures are logged;
/// the operation that called this has already happened.
///
/// Two guards, both the same standing rule (never destroy a credential an
/// engine may still hold), stated once here for the detach and engine-delete
/// paths:
///   - another attachment still naming this client keeps BOTH (review F-5):
///     the (now-gone) boot backfill pointed two engines at one legacy client
///     and one shared secret, and revoking on behalf of one would break the
///     other's live Trino properties;
///   - the secret goes only once the client that holds it is actually revoked
///     — the same order, and the same reason, as the attach rollback
///     (review F-6). A live, unrevoked client whose egret copy is gone is a
///     credential nobody can clean up, so it is kept and logged loudly
///     instead. `catalog_client_id IS NULL` (a row the backfill left with no
///     client) has nothing to revoke, so its secret can go.
pub(crate) async fn retire_minted_credential(
    st: &CpState,
    engine: &Engine,
    attachment: &egret_catalog_registry::AttachmentRow,
) {
    if !attachment.minted_client {
        return;
    }
    if let Some(client_id) = attachment.catalog_client_id {
        match st
            .engine_mgmt
            .registry()
            .client_used_by_another_attachment(
                client_id,
                attachment.engine_id,
                attachment.catalog_id,
            )
            .await
        {
            Ok(true) => {
                tracing::info!(engine = %engine.name, %client_id,
                    "catalog client kept: another attachment still uses it");
                return;
            }
            Ok(false) => {}
            Err(e) => {
                // Unknown sharing is treated as shared: keeping a credential
                // costs clutter, destroying a live one costs an outage.
                tracing::warn!(engine = %engine.name, %client_id, error = %e,
                    "could not check whether another attachment uses this catalog client; \
                     keeping it and its credential secret");
                return;
            }
        }
        if let Err(e) = st.catalog.revoke_client(client_id).await {
            tracing::error!(engine = %engine.name, %client_id, error = ?e,
                "minted catalog client could not be revoked; keeping its credential secret \
                 so it is not lost while the client may still be live — clean up manually");
            return;
        }
    }
    if let Some(secret_id) = attachment.credential_secret_id {
        match st.secrets.delete(None, secret_id).await {
            Ok(()) | Err(egret_secrets::SecretsError::NotFound) => {}
            Err(e) => tracing::warn!(engine = %engine.name, %secret_id, error = %e,
                "minted catalog client credential secret could not be deleted"),
        }
    }
}

/// The refusals that depend only on the engine type, the catalog's own
/// definition and its name: a kind this engine type cannot read, and a name
/// the engine reserves. They mint nothing, write nothing and destroy nothing,
/// so they can — and must — be decided before anything else.
///
/// Both paths into an attach go through this one function so the two cannot
/// drift: `attach_to_engine` runs it as its first step, and the legacy add
/// route (`engines.rs::add_catalog`) runs it BEFORE
/// `create_catalog_replacing_orphan`, which DESTROYS a same-named unattached,
/// unassigned catalog. Deciding a refusal after that destroy loses the orphan
/// for good — the route's rollback only deletes the catalog the request itself
/// created — while the 400 says nothing happened (whole-branch review F-2).
pub(crate) fn check_engine_supports(
    engine: &Engine,
    catalog_name: &str,
    def: &CatalogDef,
) -> Result<(), CatalogOpError> {
    check_engine_supports_type(&engine.r#type, catalog_name, def).map_err(CatalogOpError::Http)
}

/// [`check_engine_supports`] for an engine that does not exist yet (a
/// registration validates its catalogs before it writes the row).
pub(crate) fn check_engine_supports_type(
    engine_type: &str,
    catalog_name: &str,
    def: &CatalogDef,
) -> Result<(), CpError> {
    let engine_type = EngineType::parse(engine_type)
        .ok_or_else(|| CpError::BadRequest(format!("unknown engine type {engine_type}")))?;
    if !def.can_read(engine_type) {
        return Err(CpError::BadRequest(format!(
            "a {} engine cannot read catalog {}",
            engine_type.as_str(),
            catalog_name
        )));
    }
    // The FE would reject these itself, but only after egret had minted a
    // Catalog Client for the attachment and written a row for it — and the
    // renderer refuses them too (`RenderError::ReservedName`), which would
    // surface as a `failed` attachment rather than a refusal. Deciding it
    // here keeps it where the other engine-type refusal already is: before
    // anything is minted or destroyed.
    if engine_type == EngineType::StarRocks
        && egret_catalog_registry::is_reserved_starrocks_name(catalog_name)
    {
        return Err(CpError::BadRequest(format!(
            "StarRocks reserves the catalog name {catalog_name}"
        )));
    }
    Ok(())
}

/// Attach a registry catalog to an engine. An egret warehouse catalog mints a
/// Catalog Client for this engine — under `role_from_client`'s role when given
/// (today's `config.clientId`), else the shared engine role. The two engine
/// types then differ in ORDER ONLY, and the reason is written where the branch
/// is: Trino applies first and writes after (SP1 Part B), StarRocks writes
/// first and converges after (SP3b D1).
pub(crate) async fn attach_to_engine(
    st: &CpState,
    engine: &Engine,
    catalog: &CatalogRow,
    role_from_client: Option<Uuid>,
) -> Result<(), CatalogOpError> {
    // **An external engine is declared, not applied** (SP4d-4 Part C): there is
    // no cluster egret runs, so no DDL, no render and no minted client. The
    // row, the recompile of the profile it binds and the refusal of any pair
    // the attachment creates share one transaction.
    if crate::external::is_external(engine) {
        if engine.status == crate::engine_status::DELETING {
            return Err(CatalogOpError::Http(apply_conflict()));
        }
        return crate::external::declare_attached_and_recompile(st, engine, catalog)
            .await
            .map_err(CatalogOpError::Http);
    }
    let pre_apply = refuse_attach_that_creates_overlap(st, engine, catalog).await?;
    let out = attach_to_engine_inner(st, engine, catalog, role_from_client).await;
    if out.is_ok() {
        // **An attachment change is a recompile trigger** (whole-branch
        // review, Important 7). `egret_perm_compile::affected_services_for_grant`
        // derives which profiles a grant reaches from the CURRENT
        // `registry.engine_catalog_attachments`, so attaching a catalog to an
        // existing engine leaves that catalog's grants uncompiled in the
        // profile that now reaches them — silent, and fail-closed only until
        // someone notices a grant that grants nothing. Engine CREATE already
        // recompiles for exactly this reason (`profile_compiler`'s own doc:
        // binding mutates no grant and no rule yet changes the answer to
        // "which profiles does this catalog reach"); an attach changes the
        // same answer and had no hook. Placed on the shared seam rather than
        // on the three routes that reach it, so a fourth caller cannot miss
        // it, and it never fails its caller — the attach has happened.
        crate::profile_compiler::recompile_bound_profile(st, engine.id, &engine.name).await;
        // **The same comparison again, against the PRE-APPLY snapshot** (fix
        // round 1, I2). The dry run above looks ahead of the write; whatever was
        // committed between it and the real recompile (the apply takes seconds
        // to minutes) is only visible here. A pair created since the snapshot is
        // refused and the attachment undone.
        if let Some(snapshot) = pre_apply.as_ref() {
            if let Err(e) = recheck_attach_against_snapshot(st, snapshot).await {
                undo_refused_attach(st, engine, catalog).await;
                return Err(CatalogOpError::Http(e));
            }
        }
    }
    out
}

/// Compare the unit as it is now with the pre-apply `snapshot`. Fail-closed: a
/// read that cannot complete refuses like an overlap does.
async fn recheck_attach_against_snapshot(
    st: &CpState,
    snapshot: &crate::profile_compiler::OverlapSnapshot,
) -> Result<(), CpError> {
    let Some(compiler) = st.perm_compiler.clone() else {
        return Ok(());
    };
    let mut tx = st
        .engine_mgmt
        .pool()
        .begin()
        .await
        .map_err(crate::http_error::db_error)?;
    let out = compiler
        .refuse_created_overlaps_in_tx(&mut tx, snapshot)
        .await;
    drop(tx);
    out.map_err(CpError::from)
}

/// Reverse an attach the post-apply comparison refused: the full detach, so the
/// cluster, the row, the minted credential and the compiled profile all go back.
/// If even that fails the attachment stays, and the error names the repair.
async fn undo_refused_attach(st: &CpState, engine: &Engine, catalog: &CatalogRow) {
    if let Err(e) = detach_from_engine(st, engine, catalog).await {
        let reason = match e {
            CatalogOpError::Http(e) => e.to_string(),
            CatalogOpError::Apply(e) => format!("{e:?}"),
        };
        tracing::error!(engine = %engine.name, catalog = %catalog.name, error = %reason,
            "an attach created an order-ambiguous pair and was refused, but undoing it failed; \
             the attachment (and the catalog on the cluster) remain: detach it by hand");
    }
}

/// **A dry run of the attachment's recompile, refusing the pairs it would
/// create** (SP4d-4 Task 3 re-review M-2, closed in Task 7).
///
/// An attachment changes which catalogs — so which tag assignments — the bound
/// profile's unit resolves, and every overlap check reads those as stored: a
/// pair the attachment completes would read as already there. The real attach
/// cannot sit in one transaction with its recompile (a Trino attach applies to
/// the cluster FIRST, a StarRocks attach converges after it), so the pair is
/// looked for before anything is applied: on a transaction that is rolled back,
/// take the before-snapshot, write a stand-in attachment row, recompile the
/// profile, and refuse what the recompile creates. Nothing it writes survives.
///
/// **Its limit is the usual one for a check made ahead of a write:** another
/// transaction may change the unit between this and the real recompile, so it
/// hands back the PRE-APPLY snapshot and `attach_to_engine` compares against it
/// again after the real recompile. A dry run that fails for any reason refuses.
async fn refuse_attach_that_creates_overlap(
    st: &CpState,
    engine: &Engine,
    catalog: &CatalogRow,
) -> Result<Option<crate::profile_compiler::OverlapSnapshot>, CatalogOpError> {
    let (Some(sid), Some(_)) = (engine.authz_service_id, st.perm_compiler.as_ref()) else {
        return Ok(None);
    };
    let mut tx = st
        .engine_mgmt
        .pool()
        .begin()
        .await
        .map_err(crate::http_error::db_error)?;
    let before = match crate::profile_compiler::snapshot_overlaps(st, &mut tx, Some(sid)).await {
        Ok(b) => b,
        Err(e) => {
            return Err(CatalogOpError::Http(refusal_for_dry_run(
                engine, catalog, e,
            )))
        }
    };
    let outcome: Result<(), egret_perm::CompileError> = async {
        if let Err(e) = st
            .engine_mgmt
            .registry()
            .insert_attachment_in_tx(
                &mut tx,
                &NewAttachment {
                    engine_id: engine.id,
                    catalog_id: catalog.id,
                    catalog_client_id: None,
                    credential_secret_id: None,
                    minted_client: false,
                    applied_now: false,
                    is_primary: false,
                    created_at: None,
                    apply_status: egret_catalog_registry::store::apply_status::PENDING,
                },
            )
            .await
        {
            // Already attached (the real attach answers that 409) or a row the
            // stand-in cannot be: nothing to look for here.
            tracing::debug!(engine = %engine.name, error = %e,
                "overlap dry run for an attachment skipped: stand-in row not written");
            return Ok(());
        }
        crate::profile_compiler::recompile_refusing_created_overlaps(
            st,
            &mut tx,
            sid,
            before.as_ref(),
        )
        .await
    }
    .await;
    // Dropped, never committed: the stand-in row and the recompile go with it.
    drop(tx);
    match outcome {
        Ok(()) => Ok(before),
        Err(e) => Err(CatalogOpError::Http(refusal_for_dry_run(
            engine, catalog, e,
        ))),
    }
}

/// A dry run that cannot complete REFUSES (it used to log and attach anyway):
/// an overlap is the caller's 409, anything else a 500 naming nothing the
/// caller can act on, with the cause logged.
fn refusal_for_dry_run(
    engine: &Engine,
    catalog: &CatalogRow,
    e: egret_perm::CompileError,
) -> CpError {
    if e.rule_overlap_body().is_none() {
        tracing::error!(engine = %engine.name, catalog = %catalog.name, error = %e,
            "overlap dry run for an attachment could not complete; the attach is refused \
             rather than made unchecked");
    }
    e.into()
}

async fn attach_to_engine_inner(
    st: &CpState,
    engine: &Engine,
    catalog: &CatalogRow,
    role_from_client: Option<Uuid>,
) -> Result<(), CatalogOpError> {
    if engine.status == crate::engine_status::DELETING {
        return Err(CatalogOpError::Http(apply_conflict()));
    }
    let registry = st.engine_mgmt.registry();
    if registry
        .get_attachment(engine.id, catalog.id)
        .await?
        .is_some()
    {
        return Err(CatalogOpError::Http(CpError::Conflict(format!(
            "catalog {} is already attached to engine {}",
            catalog.name, engine.name
        ))));
    }
    let def = catalog_def(catalog)?;
    // Idempotent: the legacy add route already decided these, before it touched
    // the registry (F-2). The registry attach route reaches them only here.
    check_engine_supports(engine, &catalog.name, &def)?;

    let mut minted: Option<(Uuid, Uuid)> = None;
    if let Some(wh_id) = def.warehouse_id() {
        let wh = st
            .catalog
            .get_warehouse(wh_id)
            .await?
            .ok_or_else(|| CpError::BadRequest("unknown warehouse".into()))?;
        if wh.lakekeeper_warehouse_id.is_none() {
            return Err(CatalogOpError::Http(CpError::BadRequest(
                "warehouse is not provisioned yet".into(),
            )));
        }
        let role_id = match role_from_client {
            Some(cid) => {
                st.catalog
                    .list_clients(wh.id)
                    .await?
                    .into_iter()
                    .find(|c| c.id == cid)
                    .ok_or_else(|| {
                        CpError::BadRequest(
                            "config.clientId does not reference a client of this warehouse".into(),
                        )
                    })?
                    .catalog_role_id
            }
            None => ensure_engine_role(st, wh.id).await?,
        };
        let client = st
            .catalog
            .create_client(NewClient {
                name: format!("engine-{}-{}", engine.name, catalog.name),
                consumer_type: "engine".into(),
                workspace_id: wh.workspace_id,
                warehouse_id: wh.id,
                catalog_role_id: role_id,
            })
            .await?;
        match store_client_credential(st, &client).await {
            Ok(secret_id) => minted = Some((client.id, secret_id)),
            Err(e) => {
                if let Err(err) = st.catalog.revoke_client(client.id).await {
                    tracing::warn!(engine = %engine.name, client_id = %client.id, error = ?err,
                        "attach rollback: revoke failed");
                }
                return Err(e.into());
            }
        }
    }
    let pending = PendingAttachment {
        catalog_id: catalog.id,
        catalog_client_id: minted.map(|m| m.0),
        credential_secret_id: minted.map(|m| m.1),
    };

    // StarRocks records first and converges after (D1); Trino applies first
    // and writes after (SP1 Part B). The reason is not taste: a StarRocks FE
    // is unreachable for five to ten minutes after `create_engine` — exactly
    // when the create-engine wizard attaches catalogs — and a row egret has
    // forgotten is worse than one it retries. A Trino apply has no such
    // window, and writing its row first would claim an attachment the cluster
    // may still refuse.
    if engine.is_starrocks() {
        if let Err(e) = registry
            .insert_attachment(&NewAttachment {
                engine_id: engine.id,
                catalog_id: catalog.id,
                catalog_client_id: pending.catalog_client_id,
                credential_secret_id: pending.credential_secret_id,
                minted_client: minted.is_some(),
                applied_now: false,
                // An attach is always an ADDITIONAL catalog: the engine's
                // primary is written by create (SP3c D2), and the partial
                // unique index would refuse a second one anyway.
                is_primary: false,
                created_at: None,
                // Recorded, not yet converged — the state migration 0149 exists
                // for. It becomes `applied` or `failed` a few lines below;
                // it survives as `pending` only if this process dies between.
                apply_status: egret_catalog_registry::store::apply_status::PENDING,
            })
            .await
        {
            rollback_minted(st, engine, minted).await;
            return Err(e.into());
        }
        match apply_catalog_change(st, engine, CatalogChange::Attach(pending)).await {
            // `converge_and_record` already settled the row to `applied`.
            Ok(()) => Ok(()),
            // D7: the FE took the statement, egret could not record it, so
            // this attach did not happen and the minted credential goes back.
            //
            // WHAT HAS ACTUALLY RUN ON THE FE BY NOW depends on how
            // `converge_attachment` converged, and only ONE of the three ways
            // is compensated (`converge_and_record` drops only for
            // `Converged::Created`) — the comment here used to claim the DROP
            // unconditionally (final whole-branch review, F3):
            //
            //   * `Created` — the catalog egret made is dropped again, so
            //     nothing on the engine references the credential.
            //   * `Altered` — a catalog egret did NOT create (the
            //     `Drift::Unverifiable` arm, `an_unverifiable_catalog_is_
            //     altered_not_refused`) has been rewritten with the minted
            //     credential, and no DROP runs. `rollback_minted` below
            //     REVOKES that client before deleting the secret, so nothing
            //     live is orphaned and the standing secret rule holds — but
            //     the FE is left holding a pre-existing catalog whose
            //     credential egret has just revoked, and no row names it any
            //     more. Dropping it is not an option: egret did not create it.
            //     An operator's repair is to re-attach, which converges it
            //     with a fresh credential.
            //   * `AlreadyMatching` — nothing was sent at all, so the FE is
            //     exactly as it was and the minted credential was never used.
            //
            // Both uncompensated cases need the DATABASE write to have failed
            // on a fresh attach, which is why this is accepted untested (the
            // ledger's F5) rather than defended.
            //
            // The row is USUALLY already gone (that race is what `Unrecorded`
            // is normally about), but not always: a transient database failure
            // on the `set_apply_applied` write leaves the `pending` row in
            // place (fix round 1 review, F5). Deleting it first is what makes
            // giving back the credential safe — otherwise the surviving row
            // names a `credential_secret_id` that `rollback_minted` is about
            // to delete, every Retry then fails to resolve it, and only a
            // detach plus a fresh attach can repair it. Best effort, and it
            // runs BEFORE the rollback, never after.
            Err(ApplyError::Unrecorded(e)) => {
                if let Err(err) = registry.delete_attachment(engine.id, catalog.id).await {
                    tracing::error!(engine = %engine.name, catalog = %catalog.name, error = %err,
                        "could not remove the attachment of an attach that did not happen; \
                         keeping its credential secret so the row stays repairable");
                    return Err(CatalogOpError::Http(e));
                }
                rollback_minted(st, engine, minted).await;
                Err(CatalogOpError::Http(e))
            }
            // D1: everything else — an unreachable FE, a refused statement, a
            // definition that will not render — is the attachment's state,
            // not the request's. The row is the point; Retry converges it.
            // The minted credential is KEPT: the failed apply may have
            // reached the FE, and the retry needs it either way.
            Err(e) => {
                registry
                    .set_apply_failed(engine.id, catalog.id, &e.message())
                    .await?;
                Ok(())
            }
        }
    } else {
        if let Err(e) = apply_catalog_change(st, engine, CatalogChange::Attach(pending)).await {
            rollback_minted(st, engine, minted).await;
            return Err(CatalogOpError::Apply(e));
        }
        registry
            .insert_attachment(&NewAttachment {
                engine_id: engine.id,
                catalog_id: catalog.id,
                catalog_client_id: pending.catalog_client_id,
                credential_secret_id: pending.credential_secret_id,
                minted_client: minted.is_some(),
                applied_now: true,
                // See the StarRocks branch above: an attach never leads.
                is_primary: false,
                created_at: None,
                apply_status: egret_catalog_registry::store::apply_status::APPLIED,
            })
            .await
            .map_err(|e| {
                tracing::error!(engine = %engine.name, catalog = %catalog.name, error = %e,
                    "catalog applied to the engine but the attachment could not be written; \
                     the next apply of this engine removes it again");
                CatalogOpError::from(e)
            })?;
        Ok(())
    }
}

/// Give back what an attach minted, after that attach failed.
///
/// Only delete egret's copy of the credential once the client that holds it is
/// actually revoked. If the failed apply got as far as the engine, a live,
/// unrevoked client whose egret copy is gone is a credential nobody can clean
/// up; keep it and log loudly instead so an operator can find and revoke it.
/// [`retire_minted_credential`] (detach, engine delete) applies the same order
/// for the same reason — the rule holds on every path, not just this one.
async fn rollback_minted(st: &CpState, engine: &Engine, minted: Option<(Uuid, Uuid)>) {
    let Some((client_id, secret_id)) = minted else {
        return;
    };
    match st.catalog.revoke_client(client_id).await {
        Ok(()) => {
            if let Err(err) = st.secrets.delete(None, secret_id).await {
                tracing::warn!(engine = %engine.name, %secret_id, error = %err,
                    "attach rollback: secret delete failed");
            }
        }
        Err(err) => {
            tracing::error!(engine = %engine.name, %client_id, %secret_id, error = ?err,
                "attach rollback: revoke failed; leaving the credential secret in place \
                 so it is not lost while the client may still be live — clean up manually");
        }
    }
}

/// Detach: apply the engine without the catalog, then delete the attachment and
/// retire what egret minted for it. The engine's primary catalog cannot go.
pub(crate) async fn detach_from_engine(
    st: &CpState,
    engine: &Engine,
    catalog: &CatalogRow,
) -> Result<(), CatalogOpError> {
    // An external engine's attachment is a declared row: remove it and recompile,
    // with no render, no DDL and no credential to retire.
    if crate::external::is_external(engine) {
        return crate::external::detach_declared_and_recompile(st, engine, catalog)
            .await
            .map_err(CatalogOpError::Http);
    }
    let registry = st.engine_mgmt.registry();
    let attachment = registry
        .get_attachment(engine.id, catalog.id)
        .await?
        .ok_or(CatalogOpError::Http(CpError::NotFound))?;
    if is_primary(&attachment) {
        return Err(CatalogOpError::Http(CpError::BadRequest(
            "cannot delete the primary catalog".into(),
        )));
    }
    if engine.status == crate::engine_status::DELETING {
        return Err(CatalogOpError::Http(apply_conflict()));
    }
    apply_catalog_change(st, engine, CatalogChange::Detach(catalog.id))
        .await
        .map_err(CatalogOpError::Apply)?;
    registry.delete_attachment(engine.id, catalog.id).await?;
    // The cluster no longer uses these; failures are logged, the detach happened.
    retire_minted_credential(st, engine, &attachment).await;
    // The mirror image of the attach hook above, and the one with teeth
    // (whole-branch review, Important 7): with the catalog detached, this
    // catalog's grants are no longer in ANY affected set, so deleting a grant
    // while it is detached recompiles nothing and the compiled `perm-grant-*`
    // policy stays in the profile — a revocation that does not revoke, until
    // some unrelated recompile happens. Recompiling here is what makes the
    // detached state the compiled state.
    crate::profile_compiler::recompile_bound_profile(st, engine.id, &engine.name).await;
    Ok(())
}

/// Re-apply one engine after a catalog changed; flags the attachment on a
/// rejected apply. A `deleting` engine (concurrent delete) is a 409 that
/// writes nothing — a refused apply must leave the DB unchanged, and the
/// attachment's `apply_status` says nothing true about a cluster the apply
/// never reached.
///
/// `apply_status` answers exactly one question: *did this catalog reach that
/// engine?* Only a failure the ENGINE produced may be written onto the row;
/// everything that fails on either side of the engine leaves the row saying
/// what it said before, which is still true (see the carve-out below).
pub(crate) async fn retry_apply(
    st: &CpState,
    engine: &Engine,
    catalog_id: Uuid,
) -> Result<(), CatalogOpError> {
    crate::external::refuse_if_external(engine, "apply a catalog to it")
        .map_err(CatalogOpError::Http)?;
    if engine.status == crate::engine_status::DELETING {
        return Err(CatalogOpError::Apply(ApplyError::Conflict));
    }
    if let Err(e) = apply_catalog_change(st, engine, CatalogChange::ReapplyOne(catalog_id)).await {
        // TWO failures must not be flagged on the row, and for one reason: the
        // row's `apply_status` is a statement about the ENGINE, and neither of
        // them is.
        //
        // `Unrecorded` (fix round 1 review, F5) is the far side: the engine
        // took the change and the `apply_status` WRITE is what failed. Writing
        // a second status to the same row through the same database would
        // either hit the same failure or — worse — succeed, recording `failed`
        // for an engine that is actually converged. It is the caller's error,
        // not the attachment's state.
        //
        // `Build` is the near side: the change never reached the engine at
        // all — no warehouse, no secret, no Kubernetes, or a definition that
        // would not render. And on Trino it is not even necessarily ABOUT this
        // catalog: `CatalogChange::ReapplyOne` maps to `BuildOpts::default()`
        // because a Trino apply is the whole ConfigMap, so
        // `build_provision_req_with` renders EVERY attachment of the engine
        // with a `?` per row and one unparseable SIBLING ends the build. Before
        // this carve-out that wrote `failed` — with the sibling's parse error —
        // onto the healthy row the operator had asked about, and `propagate`
        // reaches this function too, so saving a healthy catalog's
        // Configuration flagged it from the repair page itself. SP3c-1 renders
        // `apply_status` on five surfaces; the datum has to be true
        // (whole-branch review, F2).
        //
        // What is NOT lost: the refusal still reaches the caller as a 400/502
        // with the same message, and the reason a row cannot render is reported
        // independently as `configError` on `GET /catalogs` (the registry list;
        // SP3c-2 task 3 moved this off the legacy `/engines/{id}/catalogs`).
        if !matches!(e, ApplyError::Unrecorded(_) | ApplyError::Build(_)) {
            st.engine_mgmt
                .registry()
                .set_apply_failed(engine.id, catalog_id, &e.message())
                .await?;
        }
        return Err(CatalogOpError::Apply(e));
    }
    Ok(())
}

/// Spec §4.5.3: apply a catalog's stored configuration to every engine that
/// attaches it, in engine-name order. A failure flags that attachment and moves
/// on — the other engines already hold the new configuration, so nothing reverts.
pub(crate) async fn propagate(
    st: &CpState,
    catalog: &CatalogRow,
) -> Result<Vec<ApplyOutcome>, CpError> {
    let mut out = Vec::new();
    for row in st
        .engine_mgmt
        .registry()
        .list_catalog_attachments(catalog.id)
        .await?
    {
        let Some(engine) = st.engine_mgmt.get_engine(row.engine_id).await? else {
            continue;
        };
        // A declared attachment is not applied by egret: say so in the list
        // rather than dropping the engine from it or calling it a failure.
        if crate::external::is_external(&engine) {
            out.push(ApplyOutcome {
                engine_id: engine.id,
                engine_name: engine.name.clone(),
                applied: false,
                skipped: true,
                error: Some(
                    "external engine: egret does not apply catalogs to it; the attachment is \
                     declared only"
                        .into(),
                ),
            });
            continue;
        }
        let outcome = match retry_apply(st, &engine, catalog.id).await {
            Ok(()) => ApplyOutcome {
                engine_id: engine.id,
                engine_name: engine.name.clone(),
                applied: true,
                skipped: false,
                error: None,
            },
            Err(CatalogOpError::Apply(e)) => ApplyOutcome {
                engine_id: engine.id,
                engine_name: engine.name.clone(),
                applied: false,
                skipped: false,
                error: Some(e.message()),
            },
            Err(CatalogOpError::Http(e)) => return Err(e),
        };
        out.push(outcome);
    }
    Ok(out)
}
