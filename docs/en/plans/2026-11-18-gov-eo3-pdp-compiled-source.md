# EO3 — PDP consumes IR-compiled policy behind a reversible switch — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the live `CedarPdp` serve the `egret-cedar`-compiled `PolicySet` behind a global, reversible `legacy→shadow→compiled` switch that defaults to legacy, shadow-compares on production data, and rolls back instantly.

**Architecture:** A singleton DB row (`perm.pdp_policy_source`) selects the serve-source; the PDP re-reads it every reload. `load_inner` orchestrates: `legacy` serves the existing SQL-built set; `shadow`/`compiled` build **both** sets (SQL legacy + `egret_cedar::compile(load_inputs(db))`), set-compare them by policy text, log/metric divergences, and serve legacy (shadow) or compiled (compiled, fail-safe to legacy on compile error). Thin platform-admin HTTP handlers in `egret-auth` read the state, run a live equivalence report, and move the switch (switch→compiled gated on a clean report), publishing `RolesChanged` to apply promptly.

**Tech Stack:** Rust, `cedar-policy` 4.12, `sqlx` 0.9 (Postgres), `axum`, `utoipa`, `tokio`, `arc_swap`.

## Global Constraints

- Chat Vietnamese; all code, comments, commit messages, and docs in English.
- Conventional Commits. `Closes #N` only in the PR body, never in commit messages.
- Co-author every commit: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. PR body ends with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.
- Never commit secrets. Stage by explicit path (never `git add -A`). `target-eo3/` is NOT gitignored — never stage it.
- This worktree: prefix every cargo invocation with `CARGO_TARGET_DIR=target-eo3`.
- Migration `0238_pdp_policy_source.sql` — migration numbers are a shared namespace; **renumber above `main` and any open PR at merge time**.
- Fail closed / fail safe (EO-D7): any unreadable switch state or compiled-build error ⇒ serve the legacy (proven) path; never block boot or a reload; never break authz.
- **Zero behaviour change on merge:** default state is `legacy`; `build`/`load_inner` keep their current signatures so the ~120 existing `CedarPdp::build(db, &bus)` call sites and every current PDP test compile and behave unchanged.
- DB-gated tests use `egret_test_db::url()` and must fail (not silently skip) without a DB unless `EGRET_ALLOW_DB_SKIP` is set.
- EO3 is library + control-plane-API work: it adds operator HTTP endpoints, so a **separate `egret-data/docs` PR** (Platform runbook) is required — tracked outside this code plan.

## Interfaces (verified against the merged EO2 tree)

- `egret_cedar::load::load_inputs(db: &egret_metastore::Db) -> anyhow::Result<egret_cedar::compile::CompileInputs>`.
- `egret_cedar::compile::compile(&CompileInputs) -> Result<Compiled, CompileError>`, `Compiled { policy_set: PolicySet, role_names: HashMap<Uuid, String>, skipped_classifications: Vec<Uuid> }`, `CompileError` is `std::error::Error`.
- `egret-auth` PDP (`crates/egret-auth/src/pdp/mod.rs`): private `struct PdpInner { templates: PolicySet, role_names: HashMap<Uuid, String> }`; `async fn load_inner(db: &Db) -> Result<PdpInner>` (the body to split); `fn spawn_reload(&self, rx)` calls `CedarPdp::load_inner`; `#[doc(hidden)] pub async fn compiled_policy_set_for_test(db)`; `Db = egret_metastore::Db` with `db.pool: sqlx::PgPool`; `Result<T> = egret_core::Result<T>`.
- `cedar_policy::PolicySet`: `.templates()` → items with `.id().to_string()` + `.to_string()`; `.policies()` → items with `.id().to_string()` + `.to_string()`.
- `egret_core::Error { NotFound(String), InvalidInput(String), Forbidden, Conflict(String), Internal(anyhow::Error) }`, `impl IntoResponse`; `Conflict` ⇒ 409. `pub type Result<T> = Result<T, Error>`.
- `egret_auth::principal::{Auth, PlatformAuth(pub Principal)}` — `PlatformAuth` gates platform-admin (403 otherwise), needs only `SessionService: FromRef<State>`. `egret_core::Principal { user_id, email: String, platform_roles, … }`.
- `egret_events::EventBus::publish<E: Event>(&self, event: E)`; `egret_auth::events::RolesChanged` is a unit struct and an `Event`. The PDP's `spawn_reload` already reloads on `RolesChanged`.
- Router precedent: `crates/egret-catalog-engine/src/warehouse_admin/mod.rs` (`#[utoipa::path]` with FULL mounted path; `PlatformAuth(p)`; `p.email` audit actor; handlers return `Result<Json<T>, _>`). egret-auth already exposes routers (`oidc_router`, `auth_router` in `src/routes.rs`/`src/oidc_routes.rs`).

---

### Task 1: Switch store — migration + `PolicySource` + accessors

**Files:**
- Create: `migrations/0238_pdp_policy_source.sql`
- Create: `crates/egret-auth/src/pdp/source_switch.rs`
- Modify: `crates/egret-auth/src/pdp/mod.rs` (add `pub mod source_switch;` near the other `pub mod` lines at the top of the file)
- Test: `crates/egret-auth/tests/pdp_source_switch.rs` (new; store tests)

**Interfaces:**
- Produces: `PolicySource { Legacy, Shadow, Compiled }` (+ `as_db()->&'static str`, `from_db(&str)->Option<Self>`); `SourceState { source: PolicySource, switched_at: Option<OffsetDateTime>, switched_by: Option<String> }`; `async fn read_source(db) -> PolicySource`; `async fn read_state(db) -> egret_core::Result<SourceState>`; `async fn write_source(db, PolicySource, actor: &str) -> egret_core::Result<()>`.
- Consumes: `egret_metastore::Db`, `sqlx`.

- [ ] **Step 1: Write the migration**

Create `migrations/0238_pdp_policy_source.sql`:

