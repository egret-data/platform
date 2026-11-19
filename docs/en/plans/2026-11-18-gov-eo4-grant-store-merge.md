# EO4 — One grant store for egret objects (capstone) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fold the Cedar grant table `access_grants` into the `perm.*` one grant store behind a global, reversible `legacy→unified` grant-source switch that defaults to legacy and rolls back instantly, so the PDP's per-request grant links read from the unified store with provably identical decisions.

**Architecture:** A singleton DB row (`perm.cedar_grant_source`) selects which table `AccessGrantStore::grants_for` reads; the PDP re-reads it every reload. A sibling table `perm.cedar_grants` mirrors `access_grants`'s decision-relevant columns and is kept **live-consistent** by a trigger on `access_grants` (so the unified read never goes stale); `fold` backfills the pre-existing rows once. The `egret-cedar` EP's grant source flips to `unified` only when a fresh equivalence report (replica-fidelity: `access_grants` row-set == `perm.cedar_grants` row-set) is empty. Thin platform-admin HTTP handlers read the state, run the report, fold, flip (gated), and undo, publishing `RolesChanged`. Default `legacy` ⇒ zero behaviour change. Deleting the legacy read paths is a deferred post-soak finalize (EO4-D5), out of scope here.

**Tech Stack:** Rust, `cedar-policy` 4.12, `sqlx` 0.9 (Postgres, trigger in SQL), `axum`, `utoipa`, `tokio`, `arc_swap`, `chrono`.

## Global Constraints