```sql
-- 0238_pdp_policy_source.sql
--
-- EO3 (PDP consumes the IR-compiled policy): the GLOBAL, single-row switch that
-- selects which assembler produces the Cedar `PolicySet` the control-plane PDP
-- serves. `CedarPdp::load_inner` reads it on every reload (initial build and
-- every RolesChanged):
--
--   * `legacy` (the DEFAULT, and the state with no row): assemble the set from
--     `load_inner`'s bespoke SQL exactly as before EO3 — the proven path.
--   * `shadow`: also build the egret-cedar-compiled set, compare the two, log /
--     metric any divergence, but still SERVE legacy. A production-data canary.
--   * `compiled`: serve the egret-cedar-compiled set (still comparing each
--     reload); fail-safe to legacy if the compiled build errors.
--
-- Behaviour-preserving: EO2's differential proved `compile()` decision-equivalent
-- to `load_inner()`; shipping this migration with no row keeps every PDP on
-- `legacy`, so it enables nothing on its own. One row only (singleton via the
-- `id = true` primary key). `switched_at` / `switched_by` are audit columns.
CREATE TABLE perm.pdp_policy_source (
    id           boolean PRIMARY KEY DEFAULT true CHECK (id),
    source       text NOT NULL DEFAULT 'legacy'
                 CHECK (source IN ('legacy', 'shadow', 'compiled')),
    switched_at  timestamptz,
    switched_by  text
);
```

(No seeded row: an absent row reads as `legacy`, identical to a defaulted row, and keeps the migration a pure no-op.)

- [ ] **Step 2: Write the store module**

Create `crates/egret-auth/src/pdp/source_switch.rs`:

```rust
//! EO3: the global, single-row switch selecting which assembler the PDP serves
//! (`perm.pdp_policy_source`). Read on every reload; written by the operator
//! endpoints. Default (no row) is `Legacy` — the proven, pre-EO3 path.

use egret_metastore::Db;
use serde::Serialize;
use time::OffsetDateTime;

/// Which assembler the live PDP serves. Stored as text in the DB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PolicySource {
    /// `load_inner`'s bespoke SQL (pre-EO3 behaviour). The default.
    Legacy,
    /// Serve legacy, but also build + compare the compiled set (canary).
    Shadow,
    /// Serve the egret-cedar-compiled set; fail-safe to legacy on error.
    Compiled,
}

impl PolicySource {
    pub fn as_db(self) -> &'static str {
        match self {
            PolicySource::Legacy => "legacy",
            PolicySource::Shadow => "shadow",
            PolicySource::Compiled => "compiled",
        }
    }
    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "legacy" => Some(PolicySource::Legacy),
            "shadow" => Some(PolicySource::Shadow),
            "compiled" => Some(PolicySource::Compiled),
            _ => None,
        }
    }
}

/// The switch row with its audit columns, for the GET endpoint.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SourceState {
    pub source: PolicySource,
    #[serde(with = "time::serde::rfc3339::option")]
    pub switched_at: Option<OffsetDateTime>,
    pub switched_by: Option<String>,
}

/// Read the current source, fail-safe to `Legacy` on any error or absent row.
/// Called on the reload hot path, so it never returns an error — an unreadable
/// switch must not break a reload (EO-D7).
pub async fn read_source(db: &Db) -> PolicySource {
    match sqlx::query_scalar::<_, String>(
        "SELECT source FROM perm.pdp_policy_source WHERE id = true",
    )
    .fetch_optional(&db.pool)
    .await
    {
        Ok(Some(s)) => PolicySource::from_db(&s).unwrap_or_else(|| {
            tracing::error!(source = %s, "unknown pdp_policy_source value; serving legacy");
            PolicySource::Legacy
        }),
        Ok(None) => PolicySource::Legacy,
        Err(e) => {
            tracing::warn!(error = %e, "pdp_policy_source unreadable; serving legacy");
            PolicySource::Legacy
        }
    }
}

/// Read the full switch row (source + audit) for the GET endpoint.
pub async fn read_state(db: &Db) -> egret_core::Result<SourceState> {
    let row: Option<(String, Option<OffsetDateTime>, Option<String>)> = sqlx::query_as(
        "SELECT source, switched_at, switched_by FROM perm.pdp_policy_source WHERE id = true",
    )
    .fetch_optional(&db.pool)
    .await
    .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
    Ok(match row {
        Some((s, at, by)) => SourceState {
            source: PolicySource::from_db(&s).unwrap_or(PolicySource::Legacy),
            switched_at: at,
            switched_by: by,
        },
        None => SourceState {
            source: PolicySource::Legacy,
            switched_at: None,
            switched_by: None,
        },
    })
}

/// Upsert the singleton switch row, stamping the audit columns.
pub async fn write_source(db: &Db, source: PolicySource, actor: &str) -> egret_core::Result<()> {
    sqlx::query(
        "INSERT INTO perm.pdp_policy_source (id, source, switched_at, switched_by) \
         VALUES (true, $1, now(), $2) \
         ON CONFLICT (id) DO UPDATE SET \
           source = EXCLUDED.source, \
           switched_at = EXCLUDED.switched_at, \
           switched_by = EXCLUDED.switched_by",
    )
    .bind(source.as_db())
    .bind(actor)
    .execute(&db.pool)
    .await
    .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
    Ok(())
}
```

Add `pub mod source_switch;` to the `pub mod` block at the top of `crates/egret-auth/src/pdp/mod.rs`. If `time` is not already a dependency of `egret-auth`, add `time = { workspace = true, features = ["serde-well-known"] }` to `crates/egret-auth/Cargo.toml` (check first: `grep '^time' crates/egret-auth/Cargo.toml` and `grep -A3 '\[workspace.dependencies\]' Cargo.toml | grep time`; use the workspace feature set already in use elsewhere — mirror another crate's `time` line rather than inventing features).

- [ ] **Step 3: Run it to verify it fails (no table yet in a stale template) / compiles**

Run: `CARGO_TARGET_DIR=target-eo3 cargo build -p egret-auth`
Expected: compiles. (The store is additive; nothing calls it yet.)

- [ ] **Step 4: Write the store tests**

Create `crates/egret-auth/tests/pdp_source_switch.rs`:

```rust
//! EO3 switch-store tests. DB-gated via egret_test_db.
use egret_auth::pdp::source_switch::{read_source, read_state, write_source, PolicySource};

async fn db() -> egret_metastore::Db {
    let url = egret_test_db::url().await;
    egret_metastore::Db::connect(&url).await.expect("db")
}

#[tokio::test]
async fn default_is_legacy_when_absent() {
    let db = db().await;
    assert_eq!(read_source(&db).await, PolicySource::Legacy);
    let st = read_state(&db).await.expect("state");
    assert_eq!(st.source, PolicySource::Legacy);
    assert!(st.switched_at.is_none() && st.switched_by.is_none());
}

#[tokio::test]
async fn write_then_read_roundtrips_with_audit() {
    let db = db().await;
    write_source(&db, PolicySource::Shadow, "op@egret.local").await.expect("write");
    assert_eq!(read_source(&db).await, PolicySource::Shadow);
    let st = read_state(&db).await.expect("state");
    assert_eq!(st.source, PolicySource::Shadow);
    assert_eq!(st.switched_by.as_deref(), Some("op@egret.local"));
    assert!(st.switched_at.is_some());

    // Singleton: a second write updates the SAME row, not a second one.
    write_source(&db, PolicySource::Compiled, "op2@egret.local").await.expect("write2");
    assert_eq!(read_source(&db).await, PolicySource::Compiled);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM perm.pdp_policy_source")
        .fetch_one(&db.pool).await.expect("count");
    assert_eq!(n, 1, "singleton row");
}
```

Verify the `egret_test_db::url()` / `Db::connect` idiom against an existing `egret-auth/tests/*.rs` and match it exactly (the fixture helper may differ — e.g. `egret_test_db::db()`); use the established helper rather than the sketch above if it diverges.

- [ ] **Step 5: Run the tests**

Run: `CARGO_TARGET_DIR=target-eo3 DATABASE_URL=... cargo test -p egret-auth --test pdp_source_switch`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add migrations/0238_pdp_policy_source.sql crates/egret-auth/src/pdp/source_switch.rs crates/egret-auth/src/pdp/mod.rs crates/egret-auth/Cargo.toml crates/egret-auth/tests/pdp_source_switch.rs
git commit -m "feat(auth): EO3 PDP policy-source switch store (migration + accessors)"
```

---

### Task 2: Builders + equivalence comparison + report

**Files:**
- Modify: `crates/egret-auth/src/pdp/mod.rs` (split `load_inner` body into `build_legacy`; add `build_compiled`, `diff`, `report`)
- Modify: `crates/egret-auth/src/pdp/source_switch.rs` (add `Divergence` DTO + `DivergenceKind`)
- Test: `crates/egret-auth/src/pdp/mod.rs` `#[cfg(test)]` (pure diff) + `crates/egret-auth/tests/pdp_source_switch.rs` (DB report + teeth)

**Interfaces:**
- Consumes: `egret_cedar::{load::load_inputs, compile::compile}`; `PdpInner`.
- Produces: `pub(crate) async fn build_legacy(db) -> Result<PdpInner>`; `pub(crate) async fn build_compiled(db) -> Result<PdpInner>`; `pub(crate) fn diff(legacy: &PdpInner, compiled: &PdpInner) -> Vec<Divergence>`; `pub async fn report(db) -> egret_core::Result<Vec<Divergence>>`; `source_switch::Divergence { kind: DivergenceKind, id: String, detail: String }`.

- [ ] **Step 1: Write the failing pure diff test**

Add to `crates/egret-auth/src/pdp/mod.rs` a `#[cfg(test)] mod eo3_diff_tests`:

```rust
#[cfg(test)]
mod eo3_diff_tests {
    use super::*;
    use crate::pdp::source_switch::DivergenceKind;
    use cedar_policy::{Policy, PolicySet};
    use std::collections::HashMap;

    fn pset(texts: &[(&str, &str)]) -> PolicySet {
        let mut ps = PolicySet::new();
        for (id, body) in texts {
            let pid: PolicyId = id.parse().unwrap();
            ps.add(Policy::parse(Some(pid), *body).unwrap()).unwrap();
        }
        ps
    }

    #[test]
    fn identical_sets_have_no_divergence() {
        let a = pset(&[("p1", "permit(principal, action, resource);")]);
        let b = pset(&[("p1", "permit(principal, action, resource);")]);
        let legacy = PdpInner { templates: a, role_names: HashMap::new() };
        let compiled = PdpInner { templates: b, role_names: HashMap::new() };
        assert!(diff(&legacy, &compiled).is_empty());
    }

    #[test]
    fn missing_and_differing_policies_diverge() {
        let legacy = PdpInner {
            templates: pset(&[
                ("p1", "permit(principal, action, resource);"),
                ("p2", "forbid(principal, action, resource);"),
            ]),
            role_names: HashMap::new(),
        };
        let compiled = PdpInner {
            templates: pset(&[
                ("p1", "forbid(principal, action, resource);"), // text differs
                // p2 missing
                ("p3", "permit(principal, action, resource);"), // only in compiled
            ]),
            role_names: HashMap::new(),
        };
        let d = diff(&legacy, &compiled);
        assert_eq!(d.len(), 3);
        assert!(d.iter().all(|x| matches!(x.kind, DivergenceKind::Policy)));
    }

    #[test]
    fn differing_role_names_diverge() {
        let r = uuid::Uuid::from_u128(1);
        let ps = pset(&[("p1", "permit(principal, action, resource);")]);
        let legacy = PdpInner { templates: ps.clone(), role_names: HashMap::from([(r, "admin".to_string())]) };
        let compiled = PdpInner { templates: ps, role_names: HashMap::from([(r, "ADMIN".to_string())]) };
        let d = diff(&legacy, &compiled);
        assert_eq!(d.len(), 1);
        assert!(matches!(d[0].kind, DivergenceKind::RoleName));
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `CARGO_TARGET_DIR=target-eo3 cargo test -p egret-auth eo3_diff_tests`
Expected: FAIL (no `diff`, no `Divergence`).

- [ ] **Step 3: Add the `Divergence` DTO**

In `crates/egret-auth/src/pdp/source_switch.rs` add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DivergenceKind {
    /// A policy id present on one side only, or whose text differs.
    Policy,
    /// A role id whose compiled name differs from the legacy name.
    RoleName,
}

/// One point where the legacy and compiled assemblers disagree. Expected to be
/// EMPTY (EO2-D1: both reuse the same text formatters), so any entry is a signal.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Divergence {
    pub kind: DivergenceKind,
    /// PolicyId (Policy) or role uuid (RoleName).
    pub id: String,
    /// Human-readable: "only in legacy" | "only in compiled" | "text differs" | the two names.
    pub detail: String,
}
```

- [ ] **Step 4: Split `load_inner` and add the builders + diff + report**

In `crates/egret-auth/src/pdp/mod.rs`:

1. Rename the current `async fn load_inner(db: &Db) -> Result<PdpInner>` to `pub(crate) async fn build_legacy(db: &Db) -> Result<PdpInner>` — its body is moved **verbatim** (it stays the proven SQL path). (The new `load_inner` is added in Task 3; until then, add a temporary `async fn load_inner(db: &Db) -> Result<PdpInner> { Self::build_legacy(db).await }` so `build` and `spawn_reload` keep compiling and behaviour is unchanged.)
2. Make `compiled_policy_set_for_test` call `Self::build_legacy(db)` (was `load_inner`) so the EO2 differential always gets the legacy oracle regardless of switch state.
3. Add `build_compiled`, `diff`, `report`:

```rust
impl CedarPdp {
    /// EO3: assemble the PDP policy via the egret-cedar compiler (the path EO3
    /// switches to). Reads the SAME legacy tables `build_legacy` reads, through
    /// `egret_cedar::load::load_inputs`, then `egret_cedar::compile::compile`.
    pub(crate) async fn build_compiled(db: &Db) -> Result<PdpInner> {
        let inputs = egret_cedar::load::load_inputs(db)
            .await
            .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
        let compiled = egret_cedar::compile::compile(&inputs)
            .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("egret-cedar compile: {e}")))?;
        Ok(PdpInner {
            templates: compiled.policy_set,
            role_names: compiled.role_names,
        })
    }
}
```

```rust
use crate::pdp::source_switch::{Divergence, DivergenceKind};

/// EO3: set-equality comparison of the two assemblers' output. Compares every
/// policy and template by `PolicyId -> text` (byte equality — both sides share
/// the EO2-D1 formatters, so equivalent inputs yield identical text), plus the
/// `role_names` maps. An empty result ⇒ decision-equivalent (strictly stronger
/// than decision equality for identical inputs). Pure; no DB, no Authorizer.
pub(crate) fn diff(legacy: &PdpInner, compiled: &PdpInner) -> Vec<Divergence> {
    fn text_map(ps: &PolicySet) -> std::collections::BTreeMap<String, String> {
        let mut m = std::collections::BTreeMap::new();
        for t in ps.templates() {
            m.insert(t.id().to_string(), t.to_string());
        }
        for p in ps.policies() {
            m.insert(p.id().to_string(), p.to_string());
        }
        m
    }
    let l = text_map(&legacy.templates);
    let c = text_map(&compiled.templates);
    let mut out = Vec::new();
    for (id, lt) in &l {
        match c.get(id) {
            None => out.push(Divergence {
                kind: DivergenceKind::Policy,
                id: id.clone(),
                detail: "only in legacy".into(),
            }),
            Some(ct) if ct != lt => out.push(Divergence {
                kind: DivergenceKind::Policy,
                id: id.clone(),
                detail: "text differs".into(),
            }),
            Some(_) => {}
        }
    }
    for id in c.keys() {
        if !l.contains_key(id) {
            out.push(Divergence {
                kind: DivergenceKind::Policy,
                id: id.clone(),
                detail: "only in compiled".into(),
            });
        }
    }
    // role_names: report any role id whose name differs or is missing on a side.
    let mut role_ids: std::collections::BTreeSet<&uuid::Uuid> = Default::default();
    role_ids.extend(legacy.role_names.keys());
    role_ids.extend(compiled.role_names.keys());
    for id in role_ids {
        let ln = legacy.role_names.get(id);
        let cn = compiled.role_names.get(id);
        if ln != cn {
            out.push(Divergence {
                kind: DivergenceKind::RoleName,
                id: id.to_string(),
                detail: format!("legacy={ln:?} compiled={cn:?}"),
            });
        }
    }
    out
}

impl CedarPdp {
    /// EO3: a one-shot live equivalence report — build BOTH assemblers from the
    /// current DB and diff them. Empty ⇒ safe to switch to `compiled`. Read-only.
    pub async fn report(db: &Db) -> Result<Vec<Divergence>> {
        let legacy = Self::build_legacy(db).await?;
        let compiled = Self::build_compiled(db).await?;
        Ok(diff(&legacy, &compiled))
    }
}
```

- [ ] **Step 5: Run the pure diff tests**

Run: `CARGO_TARGET_DIR=target-eo3 cargo test -p egret-auth eo3_diff_tests`
Expected: PASS (3 tests).

- [ ] **Step 6: Write the DB report + teeth test**

Add to `crates/egret-auth/tests/pdp_source_switch.rs`:

```rust
use egret_auth::pdp::CedarPdp;

#[tokio::test]
async fn report_is_empty_on_a_normal_db() {
    let db = db().await;
    // A realistic DB: the test template already has roles/role_permissions.
    let divergences = CedarPdp::report(&db).await.expect("report");
    assert!(divergences.is_empty(), "expected equivalence, got {divergences:?}");
}

#[tokio::test]
async fn report_catches_a_seeded_divergence() {
    let db = db().await;
    // Teeth: insert a role with a permission reachable only by the legacy path
    // vs the compiled path would be a deeper mutation; the simplest robust teeth
    // is to assert the diff machinery itself flags an injected mismatch. Build
    // both, then mutate a role name row so role_names diverge, and re-report.
    // (Pick the concrete seam with the implementer; the REQUIRED property is:
    //  a DB state exists for which report() is non-empty, proving non-vacuity.)
    // Minimal concrete seam: rename a role between the two builds is not possible
    // in one report() call, so instead assert diff() directly over build_legacy
    // vs a hand-mutated compiled set:
    let legacy = egret_auth::pdp::test_build_legacy(&db).await.expect("legacy");
    let mut compiled = egret_auth::pdp::test_build_compiled(&db).await.expect("compiled");
    // Drop one role name to force a RoleName divergence.
    if let Some((&k, _)) = compiled.role_names_mut().iter().next().map(|(k, v)| (k, v.clone())).as_ref().map(|(k, v)| (k, v)) {
        compiled.role_names_mut().remove(k);
    }
    let d = egret_auth::pdp::test_diff(&legacy, &compiled);
    assert!(!d.is_empty(), "teeth: a dropped role name must diverge");
}
```

Because `PdpInner`/`build_*`/`diff` are `pub(crate)`, expose **thin `#[doc(hidden)]` test shims** in `mod.rs` for the integration test (mirroring the existing `compiled_policy_set_for_test` pattern) rather than making internals public:

```rust
#[doc(hidden)]
pub async fn report_for_test(db: &Db) -> Result<Vec<crate::pdp::source_switch::Divergence>> {
    Self::report(db).await
}
```

Prefer this shape for the teeth: seed a role whose effective-permission closure the two assemblers compute differently is NOT possible (they share the formatters), so the honest teeth is **mutation of one assembler's output** proving `diff`/`report` is non-vacuous. Keep the teeth in the pure `eo3_diff_tests` (Step 1 already covers differing policies and role names) and keep the DB test to just `report_is_empty_on_a_normal_db`. Drop the fragile DB-mutation sketch above; the pure tests are the teeth.

- [ ] **Step 7: Run tests**

Run: `CARGO_TARGET_DIR=target-eo3 DATABASE_URL=... cargo test -p egret-auth --test pdp_source_switch` and `... eo3_diff_tests`
Expected: PASS. Also run `... --test cedar_differential` to confirm the EO2 differential still passes with `compiled_policy_set_for_test` now calling `build_legacy`.

- [ ] **Step 8: Commit**

```bash
git add crates/egret-auth/src/pdp/mod.rs crates/egret-auth/src/pdp/source_switch.rs crates/egret-auth/tests/pdp_source_switch.rs
git commit -m "feat(auth): EO3 compiled builder + policy-set equivalence report"
```

---

### Task 3: Reload orchestration — serve-by-state, shadow compare, fail-safe

**Files:**
- Modify: `crates/egret-auth/src/pdp/mod.rs` (the real `load_inner` orchestration; `spawn_reload` stays unchanged)
- Test: `crates/egret-auth/tests/pdp_source_switch.rs` (per-state served set, fail-safe, default regression)

**Interfaces:**
- Consumes: `source_switch::{read_source, PolicySource}`, `build_legacy`, `build_compiled`, `diff`.
- Produces: `async fn load_inner(db: &Db) -> Result<PdpInner>` (state-aware; same signature as before, so `build`/`spawn_reload` and all call sites are untouched).

- [ ] **Step 1: Write the failing per-state test**

Add to `crates/egret-auth/tests/pdp_source_switch.rs`. Use the `#[doc(hidden)]` accessor that returns the CURRENTLY-SERVED template ids (add one if absent):

```rust
// mod.rs: #[doc(hidden)] pub async fn served_policy_ids_for_test(db: &Db) -> Result<Vec<String>>
//   { let inner = Self::load_inner(db).await?; let mut ids: Vec<String> =
//     inner.templates.templates().map(|t| t.id().to_string())
//     .chain(inner.templates.policies().map(|p| p.id().to_string())).collect();
//     ids.sort(); Ok(ids) }

#[tokio::test]
async fn each_state_serves_the_expected_assembler() {
    use egret_auth::pdp::source_switch::{write_source, PolicySource};
    use egret_auth::pdp::CedarPdp;
    let db = db().await;

    let legacy_ids = CedarPdp::served_ids_for_test_legacy(&db).await.expect("legacy"); // build_legacy ids
    let compiled_ids = CedarPdp::served_ids_for_test_compiled(&db).await.expect("compiled"); // build_compiled ids

    write_source(&db, PolicySource::Legacy, "t").await.unwrap();
    assert_eq!(CedarPdp::served_policy_ids_for_test(&db).await.unwrap(), legacy_ids);

    write_source(&db, PolicySource::Shadow, "t").await.unwrap();
    assert_eq!(CedarPdp::served_policy_ids_for_test(&db).await.unwrap(), legacy_ids, "shadow serves legacy");

    write_source(&db, PolicySource::Compiled, "t").await.unwrap();
    assert_eq!(CedarPdp::served_policy_ids_for_test(&db).await.unwrap(), compiled_ids, "compiled serves compiled");
}
```

(On a normal DB `legacy_ids == compiled_ids`, so to make the assertion meaningful the implementer MAY instead assert that `load_inner` in `compiled` state returns the SAME `PdpInner` the `build_compiled` accessor returns, and in `legacy`/`shadow` the same as `build_legacy` — compare by value/ids. The REQUIRED property: the served set matches the state's assembler.)

- [ ] **Step 2: Run it to verify it fails**

Run: `CARGO_TARGET_DIR=target-eo3 DATABASE_URL=... cargo test -p egret-auth --test pdp_source_switch each_state`
Expected: FAIL (load_inner not yet state-aware).

- [ ] **Step 3: Implement the orchestration**

Replace the temporary `load_inner` in `crates/egret-auth/src/pdp/mod.rs` with:

```rust
/// EO3: the reload orchestrator. Reads the switch (fail-safe to legacy) and
/// returns the PdpInner to serve. `shadow`/`compiled` also build the compiled
/// set and compare it (a production-data canary); `compiled` serves it, failing
/// safe to legacy if the compiled build errors. Same signature as pre-EO3, so
/// `build` and `spawn_reload` are unchanged and the default (`legacy`) path is
/// byte-identical to before.
async fn load_inner(db: &Db) -> Result<PdpInner> {
    use crate::pdp::source_switch::{read_source, PolicySource};
    match read_source(db).await {
        PolicySource::Legacy => Self::build_legacy(db).await,
        PolicySource::Shadow => {
            let legacy = Self::build_legacy(db).await?;
            match Self::build_compiled(db).await {
                Ok(compiled) => Self::log_shadow(&legacy, &compiled),
                Err(e) => tracing::error!(error = ?e, "EO3 shadow: compiled build failed; serving legacy"),
            }
            Ok(legacy) // shadow always serves legacy
        }
        PolicySource::Compiled => {
            // Build legacy too, so we keep comparing (canary) AND have a fail-safe.
            let legacy = Self::build_legacy(db).await?;
            match Self::build_compiled(db).await {
                Ok(compiled) => {
                    Self::log_shadow(&legacy, &compiled);
                    Ok(compiled) // serve compiled
                }
                Err(e) => {
                    tracing::error!(error = ?e, "EO3 compiled: compiled build failed; FAIL-SAFE to legacy");
                    // metric: a compiled-state fallback is an alarm condition.
                    Ok(legacy)
                }
            }
        }
    }
}

/// EO3: emit the shadow comparison result. Empty ⇒ equivalent (info); any
/// divergence ⇒ a warn per entry plus a count, the operator's cutover signal.
fn log_shadow(legacy: &PdpInner, compiled: &PdpInner) {
    let d = diff(legacy, compiled);
    if d.is_empty() {
        tracing::info!("EO3 shadow: legacy and compiled policy sets are equivalent");
    } else {
        tracing::warn!(divergences = d.len(), "EO3 shadow: policy sets DIVERGE");
        for x in &d {
            tracing::warn!(kind = ?x.kind, id = %x.id, detail = %x.detail, "EO3 shadow divergence");
        }
    }
}
```

Add the `#[doc(hidden)]` test accessors (`served_policy_ids_for_test`, and `served_ids_for_test_legacy/compiled` or value-returning equivalents) used by the test.

- [ ] **Step 4: Run the per-state test + fail-safe + regression**

Add a fail-safe test (state `compiled`, DB rigged so `build_compiled` errors — e.g. a classification row whose body cannot parse AND whose PolicyId cannot parse, per EO2 `compile()` propagation; pick a seam with the implementer) asserting `load_inner` returns the legacy set and does not error.

Run: `CARGO_TARGET_DIR=target-eo3 DATABASE_URL=... cargo test -p egret-auth --test pdp_source_switch`
Expected: PASS. Then the regression gate:
Run: `CARGO_TARGET_DIR=target-eo3 DATABASE_URL=... cargo test -p egret-auth` (the full PDP suite: `cedar_pdp`, `pdp_hot_reload`, `pdp_role_hierarchy`, `classification_policy`, `object_scope`, `pdp_explicit_deny`, `pdp_expiry`, `cedar_differential`, …)
Expected: PASS — default `legacy` leaves every existing test unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-auth/src/pdp/mod.rs crates/egret-auth/tests/pdp_source_switch.rs
git commit -m "feat(auth): EO3 state-aware PDP reload with shadow compare and fail-safe"
```

---

### Task 4: Operator HTTP endpoints

**Files:**
- Create: `crates/egret-auth/src/pdp_source_routes.rs`
- Modify: `crates/egret-auth/src/lib.rs` (`pub mod pdp_source_routes;` + re-export the router/state)
- Modify: `crates/egret-bin/src/main.rs` (nest the router at `/api/control-plane`)
- Test: `crates/egret-auth/tests/pdp_source_routes.rs` (new) OR extend an existing route test harness

**Interfaces:**
- Consumes: `CedarPdp::report`, `source_switch::{read_state, write_source, PolicySource}`, `principal::PlatformAuth`, `session::SessionService`, `egret_events::EventBus`, `events::RolesChanged`.
- Produces: `pub struct PdpSourceState { db, sessions, bus }`; `pub fn pdp_source_router() -> Router<PdpSourceState>`.

- [ ] **Step 1: Write the router + handlers**

Create `crates/egret-auth/src/pdp_source_routes.rs`:

```rust
//! EO3: platform-admin endpoints controlling the PDP policy source switch.
//! Nested at `/api/control-plane` (routes carry `/perm/pdp-source`). The switch
//! logic lives in `egret-auth` (the only crate that sees both assemblers); these
//! handlers are thin. Every mutation publishes `RolesChanged` so the PDP reloads
//! and the new source applies promptly; the state is re-read every reload, so a
//! missed nudge is harmless.
use axum::extract::{FromRef, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use egret_events::EventBus;
use egret_metastore::Db;

use crate::events::RolesChanged;
use crate::pdp::source_switch::{read_state, write_source, Divergence, PolicySource, SourceState};
use crate::pdp::CedarPdp;
use crate::principal::PlatformAuth;
use crate::session::SessionService;

#[derive(Clone)]
pub struct PdpSourceState {
    pub db: Db,
    pub sessions: SessionService,
    pub bus: EventBus,
}

impl FromRef<PdpSourceState> for SessionService {
    fn from_ref(s: &PdpSourceState) -> Self {
        s.sessions.clone()
    }
}

pub fn pdp_source_router() -> Router<PdpSourceState> {
    Router::new()
        .route("/perm/pdp-source", get(get_source))
        .route("/perm/pdp-source/report", post(post_report))
        .route("/perm/pdp-source/shadow", post(post_shadow))
        .route("/perm/pdp-source/switch", post(post_switch))
        .route("/perm/pdp-source/undo", post(post_undo))
}

#[utoipa::path(
    get, path = "/api/control-plane/perm/pdp-source", tag = "control-plane-perm",
    responses(
        (status = 200, description = "Current PDP policy source + audit", body = SourceState),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Not a platform admin", body = egret_core::http::ErrorBody),
    ),
)]
async fn get_source(
    State(st): State<PdpSourceState>,
    _admin: PlatformAuth,
) -> Result<Json<SourceState>, egret_core::Error> {
    Ok(Json(read_state(&st.db).await?))
}

#[utoipa::path(
    post, path = "/api/control-plane/perm/pdp-source/report", tag = "control-plane-perm",
    responses(
        (status = 200, description = "Live equivalence report (empty ⇒ safe to switch)", body = [Divergence]),
        (status = 403, description = "Not a platform admin", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
async fn post_report(
    State(st): State<PdpSourceState>,
    _admin: PlatformAuth,
) -> Result<Json<Vec<Divergence>>, egret_core::Error> {
    Ok(Json(CedarPdp::report(&st.db).await?))
}

#[utoipa::path(post, path = "/api/control-plane/perm/pdp-source/shadow", tag = "control-plane-perm",
    responses((status = 200, body = SourceState), (status = 403, body = egret_core::http::ErrorBody)))]
async fn post_shadow(
    State(st): State<PdpSourceState>,
    PlatformAuth(p): PlatformAuth,
) -> Result<Json<SourceState>, egret_core::Error> {
    write_source(&st.db, PolicySource::Shadow, &p.email).await?;
    st.bus.publish(RolesChanged);
    Ok(Json(read_state(&st.db).await?))
}

#[utoipa::path(post, path = "/api/control-plane/perm/pdp-source/switch", tag = "control-plane-perm",
    responses(
        (status = 200, description = "Switched to compiled (report was clean)", body = SourceState),
        (status = 403, body = egret_core::http::ErrorBody),
        (status = 409, description = "Not in shadow, or the report is not clean", body = egret_core::http::ErrorBody),
    ))]