- Chat Vietnamese; all code, comments, commit messages, and docs in English.
- Conventional Commits. `Closes #N` only in the PR body, never in commit messages.
- Co-author every commit: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. PR body ends with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.
- Never commit secrets. Stage by explicit path (never `git add -A`). `target-eo4/` is NOT gitignored — never stage it.
- This worktree: prefix every cargo invocation with `CARGO_TARGET_DIR=target-eo4`.
- Migrations `0239_cedar_grant_source.sql` and `0240_cedar_grants_mirror.sql` — migration numbers are a shared namespace; **renumber above `main` and any open PR at merge time** (this branch stacks on EO3 #878, which uses `0238`; if EO3 renumbers, bump these too).
- **Timestamps use `chrono::DateTime<chrono::Utc>`**, NOT `time::OffsetDateTime`: the workspace enables only the `chrono` feature on `sqlx`/`utoipa` (EO3 Task 1 hit this — the spec sketch said `time` and the implementer had to switch). Mirror an existing `chrono`-using column read in `egret-auth`.
- Fail closed / fail safe (EO-D7): an unreadable switch row ⇒ read `legacy` (`access_grants`, the proven source); a `perm.cedar_grants` read error while `unified` ⇒ fall back to `access_grants` for that call and alarm; never block a reload; never break authz.
- **Zero behaviour change on merge:** default state is `legacy`; `AccessGrantStore::grants_for` keeps its exact signature and, under `legacy`, runs the identical query against `access_grants`, so every current PDP test and call site behaves unchanged.
- **No vocab/decision/role change (EO4-D6):** EO4 moves the grant *source* only. It does not touch `roles`/`role_permissions`, the typed Cedar-EP privileges, or the classification `forbid` path. `pdp::entities::link_grants` is unchanged.
- DB-gated tests use the crate's `egret_test_db` helper and must fail (not silently skip) without a DB unless the crate's established skip env is set. Global-row tests serialise on a mutex (as EO3's `pdp_source_switch.rs` does with `SWITCH_LOCK`) to avoid cross-test races on the singleton.
- EO4 adds operator HTTP endpoints ⇒ a **separate `egret-data/docs` PR** (Platform runbook: fold/switch/undo lifecycle, replica-fidelity report, rollback) is required — tracked outside this code plan.

## Interfaces (verified against the EO3 tree this branch stacks on)

- `crates/egret-auth/src/grants.rs`: `pub struct Grant { role_id: Uuid, scope_type: String, scope_id: Option<Uuid>, object_type: Option<String>, effect: String }`; `pub struct AccessGrantStore { db: Db }`; `pub async fn grants_for(&self, user_id: UserId, ws: Option<WorkspaceId>, object_scope_ids: &[Uuid]) -> Result<Vec<Grant>>` — the single PDP read site; its SQL selects `role_id, scope_type, scope_id, object_type, effect FROM access_grants` filtered by principal (direct + `user_groups`), scope, and `expires_at`.
- `crates/egret-auth/src/pdp/entities.rs::link_grants(ps, subject, grants: &[Grant], role_names)` consumes `grants_for`'s output unchanged; `access_grants` columns are `(id, principal_type ∈ {user,group,service}, principal_id, role_id → roles(id), scope_type ∈ {platform,workspace,object}, scope_id, object_type, granted_by, created_at, effect ∈ {allow,deny}, expires_at)`.
- EO3 switch-store precedent (same crate, this branch): `crates/egret-auth/src/pdp/source_switch.rs` — `PolicySource` enum with `as_db`/`from_db`, `read_source`(infallible, fail-safe), `read_state`, `write_source`; and `crates/egret-auth/src/pdp_source_routes.rs` — `PdpSourceState { db, sessions, bus }`, `FromRef<_> for SessionService`, `pdp_source_router()`, `#[utoipa::path]` handlers, `PdpSourceApi` OpenAPI registration, mounted in `egret-server`/`egret-bin`. **Mirror these shapes.**
- `egret_core::Error { Conflict(String) ⇒ 409, Internal(anyhow::Error) ⇒ 500, … }`; `pub type Result<T>`. `principal::PlatformAuth(pub Principal)` gates platform-admin (403); `p.email` is the audit actor. `egret_events::EventBus::publish(RolesChanged)`; the PDP's `spawn_reload` reloads on `RolesChanged`.
- `perm.migrated_from` table exists (SP-era provenance); use it for the fold marker. `registry`/`perm` schemas exist; `perm.pdp_policy_source` (EO3, `0238`) is the latest `perm.*` switch.

---

### Task 1: Grant-source switch store — migration + `GrantSource` + accessors

**Files:**
- Create: `migrations/0239_cedar_grant_source.sql`
- Create: `crates/egret-auth/src/pdp/grant_source_switch.rs`
- Modify: `crates/egret-auth/src/pdp/mod.rs` (add `pub mod grant_source_switch;` to the `pub mod` block)
- Test: `crates/egret-auth/tests/cedar_grant_source.rs` (new; store tests)

**Interfaces:**
- Produces: `GrantSource { Legacy, Unified }` (+ `as_db()->&'static str`, `from_db(&str)->Option<Self>`); `GrantSourceState { source: GrantSource, switched_at: Option<DateTime<Utc>>, switched_by: Option<String> }`; `async fn read_source(db) -> GrantSource` (infallible, fail-safe Legacy); `async fn read_state(db) -> Result<GrantSourceState>`; `async fn write_source(db, GrantSource, actor: &str) -> Result<()>`.

- [ ] **Step 1: Write the migration**

Create `migrations/0239_cedar_grant_source.sql`:

```sql
-- 0239_cedar_grant_source.sql
--
-- EO4 (one grant store for egret objects): the GLOBAL, single-row switch that
-- selects which table the control-plane PDP reads per-request grant links from.
-- `AccessGrantStore::grants_for` reads it on every reload path:
--
--   * `legacy` (the DEFAULT, and the state with no row): read `access_grants`
--     exactly as before EO4 — the proven path.
--   * `unified`: read the one grant store's mirror `perm.cedar_grants` (kept
--     live-consistent by the trigger in 0240). Decisions are identical by
--     construction (same rows → same links), proved by the equivalence gate
--     before the flip.
--
-- Shipping this with no row keeps every PDP on `legacy`, so it enables nothing
-- on its own. Singleton via the `id = true` primary key; audit columns record
-- the last flip.
CREATE TABLE perm.cedar_grant_source (
    id           boolean PRIMARY KEY DEFAULT true CHECK (id),
    source       text NOT NULL DEFAULT 'legacy'
                 CHECK (source IN ('legacy', 'unified')),
    switched_at  timestamptz,
    switched_by  text
);
```

(No seeded row: absent reads as `legacy`.)

- [ ] **Step 2: Write the store module**

Create `crates/egret-auth/src/pdp/grant_source_switch.rs`, mirroring `source_switch.rs` (EO3) exactly but with two states and `chrono`:

```rust
//! EO4: the global, single-row switch selecting which table the PDP reads
//! per-request grants from (`perm.cedar_grant_source`). Read on every reload;
//! written by the operator endpoints. Default (no row) is `Legacy` —
//! `access_grants`, the proven, pre-EO4 path.
use chrono::{DateTime, Utc};
use egret_metastore::Db;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum GrantSource {
    /// Read `access_grants` (pre-EO4 behaviour). The default.
    Legacy,
    /// Read the unified store mirror `perm.cedar_grants`.
    Unified,
}

impl GrantSource {
    pub fn as_db(self) -> &'static str {
        match self { GrantSource::Legacy => "legacy", GrantSource::Unified => "unified" }
    }
    pub fn from_db(s: &str) -> Option<Self> {
        match s { "legacy" => Some(Self::Legacy), "unified" => Some(Self::Unified), _ => None }
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct GrantSourceState {
    pub source: GrantSource,
    pub switched_at: Option<DateTime<Utc>>,
    pub switched_by: Option<String>,
}

/// Infallible, fail-safe to `Legacy` on any error or absent row (hot path).
pub async fn read_source(db: &Db) -> GrantSource {
    match sqlx::query_scalar::<_, String>(
        "SELECT source FROM perm.cedar_grant_source WHERE id = true",
    ).fetch_optional(&db.pool).await {
        Ok(Some(s)) => GrantSource::from_db(&s).unwrap_or_else(|| {
            tracing::error!(source = %s, "unknown cedar_grant_source value; reading legacy");
            GrantSource::Legacy
        }),
        Ok(None) => GrantSource::Legacy,
        Err(e) => { tracing::warn!(error = %e, "cedar_grant_source unreadable; reading legacy"); GrantSource::Legacy }
    }
}

pub async fn read_state(db: &Db) -> egret_core::Result<GrantSourceState> {
    let row: Option<(String, Option<DateTime<Utc>>, Option<String>)> = sqlx::query_as(
        "SELECT source, switched_at, switched_by FROM perm.cedar_grant_source WHERE id = true",
    ).fetch_optional(&db.pool).await
     .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
    Ok(match row {
        Some((s, at, by)) => GrantSourceState {
            source: GrantSource::from_db(&s).unwrap_or(GrantSource::Legacy), switched_at: at, switched_by: by },
        None => GrantSourceState { source: GrantSource::Legacy, switched_at: None, switched_by: None },
    })
}

pub async fn write_source(db: &Db, source: GrantSource, actor: &str) -> egret_core::Result<()> {
    sqlx::query(
        "INSERT INTO perm.cedar_grant_source (id, source, switched_at, switched_by) \
         VALUES (true, $1, now(), $2) \
         ON CONFLICT (id) DO UPDATE SET source = EXCLUDED.source, \
           switched_at = EXCLUDED.switched_at, switched_by = EXCLUDED.switched_by",
    ).bind(source.as_db()).bind(actor).execute(&db.pool).await
     .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
    Ok(())
}
```

Add `pub mod grant_source_switch;` to `crates/egret-auth/src/pdp/mod.rs`. `chrono` is already a workspace dep used in `egret-auth` (`context.rs` uses `chrono::DateTime<chrono::Utc>`); no Cargo.toml change expected — verify with `grep -n chrono crates/egret-auth/Cargo.toml`.

- [ ] **Step 3: Build**

Run: `CARGO_TARGET_DIR=target-eo4 cargo build -p egret-auth`
Expected: compiles (additive; nothing calls it yet).

- [ ] **Step 4: Write the store tests**

Create `crates/egret-auth/tests/cedar_grant_source.rs`, mirroring `pdp_source_switch.rs`'s DB helper + `SWITCH_LOCK` mutex idiom:

```rust
use egret_auth::pdp::grant_source_switch::{read_source, read_state, write_source, GrantSource};
// ... db() helper + static SWITCH_LOCK copied from pdp_source_switch.rs ...

#[tokio::test]
async fn default_is_legacy_when_absent() {
    let _g = SWITCH_LOCK.lock().await;
    let db = db().await;
    write_source(&db, GrantSource::Legacy, "reset").await.unwrap(); // normalise shared row
    assert_eq!(read_source(&db).await, GrantSource::Legacy);
}

#[tokio::test]
async fn write_then_read_roundtrips_with_audit() {
    let _g = SWITCH_LOCK.lock().await;
    let db = db().await;
    write_source(&db, GrantSource::Unified, "op@egret.local").await.unwrap();
    assert_eq!(read_source(&db).await, GrantSource::Unified);
    let st = read_state(&db).await.unwrap();
    assert_eq!(st.source, GrantSource::Unified);
    assert_eq!(st.switched_by.as_deref(), Some("op@egret.local"));
    assert!(st.switched_at.is_some());
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM perm.cedar_grant_source")
        .fetch_one(&db.pool).await.unwrap();
    assert_eq!(n, 1, "singleton row");
    write_source(&db, GrantSource::Legacy, "reset").await.unwrap(); // restore
}
```

- [ ] **Step 5: Run the tests**

Run: `CARGO_TARGET_DIR=target-eo4 cargo test -p egret-auth --test cedar_grant_source`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add migrations/0239_cedar_grant_source.sql crates/egret-auth/src/pdp/grant_source_switch.rs crates/egret-auth/src/pdp/mod.rs crates/egret-auth/tests/cedar_grant_source.rs
git commit -m "feat(auth): EO4 cedar grant-source switch store (migration + accessors)"
```

---

### Task 2: Mirror table `perm.cedar_grants` + sync trigger + fold/undo

**Files:**
- Create: `migrations/0240_cedar_grants_mirror.sql`
- Modify: `crates/egret-auth/src/grants.rs` (add `pub async fn fold(db) -> Result<u64>` and `pub async fn clear_mirror(db) -> Result<()>`)
- Test: `crates/egret-auth/tests/cedar_grant_source.rs` (fold copies all rows; trigger mirrors a later write; clear empties the mirror)

**Interfaces:**
- Produces: `perm.cedar_grants` holding the decision-relevant `access_grants` columns, trigger-synced; `AccessGrantStore::fold(&self) -> Result<u64>` (backfill count); `AccessGrantStore::clear_mirror(&self) -> Result<()>`.
- Provenance: the mirror is a maintained replica, not a one-way migration, so per-row migration provenance (`perm.migrated_from`, which is Ranger-policy-specific — `service_id`/`policy_id` FKs — and does NOT fit) is **not** used. Each `perm.cedar_grants.id` equals its `access_grants` origin (row lineage), and the fold/flip event is audited by the switch row's `switched_at`/`switched_by` (Task 1) and the endpoint's actor (Task 4). This refines EO4-D5's "provenance in `perm.migrated_from`": the replica needs no separate provenance table.

- [ ] **Step 1: Write the migration (table + trigger)**

Create `migrations/0240_cedar_grants_mirror.sql`:

```sql
-- 0240_cedar_grants_mirror.sql
--
-- EO4: the unified-store mirror of `access_grants`'s decision-relevant columns.
-- A trigger on `access_grants` keeps it live-consistent for every future write,
-- so the `unified` read path (perm.cedar_grant_source = 'unified') never serves a
-- stale grant. `AccessGrantStore::fold` backfills the rows that existed before
-- this migration. The mirror carries the SAME shape `grants_for` reads, so the
-- unified query is the legacy query against this table.
--
-- Reversible: the switch (0239) merely chooses which table `grants_for` reads;
-- `access_grants` stays the write target and is untouched, so undo is instant.
-- Deleting `access_grants` and redirecting writes here is the deferred finalize
-- (EO4-D5), NOT this migration.
CREATE TABLE perm.cedar_grants (
    id             uuid PRIMARY KEY,                    -- mirrors access_grants.id
    principal_type text NOT NULL CHECK (principal_type IN ('user','group','service')),
    principal_id   uuid NOT NULL,
    role_id        uuid NOT NULL,
    scope_type     text NOT NULL CHECK (scope_type IN ('platform','workspace','object')),
    scope_id       uuid,
    object_type    text,
    effect         text NOT NULL CHECK (effect IN ('allow','deny')),
    expires_at     timestamptz
);
CREATE INDEX cedar_grants_principal_idx ON perm.cedar_grants (principal_type, principal_id);
CREATE INDEX cedar_grants_scope_idx ON perm.cedar_grants (scope_type, scope_id);

-- Keep the mirror in lock-step with access_grants writes (INSERT/UPDATE/DELETE).
CREATE OR REPLACE FUNCTION perm.sync_cedar_grants() RETURNS trigger AS $$
BEGIN
    IF (TG_OP = 'DELETE') THEN
        DELETE FROM perm.cedar_grants WHERE id = OLD.id;
        RETURN OLD;
    END IF;
    INSERT INTO perm.cedar_grants
        (id, principal_type, principal_id, role_id, scope_type, scope_id, object_type, effect, expires_at)
    VALUES
        (NEW.id, NEW.principal_type, NEW.principal_id, NEW.role_id, NEW.scope_type,
         NEW.scope_id, NEW.object_type, NEW.effect, NEW.expires_at)
    ON CONFLICT (id) DO UPDATE SET
        principal_type = EXCLUDED.principal_type, principal_id = EXCLUDED.principal_id,
        role_id = EXCLUDED.role_id, scope_type = EXCLUDED.scope_type, scope_id = EXCLUDED.scope_id,
        object_type = EXCLUDED.object_type, effect = EXCLUDED.effect, expires_at = EXCLUDED.expires_at;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER access_grants_mirror
    AFTER INSERT OR UPDATE OR DELETE ON access_grants
    FOR EACH ROW EXECUTE FUNCTION perm.sync_cedar_grants();
```

- [ ] **Step 2: Write the failing fold/trigger test**

Add to `crates/egret-auth/tests/cedar_grant_source.rs`:

```rust
#[tokio::test]
async fn fold_backfills_and_trigger_mirrors_new_writes() {
    let _g = SWITCH_LOCK.lock().await;
    let db = db().await;
    let store = egret_auth::grants::AccessGrantStore::new(db.clone());
    // Backfill existing rows.
    store.fold().await.unwrap();
    // Replica fidelity right after fold: the two tables match row-for-row.
    let diff: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM ( \
           (SELECT id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at FROM access_grants \
            EXCEPT SELECT id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at FROM perm.cedar_grants) \
           UNION ALL \
           (SELECT id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at FROM perm.cedar_grants \
            EXCEPT SELECT id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at FROM access_grants) \
         ) d").fetch_one(&db.pool).await.unwrap();
    assert_eq!(diff, 0, "fold makes the mirror faithful");
    // A later access_grants write must appear in the mirror via the trigger.
    // (Insert a throwaway grant on a seeded role/principal, assert it mirrors, then clean up.)
    // ... insert into access_grants ...; assert it exists in perm.cedar_grants; delete it; assert gone.
}
```

- [ ] **Step 3: Implement `fold` + `clear_mirror`**

In `crates/egret-auth/src/grants.rs`, add to `impl AccessGrantStore`:

```rust
/// EO4: backfill `perm.cedar_grants` from the rows that predate the mirror
/// trigger (0240). Idempotent: upsert on id. The trigger keeps future writes in
/// sync, so this is a one-time catch-up (re-runnable). Returns the row count.
pub async fn fold(&self) -> Result<u64> {
    let r = sqlx::query(
        "INSERT INTO perm.cedar_grants \
           (id, principal_type, principal_id, role_id, scope_type, scope_id, object_type, effect, expires_at) \
         SELECT id, principal_type, principal_id, role_id, scope_type, scope_id, object_type, effect, expires_at \
         FROM access_grants \
         ON CONFLICT (id) DO UPDATE SET \
           principal_type = EXCLUDED.principal_type, principal_id = EXCLUDED.principal_id, \
           role_id = EXCLUDED.role_id, scope_type = EXCLUDED.scope_type, scope_id = EXCLUDED.scope_id, \
           object_type = EXCLUDED.object_type, effect = EXCLUDED.effect, expires_at = EXCLUDED.expires_at",
    ).execute(&self.db.pool).await.map_err(db_err)?;
    // Provenance (EO4-D5, refined): the mirror is a maintained replica, so each
    // row's origin is carried as its own id (= access_grants.id); the fold/flip
    // event is audited by the switch row (switched_at/switched_by) and the
    // endpoint actor. No separate provenance table (perm.migrated_from is
    // Ranger-policy-specific and does not fit).
    tracing::info!(rows = r.rows_affected(), "EO4 fold: backfilled perm.cedar_grants from access_grants");
    Ok(r.rows_affected())
}

/// EO4: empty the mirror (used by undo after the switch is back to legacy, and by
/// tests). The trigger refills it on the next access_grants write; `fold` refills
/// it fully. Never called while `unified`.
pub async fn clear_mirror(&self) -> Result<()> {
    sqlx::query("DELETE FROM perm.cedar_grants").execute(&self.db.pool).await.map_err(db_err)?;
    Ok(())
}
```

(Provenance uses no extra table — see the Interfaces note above. `perm.migrated_from` was checked and is Ranger-policy-specific: `service_id`/`policy_id` NOT NULL FKs to `policy.authz_*`, unsuitable for a grant-fold marker.)

- [ ] **Step 4: Run the tests**

Run: `CARGO_TARGET_DIR=target-eo4 cargo test -p egret-auth --test cedar_grant_source`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add migrations/0240_cedar_grants_mirror.sql crates/egret-auth/src/grants.rs crates/egret-auth/tests/cedar_grant_source.rs
git commit -m "feat(auth): EO4 cedar_grants mirror table, sync trigger, and fold"
```

---

### Task 3: Route `grants_for` through the switch + the equivalence report

**Files:**
- Modify: `crates/egret-auth/src/grants.rs` (`grants_for` reads the switch; add `report`)
- Test: `crates/egret-auth/tests/cedar_grant_source.rs` (per-source read; fail-safe; report empty when faithful; report catches a seeded divergence — teeth)

**Interfaces:**
- Consumes: `grant_source_switch::{read_source, GrantSource}`.
- Produces: `grants_for` unchanged signature but source-aware; `pub struct GrantDivergence { kind: GrantDivergenceKind, id: Uuid, detail: String }`, `GrantDivergenceKind { OnlyLegacy, OnlyUnified }`; `AccessGrantStore::report(&self) -> Result<Vec<GrantDivergence>>`.

- [ ] **Step 1: Write the failing per-source + teeth tests**

Add to `crates/egret-auth/tests/cedar_grant_source.rs`:

```rust
#[tokio::test]
async fn report_empty_when_mirror_is_faithful_and_catches_divergence() {
    let _g = SWITCH_LOCK.lock().await;
    let db = db().await;
    let store = egret_auth::grants::AccessGrantStore::new(db.clone());
    store.fold().await.unwrap();
    assert!(store.report().await.unwrap().is_empty(), "faithful mirror ⇒ empty report");

    // Teeth: seed an extra mirror row that does NOT exist in access_grants.
    let ghost = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO perm.cedar_grants (id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at) \
                 VALUES ($1,'user',$1,$1,'platform',NULL,NULL,'allow',NULL)")
        .bind(ghost).execute(&db.pool).await.unwrap();
    let r = store.report().await.unwrap();
    assert!(r.iter().any(|d| d.id == ghost), "report catches a mirror-only row");
    sqlx::query("DELETE FROM perm.cedar_grants WHERE id = $1").bind(ghost).execute(&db.pool).await.unwrap();
}

#[tokio::test]
async fn grants_for_reads_the_source_the_switch_names() {
    // legacy: reads access_grants; unified: reads perm.cedar_grants. With a faithful
    // mirror both return the same set for a seeded subject. Seed a role+grant, fold,
    // assert grants_for equal under both states; then add a mirror-only ghost grant,
    // assert it appears ONLY under unified (proving the read actually switches tables).
    // Restore legacy at the end.
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `CARGO_TARGET_DIR=target-eo4 cargo test -p egret-auth --test cedar_grant_source report_empty grants_for_reads`
Expected: FAIL (`report` absent; `grants_for` not source-aware).

- [ ] **Step 3: Implement source-aware `grants_for` + `report`**

In `crates/egret-auth/src/grants.rs`, route the read. Because `sqlx::query_as` takes a `&'static str`, keep two literal queries differing only in the table name and pick by the switch:

```rust
pub async fn grants_for(
    &self,
    user_id: UserId,
    ws: Option<WorkspaceId>,
    object_scope_ids: &[uuid::Uuid],
) -> Result<Vec<Grant>> {
    use crate::pdp::grant_source_switch::{read_source, GrantSource};
    // Identical predicate; only the table differs. `legacy` is byte-identical to pre-EO4.
    const LEGACY_SQL: &str = "SELECT ag.role_id, ag.scope_type, ag.scope_id, ag.object_type, ag.effect \
         FROM access_grants ag WHERE ( (ag.principal_type = 'user' AND ag.principal_id = $1) \
            OR (ag.principal_type = 'group' AND ag.principal_id IN (SELECT group_id FROM user_groups WHERE user_id = $1)) ) \
         AND ( ag.scope_type = 'platform' OR (ag.scope_type = 'workspace' AND ag.scope_id = $2) \
            OR (ag.scope_type = 'object' AND ag.scope_id = ANY($3)) ) \
         AND (ag.expires_at IS NULL OR ag.expires_at > now())";
    const UNIFIED_SQL: &str = "SELECT ag.role_id, ag.scope_type, ag.scope_id, ag.object_type, ag.effect \
         FROM perm.cedar_grants ag WHERE ( (ag.principal_type = 'user' AND ag.principal_id = $1) \
            OR (ag.principal_type = 'group' AND ag.principal_id IN (SELECT group_id FROM user_groups WHERE user_id = $1)) ) \
         AND ( ag.scope_type = 'platform' OR (ag.scope_type = 'workspace' AND ag.scope_id = $2) \
            OR (ag.scope_type = 'object' AND ag.scope_id = ANY($3)) ) \
         AND (ag.expires_at IS NULL OR ag.expires_at > now())";
    let sql = match read_source(&self.db).await {
        GrantSource::Legacy => LEGACY_SQL,
        GrantSource::Unified => UNIFIED_SQL,
    };
    let rows: Vec<GrantRow> = match sqlx::query_as(sql)
        .bind(user_id.0).bind(ws.map(|w| w.0)).bind(object_scope_ids)
        .fetch_all(&self.db.pool).await {
        Ok(r) => r,
        Err(e) => {
            // Fail-safe (EO-D7): a unified read error falls back to the proven legacy table.
            tracing::error!(error = %e, "EO4 unified grant read failed; FAIL-SAFE to access_grants");
            sqlx::query_as(LEGACY_SQL).bind(user_id.0).bind(ws.map(|w| w.0)).bind(object_scope_ids)
                .fetch_all(&self.db.pool).await.map_err(db_err)?
        }
    };
    Ok(rows.into_iter().map(|(role_id, scope_type, scope_id, object_type, effect)|
        Grant { role_id, scope_type, scope_id, object_type, effect }).collect())
}
```

Add the report (replica-fidelity: a symmetric row-set diff is a tight, faithful oracle — equal row-sets ⇒ identical `grants_for` output for every subject ⇒ identical PDP decisions):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GrantDivergenceKind { OnlyLegacy, OnlyUnified }

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct GrantDivergence { pub kind: GrantDivergenceKind, pub id: uuid::Uuid, pub detail: String }

impl AccessGrantStore {
    /// EO4: the equivalence report. A grant row present under one source and not
    /// the other is a divergence; empty ⇒ the unified mirror is faithful and the
    /// switch is safe. Compared over the full decision-relevant column tuple.
    pub async fn report(&self) -> Result<Vec<GrantDivergence>> {
        const COLS: &str = "id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at";
        // rows in access_grants but not the mirror:
        let only_legacy: Vec<(uuid::Uuid,)> = sqlx::query_as(
            "SELECT id FROM (SELECT id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at FROM access_grants \
             EXCEPT SELECT id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at FROM perm.cedar_grants) x",
        ).fetch_all(&self.db.pool).await.map_err(db_err)?;
        let only_unified: Vec<(uuid::Uuid,)> = sqlx::query_as(
            "SELECT id FROM (SELECT id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at FROM perm.cedar_grants \
             EXCEPT SELECT id,principal_type,principal_id,role_id,scope_type,scope_id,object_type,effect,expires_at FROM access_grants) x",
        ).fetch_all(&self.db.pool).await.map_err(db_err)?;
        let _ = COLS;
        let mut out = Vec::new();
        for (id,) in only_legacy { out.push(GrantDivergence { kind: GrantDivergenceKind::OnlyLegacy, id, detail: "grant in access_grants but not the unified mirror".into() }); }
        for (id,) in only_unified { out.push(GrantDivergence { kind: GrantDivergenceKind::OnlyUnified, id, detail: "grant in the unified mirror but not access_grants".into() }); }
        Ok(out)
    }
}
```

- [ ] **Step 4: Run the tests + the full regression**

Run: `CARGO_TARGET_DIR=target-eo4 cargo test -p egret-auth --test cedar_grant_source`
Then the regression gate (default `legacy` must leave every current test unchanged):
Run: `CARGO_TARGET_DIR=target-eo4 cargo test -p egret-auth` (the full PDP suite incl. `cedar_pdp`, `pdp_hot_reload`, `object_scope`, `pdp_expiry`, `cedar_differential`, …)
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-auth/src/grants.rs crates/egret-auth/tests/cedar_grant_source.rs
git commit -m "feat(auth): EO4 source-aware grants_for with fail-safe + equivalence report"
```

---

### Task 4: Operator HTTP endpoints

**Files:**
- Create: `crates/egret-auth/src/cedar_grant_routes.rs`
- Modify: `crates/egret-auth/src/lib.rs` (`pub mod cedar_grant_routes;` + merge `CedarGrantApi` into `openapi()`, mirroring how EO3's `PdpSourceApi` is merged)
- Modify: `crates/egret-server/src/lib.rs` and `crates/egret-bin/src/main.rs` (mount the router under `/api/control-plane`, mirroring EO3's `pdp_source` mount — `AppState.cedar_grant: Option<CedarGrantState>`, CSRF group, `Some` in main.rs)
- Test: `crates/egret-auth/tests/cedar_grant_routes.rs` (new)

**Interfaces:**
- Consumes: `AccessGrantStore::{fold, report, clear_mirror}`, `grant_source_switch::{read_state, write_source, GrantSource}`, `principal::PlatformAuth`, `session::SessionService`, `EventBus`, `RolesChanged`.
- Produces: `pub struct CedarGrantState { db, sessions, bus }` (+ `FromRef<_> for SessionService`); `pub fn cedar_grant_router() -> Router<CedarGrantState>`; `CedarGrantApi` (utoipa `OpenApi`).

- [ ] **Step 1: Write the router + handlers**

Create `crates/egret-auth/src/cedar_grant_routes.rs`, mirroring `pdp_source_routes.rs` exactly. Routes (all `PlatformAuth`; every mutation publishes `RolesChanged`):

```
GET  /perm/cedar-grants          -> get_state   (200 GrantSourceState | 403)
POST /perm/cedar-grants/report   -> post_report (200 [GrantDivergence] | 403 | 500)
POST /perm/cedar-grants/fold     -> post_fold   (200 GrantSourceState; backfills the mirror; stays legacy | 403)
POST /perm/cedar-grants/switch   -> post_switch (200 GrantSourceState | 403 | 409)
POST /perm/cedar-grants/undo     -> post_undo   (200 GrantSourceState | 403)
```

`post_switch` gate (mirrors EO3's `/switch`, two gates):

```rust
async fn post_switch(State(st): State<CedarGrantState>, PlatformAuth(p): PlatformAuth)
    -> Result<Json<GrantSourceState>, egret_core::Error> {
    let store = AccessGrantStore::new(st.db.clone());
    // Gate 1: a fresh report must be clean (the mirror is faithful right now).
    let report = store.report().await?;
    if !report.is_empty() {
        return Err(egret_core::Error::Conflict(format!(
            "refusing to switch: {} grant divergence(s) between access_grants and the unified mirror", report.len())));
    }
    // (No shadow state in EO4; the clean report is the gate. Fold first if the mirror is empty.)
    write_source(&st.db, GrantSource::Unified, &p.email).await?;
    st.bus.publish(RolesChanged);
    Ok(Json(read_state(&st.db).await?))
}
```

`post_undo` sets `GrantSource::Legacy` (two-state switch; idempotent at legacy), publishes `RolesChanged`. `post_fold` calls `store.fold()` then returns the (still `legacy`) state. Confirm `egret_core::http::ErrorBody` path from the EO3 routes file.

- [ ] **Step 2–3: Export + mount**

Mirror EO3's `pdp_source_routes` wiring precisely: `pub mod cedar_grant_routes;` in `lib.rs`, merge `CedarGrantApi` into `egret_auth::openapi()` (copy the `PdpSourceApi` merge lines), and mount in `egret-server`/`egret-bin` via an `AppState.cedar_grant: Option<CedarGrantState>` CSRF group set to `Some` in `main.rs` with the same `bus` the PDP subscribes to. Grep EO3's commits for the exact lines: `grep -rn "pdp_source" crates/egret-server/src/lib.rs crates/egret-bin/src/main.rs crates/egret-auth/src/lib.rs`.

- [ ] **Step 4: Write the route tests**

Create `crates/egret-auth/tests/cedar_grant_routes.rs` using the same harness as `pdp_source_routes.rs`. Assert: `GET` admin → 200 `legacy`; non-admin → 403; `POST /fold` → 200, mirror populated; `POST /switch` on a faithful mirror → 200 `unified`; `POST /switch` with a seeded mirror-only ghost → 409; `POST /undo` → `legacy`.

- [ ] **Step 5: Run + build**

Run: `CARGO_TARGET_DIR=target-eo4 cargo test -p egret-auth --test cedar_grant_routes` and `CARGO_TARGET_DIR=target-eo4 cargo build -p egret-bin -p egret-server`
Expected: PASS + builds.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-auth/src/cedar_grant_routes.rs crates/egret-auth/src/lib.rs crates/egret-server/src/lib.rs crates/egret-bin/src/main.rs crates/egret-auth/tests/cedar_grant_routes.rs
git commit -m "feat(auth): EO4 operator endpoints for the cedar grant-source switch"
```

---

### Task 5: End-to-end hot flip + rollback + live-sync

**Files:**
- Test: `crates/egret-auth/tests/cedar_grant_hot_flip.rs` (new) — mirrors `pdp_source_hot_flip.rs`

**Interfaces:**
- Consumes: `CedarPdp::build`, `AccessGrantStore::{fold, clear_mirror}`, `grant_source_switch::write_source`, `EventBus`, `RolesChanged`, the `Pdp::authorize` path.

- [ ] **Step 1: Write the failing hot-flip test**

Create `crates/egret-auth/tests/cedar_grant_hot_flip.rs`, modeled on `pdp_source_hot_flip.rs` (reuse its fixture, `RolesChanged` publish, and reload-wait idiom). Over a seeded DB with at least one role and one object/workspace grant:

1. Build the PDP with the switch at `legacy`. Record decisions for a small corpus (`authorize` over a couple of (subject, action, resource), including an allow and a deny).
2. `fold`; `write_source(Unified)`; publish `RolesChanged`; wait. Assert the SAME decisions (faithful mirror ⇒ identical links) and that the live `arc_swap` served set is unchanged.
3. **Live-sync proof:** while `unified`, insert a new `access_grants` grant that allows an action currently denied (the trigger mirrors it); publish `RolesChanged`; wait. Assert the decision now reflects the new grant — proving the unified read is live, not a stale snapshot. Then remove it and re-assert.
4. `write_source(Legacy)` (rollback); publish `RolesChanged`; wait. Assert the original decisions again — instant reversibility to the proven path.

The corpus decisions identical across legacy↔unified (phases 2,4) plus the live-sync (phase 3) are the end-to-end equivalence + reversibility + freshness proof.

- [ ] **Step 2: Run it**

Run: `CARGO_TARGET_DIR=target-eo4 cargo test -p egret-auth --test cedar_grant_hot_flip`
Expected: PASS. Restore the switch to `legacy` at test end (shared row).

- [ ] **Step 3: Commit**

```bash
git add crates/egret-auth/tests/cedar_grant_hot_flip.rs
git commit -m "test(auth): EO4 e2e hot flip, rollback, and live-sync for the grant-source switch"
```

---

## Self-Review

- **Spec coverage:** EO4-D1 mirror (Task 2 `perm.cedar_grants` mirrors access_grants columns; no expand) ✓; EO4-D2 sibling table, `perm.grants` untouched ✓; EO4-D3 per-EP reversible switch re-read every reload (Task 1 + `grants_for` reads it) ✓; EO4-D4 equivalence-gated flip with teeth (Task 3 report + Task 4 `/switch` gate + teeth test) ✓; EO4-D5 reversible, finalize deferred (undo to legacy; access_grants write path untouched; no deletion); provenance refined — the replica carries row lineage by id + switch audit, no `perm.migrated_from` (verified unsuitable) ✓; EO4-D6 no vocab/role/decision change (`link_grants` untouched; roles/role_permissions untouched) ✓. Verification items 1–5 of EO0 map to Tasks 1–5 ✓.
- **Live-correctness:** the trigger (Task 2) makes `unified` reads reflect writes immediately — the e2e live-sync phase (Task 5.3) proves it, closing the "stale snapshot breaks authz" risk a point-in-time copy would carry.
- **Placeholder scan:** the two test bodies with `// ...` (Task 2 Step 2 trigger-mirror insert, Task 3 Step 1 `grants_for_reads` seed) describe the exact inserts/asserts in prose with concrete columns; the implementer fills the seed rows from the crate's existing grant fixtures. No logic is left unspecified.
- **Type consistency:** `GrantSource`/`GrantSourceState` (Task 1) used verbatim in Tasks 3–4; `GrantDivergence`/`GrantDivergenceKind` (Task 3) used in Task 4; `AccessGrantStore::{fold,clear_mirror,report}` signatures match across tasks; `Grant`/`GrantRow` unchanged from the current `grants.rs`.
- **Constraints carried:** chrono-not-time → Global Constraints + Task 1 ✓; migrations 0239/0240 renumber-at-merge → Global Constraints ✓; `perm.migrated_from` real columns checked before use → Task 2 Step 3 ✓; docs-site runbook PR separate → Global Constraints ✓; default `legacy` byte-identical + full regression gate → Task 3 Step 4 ✓.
```