async fn post_switch(
    State(st): State<PdpSourceState>,
    PlatformAuth(p): PlatformAuth,
) -> Result<Json<SourceState>, egret_core::Error> {
    // Gate 1: must be in shadow (soak before cutover).
    if read_state(&st.db).await?.source != PolicySource::Shadow {
        return Err(egret_core::Error::Conflict(
            "switch to compiled requires the current source to be shadow".into(),
        ));
    }
    // Gate 2: a freshly-run report must be clean.
    let report = CedarPdp::report(&st.db).await?;
    if !report.is_empty() {
        return Err(egret_core::Error::Conflict(format!(
            "refusing to switch: {} divergence(s) between legacy and compiled",
            report.len()
        )));
    }
    write_source(&st.db, PolicySource::Compiled, &p.email).await?;
    st.bus.publish(RolesChanged);
    Ok(Json(read_state(&st.db).await?))
}

#[utoipa::path(post, path = "/api/control-plane/perm/pdp-source/undo", tag = "control-plane-perm",
    responses((status = 200, description = "Stepped back one state", body = SourceState),
        (status = 403, body = egret_core::http::ErrorBody)))]
async fn post_undo(
    State(st): State<PdpSourceState>,
    PlatformAuth(p): PlatformAuth,
) -> Result<Json<SourceState>, egret_core::Error> {
    // compiled -> shadow -> legacy. Idempotent at legacy.
    let next = match read_state(&st.db).await?.source {
        PolicySource::Compiled => PolicySource::Shadow,
        PolicySource::Shadow => PolicySource::Legacy,
        PolicySource::Legacy => PolicySource::Legacy,
    };
    write_source(&st.db, next, &p.email).await?;
    st.bus.publish(RolesChanged);
    Ok(Json(read_state(&st.db).await?))
}
```

Confirm `egret_core::http::ErrorBody` exists (the catalog router uses it); if the import path differs, match the catalog router's exact use.

- [ ] **Step 2: Export the router**

In `crates/egret-auth/src/lib.rs` add `pub mod pdp_source_routes;` (and re-export `pub use pdp_source_routes::{pdp_source_router, PdpSourceState};` if the crate re-exports its other routers).

- [ ] **Step 3: Mount in egret-bin**

In `crates/egret-bin/src/main.rs`, where the control-plane router tree is assembled (near the other `/api/control-plane` nests; the existing catalog admin router is nested there), add:

```rust
.nest(
    "/api/control-plane",
    egret_auth::pdp_source_routes::pdp_source_router().with_state(
        egret_auth::pdp_source_routes::PdpSourceState {
            db: db.clone(),
            sessions: sessions.clone(),
            bus: bus.clone(),
        },
    ),
)
```

Match the EXACT nesting idiom already used for `catalog_admin_router()` (it may merge into one `/api/control-plane` router rather than a second `.nest` — follow the local pattern; `bus` is the same `EventBus` the PDP was built from at main.rs ~2295).

- [ ] **Step 4: Write the route tests**

Create `crates/egret-auth/tests/pdp_source_routes.rs` using the crate's established route-test harness (find one: `grep -rl "pdp_source_router\|auth_router\|oneshot" crates/egret-auth/tests` or mirror another crate's axum `oneshot` test). Assert:
- `GET /perm/pdp-source` as platform-admin → 200, `source: "legacy"`.
- `GET`/`POST` as a non-admin principal → 403.
- `POST /perm/pdp-source/shadow` → 200, state `shadow`; `GET` reflects it + `switched_by`.
- `POST /perm/pdp-source/switch` from `shadow` on a clean DB → 200, state `compiled`.
- `POST /perm/pdp-source/switch` from `legacy` → 409; from `shadow` with an injected divergence → 409 (reuse the diff teeth seam if feasible, else assert the clean-path 200 and unit-test the gate logic).
- `POST /perm/pdp-source/undo` from `compiled` → `shadow`; again → `legacy`.

- [ ] **Step 5: Run the route tests + build egret-bin**

Run: `CARGO_TARGET_DIR=target-eo3 DATABASE_URL=... cargo test -p egret-auth --test pdp_source_routes` and `CARGO_TARGET_DIR=target-eo3 cargo build -p egret-bin`
Expected: PASS + builds.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-auth/src/pdp_source_routes.rs crates/egret-auth/src/lib.rs crates/egret-bin/src/main.rs crates/egret-auth/tests/pdp_source_routes.rs
git commit -m "feat(auth): EO3 operator endpoints for the PDP policy-source switch"
```

---

### Task 5: End-to-end hot flip + rollback

**Files:**
- Test: `crates/egret-auth/tests/pdp_source_hot_flip.rs` (new) — mirrors `pdp_hot_reload.rs`

**Interfaces:**
- Consumes: `CedarPdp::build`, `source_switch::write_source`, `EventBus`, `RolesChanged`, the `Pdp::authorize` path.

- [ ] **Step 1: Write the failing hot-flip test**

Create `crates/egret-auth/tests/pdp_source_hot_flip.rs`, modeled on `crates/egret-auth/tests/pdp_hot_reload.rs` (reuse its fixture + the `RolesChanged` publish + a short `tokio` yield/await for the reload to land — copy its exact wait idiom). Assert, over a seeded DB with at least one role and one grant:

1. Build the PDP with the switch at `legacy`. Record decisions for a small corpus (`authorize` over a couple of (subject, action, resource)).
2. `write_source(&db, Shadow, …)`; publish `RolesChanged`; wait for reload. Assert the SAME decisions (shadow serves legacy) and that the build did not error.
3. `write_source(&db, Compiled, …)`; publish `RolesChanged`; wait. Assert the SAME decisions (EO2 proved equivalence) — now served from the compiled set.
4. `write_source(&db, Legacy, …)` (rollback); publish `RolesChanged`; wait. Assert the SAME decisions again — proving instant rollback to the proven path.

The corpus decisions being identical across all four phases is the end-to-end equivalence + reversibility proof. (If a stronger assertion is wanted, add a phase that, under `Compiled`, confirms the served template ids equal `build_compiled`'s via the `#[doc(hidden)]` accessor.)

- [ ] **Step 2: Run it to verify it fails / passes against the wiring**

Run: `CARGO_TARGET_DIR=target-eo3 DATABASE_URL=... cargo test -p egret-auth --test pdp_source_hot_flip`
Expected: PASS once Tasks 1–3 are in (this task is pure test; if it fails, the failure is a real wiring bug to fix in `load_inner`/`spawn_reload`).

- [ ] **Step 3: Full-suite + clippy + fmt gate**

Run:
```bash
CARGO_TARGET_DIR=target-eo3 cargo fmt --all
CARGO_TARGET_DIR=target-eo3 cargo clippy -p egret-auth -p egret-cedar -p egret-bin --all-targets -- -D warnings
CARGO_TARGET_DIR=target-eo3 DATABASE_URL=... cargo test -p egret-auth
```
Expected: clean fmt, zero clippy warnings, all tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-auth/tests/pdp_source_hot_flip.rs
git commit -m "test(auth): EO3 end-to-end PDP source hot flip and rollback"
```

---

## Self-Review

**1. Spec coverage** (against `docs/en/specs/2026-11-18-gov-eo3-pdp-compiled-source.md`):
- EO3-D1 (reversible switch, default legacy, re-read every reload) → Task 1 (store) + Task 3 (`load_inner` reads `read_source` every call). ✓
- EO3-D2 (shadow before cutover on production data) → Task 3 (`shadow`/`compiled` build+compare) + Task 4 (`switch` gated on clean report). ✓
- EO3-D3 (set-equality on policy text + role_names) → Task 2 (`diff`). ✓
- EO3-D4 (switch logic in egret-auth, thin handlers) → Task 2/3 (logic in `pdp`), Task 4 (thin handlers). ✓
- EO3-D5 (fail-safe to legacy on compile error) → Task 3 (`Compiled` arm `Err` → `Ok(legacy)`). ✓
- EO3-D6 (no new decisions / no grant-source change) → `build_compiled` reads the same tables via `load_inputs`; EO2 differential still green (Task 2 Step 7). ✓
- Endpoints (GET/report/shadow/switch/undo, platform-admin) → Task 4. ✓
- Verification items 1–7 → Tasks 1–5 tests; default-legacy regression = Task 3 Step 4 full suite. ✓
- Migration 0238 renumber-at-merge → Global Constraints. ✓
- Docs-site PR → Global Constraints (separate). ✓

**2. Placeholder scan:** Task 2 Step 6 and Task 3 Step 1 intentionally leave the teeth/accessor SEAM to the implementer but state the REQUIRED property and give the pure-test teeth concretely (Step 1–2 of Task 2). No `TODO`/`TBD`. Test-accessor names are described with their exact bodies. ✓

**3. Type consistency:** `PdpInner { templates: PolicySet, role_names: HashMap<Uuid,String> }` used identically in Tasks 2–3. `build_legacy`/`build_compiled` return `Result<PdpInner>`; `diff(&PdpInner,&PdpInner)->Vec<Divergence>`; `report(&Db)->Result<Vec<Divergence>>`; `PolicySource`/`SourceState`/`Divergence`/`DivergenceKind` defined in Task 1–2 and consumed in Task 4 with matching names. `egret_core::Error::Conflict` → 409 used in Task 4 switch gate. `bus.publish(RolesChanged)` matches the verified `EventBus::publish` + unit-struct `RolesChanged`. ✓

**Open seams for the implementer (flagged, not placeholders):** exact `egret_test_db` fixture helper; exact `time` feature flags on egret-auth; the exact `main.rs` control-plane nesting idiom; the fail-safe DB seam that makes `build_compiled` error; the crate's route-test harness idiom. Each names what to match and where to find the precedent.
