# EO5 Soak-Defaults Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Flip both EO3/EO4 reversible switches from the shipped `legacy` default to `shadow` (observe) — serve the proven legacy decisions while building/comparing the new paths and logging divergence — to begin the clean soak that finalize requires. Serve no new path; delete nothing.

**Architecture:** One migration folds `access_grants → perm.cedar_grants` and seeds both singleton switch rows to `shadow` (`ON CONFLICT DO NOTHING`, keeping `read_source`'s absent/unreadable ⇒ `legacy` fail-safe). EO3's `shadow` is already fully built. EO4 gains a symmetric `GrantSource::Shadow`: `grants_for` serves `access_grants` under it, and each PDP reload runs the `report()` row-set equivalence and logs it. EO4 also gets a `POST …/shadow` endpoint and one-level `undo`.

**Tech Stack:** Rust (egret-auth, egret-server), sqlx 0.9, chrono, utoipa; Postgres migrations; orval-generated TS client.

## Global Constraints

- **chrono, not time** — audit timestamps are `chrono::DateTime<Utc>` (both switch modules already use chrono).
- **Prefix every cargo invocation with `CARGO_TARGET_DIR=target-eo5`** (shared-target E0432 otherwise). `target-eo5/` is NOT gitignored by name — never stage it; stage by explicit path.
- **DB for tests:** container `egret-eo5-pg` (postgres:16, port **5468**, db `egret_test`), `DATABASE_URL=postgres://egret:egret@localhost:5468/egret_test`. The MIGRATOR auto-applies migrations to scratch DBs.
- **Migration number 0241** (main is at 0240). Migration numbers are a shared namespace — **renumber above main AND all open PRs at merge time**, not now.
- **sqlx 0.9 `query`/`query_as` take `&'static str`** — no runtime table interpolation; keep the two literal SQL constants pattern already in `grants_for`.
- **Zero decision change** — `shadow` serves exactly the legacy decisions on both switches. Every EO2/EO3/EO4 differential and the existing authz suites MUST stay green.
- **Run the frontend typecheck, not only the drift gates.** The drift gates (`committed_openapi_json_is_current`, `generated_client_was_emitted_from_the_committed_spec`) regenerate deterministically and do NOT run `tsc`; a duplicate/invalid generated client passes them but fails the `frontend` CI job. After any OpenAPI change run `cd frontend && npm run typecheck`. Give the new endpoint a distinct `operation_id` (`cedar_grants_shadow`).
- **Separate `egret-data/docs` PR owed** at ship: update `pdp-policy-source.md` and `cedar-grant-source.md` — the default is now `shadow`/observe; explain the soak and that `legacy` remains the fail-safe floor.
- **Seeded-default fallout:** every scratch test DB now boots at `shadow` (not absent⇒legacy). Tests that assert the *initial state* is `legacy` must change to `shadow`; tests that assert *decisions* stay unchanged (shadow serves legacy). This is expected test surface, concentrated in Task 4.
- Conventional Commits; `Closes #N` only in the PR body. Co-author `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Never commit secrets or scratch files.

---

### Task 1: EO4 `GrantSource::Shadow` variant + `grants_for` routing

**Files:**
- Modify: `crates/egret-auth/src/pdp/grant_source_switch.rs` (add `Shadow` to `GrantSource`, `as_db`/`from_db`)
- Modify: `crates/egret-auth/src/grants.rs:88` (`grants_for` match arms)
- Test: `crates/egret-auth/tests/cedar_grant_source.rs` (routing + error-propagation)

**Interfaces:**
- Consumes: existing `GrantSource { Legacy, Unified }`, `grants_for(source, user_id, ws, object_scope_ids)`.
- Produces: `GrantSource::Shadow` with `as_db()=="shadow"`, `from_db("shadow")==Some(Shadow)`; `grants_for` routes `Legacy | Shadow → access_grants` (errors propagate, no fallback), `Unified → perm.cedar_grants` (unchanged, keeps unified fail-safe).

- [ ] **Step 1: Write the failing test** — add to `cedar_grant_source.rs`:

```rust
#[test]
fn shadow_round_trips_and_is_distinct() {
    use egret_auth::pdp::grant_source_switch::GrantSource;
    assert_eq!(GrantSource::Shadow.as_db(), "shadow");
    assert_eq!(GrantSource::from_db("shadow"), Some(GrantSource::Shadow));
    assert_ne!(GrantSource::Shadow, GrantSource::Legacy);
    assert_ne!(GrantSource::Shadow, GrantSource::Unified);
}
```

(Confirm the exact `pub` path to `GrantSource` in this test file's existing imports and match it.)

- [ ] **Step 2: Run to verify it fails** — `CARGO_TARGET_DIR=target-eo5 cargo test -p egret-auth --test cedar_grant_source shadow_round_trips -- --nocapture`. Expected: FAIL (no `Shadow` variant).

- [ ] **Step 3: Add the variant.** In `grant_source_switch.rs`, add `Shadow` to the enum (with a doc line "Serve `access_grants`, but run the equivalence report each reload (observe)."), and the `as_db`/`from_db` arms `"shadow"`.

- [ ] **Step 4: Route it in `grants_for`.** Change the `sql` match to `GrantSource::Legacy | GrantSource::Shadow => LEGACY_SQL,` and the error match so `(GrantSource::Legacy | GrantSource::Shadow, Err(e)) => return Err(db_err(e)),` while `(GrantSource::Unified, Err(e))` keeps the fail-safe. The build will force every other `match source`/`GrantSource` site to handle `Shadow` — handle each (e.g. `build_legacy`/`build_compiled` placeholders are set via field init, unaffected; any exhaustiveness the compiler flags must treat `Shadow` as "serve legacy").

- [ ] **Step 5: Run tests** — `CARGO_TARGET_DIR=target-eo5 cargo test -p egret-auth --test cedar_grant_source` and `… --lib`. Expected: PASS, no non-exhaustive-match errors.

- [ ] **Step 6: Commit** — `git add crates/egret-auth/src/pdp/grant_source_switch.rs crates/egret-auth/src/grants.rs crates/egret-auth/tests/cedar_grant_source.rs` then `git commit -m "feat(auth): add EO4 GrantSource::Shadow routing (serve access_grants)"`.

---

### Task 2: EO4 shadow observe (report each reload) + `/shadow` endpoint + one-level undo

**Files:**
- Modify: `crates/egret-auth/src/pdp/mod.rs` (`load_inner` runs `report()` + logs when `grant_source == Shadow`)
- Modify: `crates/egret-auth/src/cedar_grant_routes.rs` (add `post_shadow` handler + route + utoipa; `undo` steps back one level)
- Modify: `crates/egret-auth/src/lib.rs` (register the new path in `CedarGrantApi` openapi)
- Test: `crates/egret-auth/tests/cedar_grant_routes.rs`, `crates/egret-auth/tests/cedar_grant_hot_flip.rs`

**Interfaces:**
- Consumes: `GrantSource::Shadow` (Task 1), `AccessGrantStore::new(db).report() -> Result<Vec<GrantDivergence>>`, `write_source`, `read_state`, `RolesChanged`, `PlatformAuth`.
- Produces: a reload-time `log_grant_shadow(db)` behaviour; `POST /api/control-plane/perm/cedar-grants/shadow` (operation_id `cedar_grants_shadow`) writing `GrantSource::Shadow`; `undo` mapping `Unified → Shadow`, `Shadow → Legacy`, `Legacy → Legacy`.

- [ ] **Step 1: Write the failing observe helper test** — a small unit test that `log_grant_shadow` is exercised is awkward (it only logs); instead assert the load path behaviour through the e2e in `cedar_grant_hot_flip.rs`. Write a failing e2e step: seed the DB, set `cedar_grant_source = 'shadow'`, insert a divergent row ONLY into `perm.cedar_grants`, reload the PDP, and assert decisions are the **legacy** decisions (shadow serves access_grants — the mirror-only row is NOT honoured). Add the assertion that a `shadow`-state read via `read_state` returns `GrantSource::Shadow`.

- [ ] **Step 2: Run to verify it fails** — `CARGO_TARGET_DIR=target-eo5 cargo test -p egret-auth --test cedar_grant_hot_flip -- --nocapture`. Expected: FAIL (no `/shadow`, state not settable, or observe not wired).

- [ ] **Step 3: Wire the observe in `load_inner`.** After `inner.grant_source = grant_source_switch::read_source(db).await;`, add: when it is `GrantSource::Shadow`, run the report and log, mirroring `log_shadow`:

```rust
if inner.grant_source == GrantSource::Shadow {
    match crate::grants::AccessGrantStore::new(db.clone()).report().await {
        Ok(d) if d.is_empty() =>
            tracing::info!("EO4 shadow: access_grants and the unified mirror are equivalent"),
        Ok(d) => {
            tracing::warn!(divergences = d.len(), "EO4 shadow: grant sources DIVERGE");
            for x in &d { tracing::warn!(kind = ?x.kind, id = %x.id, detail = %x.detail, "EO4 shadow divergence"); }
        }
        Err(e) => tracing::warn!(error = %e, "EO4 shadow: report failed; serving access_grants"),
    }
}
```

(Confirm `AccessGrantStore`/`report`/`GrantDivergence` field names against `grants.rs`; `Db` is `Clone`.)

- [ ] **Step 4: Add the `/shadow` endpoint.** In `cedar_grant_routes.rs` add `.route("/perm/cedar-grants/shadow", post(post_shadow))`, and:

```rust
#[utoipa::path(post, path = "/api/control-plane/perm/cedar-grants/shadow", tag = "control-plane-perm",
    operation_id = "cedar_grants_shadow",
    responses((status = 200, description = "Entered shadow (observe); still serves access_grants", body = GrantSourceState),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, body = egret_core::http::ErrorBody)))]
async fn post_shadow(
    State(st): State<CedarGrantState>,
    PlatformAuth(p): PlatformAuth,
) -> Result<Json<GrantSourceState>, egret_core::Error> {
    write_source(&st.db, GrantSource::Shadow, &p.email).await?;
    st.bus.publish(RolesChanged);
    Ok(Json(read_state(&st.db).await?))
}
```

Add `post_shadow` to the `CedarGrantApi` `paths(...)` list in `lib.rs`.

- [ ] **Step 5: One-level undo.** Change `post_undo` to read current state and step back one level:

```rust
let next = match read_state(&st.db).await?.source {
    GrantSource::Unified => GrantSource::Shadow,
    GrantSource::Shadow | GrantSource::Legacy => GrantSource::Legacy,
};
write_source(&st.db, next, &p.email).await?;
```

Keep the `RolesChanged` publish and `read_state` response. Update the undo doc comment to "steps back one level: unified → shadow → legacy".

- [ ] **Step 6: Run tests** — `CARGO_TARGET_DIR=target-eo5 cargo test -p egret-auth --test cedar_grant_hot_flip --test cedar_grant_routes`. Expected: PASS. Add/adjust a `cedar_grant_routes.rs` test asserting `undo` from `unified` lands on `shadow`, and from `shadow` lands on `legacy`.

- [ ] **Step 7: Commit** — stage the four files + tests; `git commit -m "feat(auth): EO4 shadow observe, /shadow endpoint, one-level undo"`.

---

### Task 3: Migration 0241 — fold + seed both switches to `shadow`

**Files:**
- Create: `migrations/0241_eo5_soak_defaults.sql`
- Test: `crates/egret-auth/tests/cedar_grant_source.rs` (fold-in-migration faithful; seeded-default = shadow; ON CONFLICT keeps an explicit value)

**Interfaces:**
- Consumes: `access_grants`, `perm.cedar_grants` (0240), `perm.pdp_policy_source` (0238), `perm.cedar_grant_source` (0240).
- Produces: after migration, `perm.cedar_grants` == `access_grants` row-for-row; both switch rows seeded `shadow` unless already set.

- [ ] **Step 1: Write the failing test** — on a fresh migrated DB (the harness applies all migrations): assert `read_state` on both switches returns `shadow`; insert some `access_grants` rows BEFORE the test DB is built is not possible, so instead test fold faithfulness via `report()`: seed matching rows into `access_grants` (the trigger mirrors them), then assert `AccessGrantStore::report()` is empty; and a dedicated test that inserts a row into `access_grants` with the trigger temporarily dropped, runs a manual `fold()`, and asserts `report()` empty (proves fold backfills pre-trigger rows). Also assert an explicit pre-set value is NOT overwritten by re-running the seed SQL (ON CONFLICT DO NOTHING) — exercise by calling `write_source(Unified)` then re-applying the seed statement and checking it stays `unified`.

- [ ] **Step 2: Run to verify it fails** — `CARGO_TARGET_DIR=target-eo5 cargo test -p egret-auth --test cedar_grant_source seed -- --nocapture`. Expected: FAIL (no seeded shadow; default still legacy).

- [ ] **Step 3: Write the migration** — `migrations/0241_eo5_soak_defaults.sql`:

```sql
-- EO5: begin the soak. Fold existing grants into the mirror so it is complete
-- from the first boot, then seed both reversible switches to `shadow` (observe):
-- serve the proven legacy decisions while comparing the new paths. ON CONFLICT
-- DO NOTHING never clobbers an operator's explicit choice; an absent/unreadable
-- row still falls back to `legacy` in code, so `shadow` is the default and
-- `legacy` the fail-safe floor.

INSERT INTO perm.cedar_grants
    (id, principal_type, principal_id, role_id, scope_type, scope_id, object_type, effect, expires_at)
SELECT id, principal_type, principal_id, role_id, scope_type, scope_id, object_type, effect, expires_at
FROM access_grants
ON CONFLICT (id) DO UPDATE SET
    principal_type = EXCLUDED.principal_type, principal_id = EXCLUDED.principal_id,
    role_id = EXCLUDED.role_id, scope_type = EXCLUDED.scope_type, scope_id = EXCLUDED.scope_id,
    object_type = EXCLUDED.object_type, effect = EXCLUDED.effect, expires_at = EXCLUDED.expires_at;

INSERT INTO perm.pdp_policy_source (id, source, switched_at, switched_by)
VALUES (true, 'shadow', now(), 'eo5-migration')
ON CONFLICT (id) DO NOTHING;

INSERT INTO perm.cedar_grant_source (id, source, switched_at, switched_by)
VALUES (true, 'shadow', now(), 'eo5-migration')
ON CONFLICT (id) DO NOTHING;
```

(Verify the exact column lists against 0238/0240 before finalizing.)

- [ ] **Step 4: Run tests** — `CARGO_TARGET_DIR=target-eo5 cargo test -p egret-auth --test cedar_grant_source`. Expected: PASS.

- [ ] **Step 5: Commit** — `git add migrations/0241_eo5_soak_defaults.sql crates/egret-auth/tests/cedar_grant_source.rs` then `git commit -m "feat(migrations): EO5 fold mirror and seed both switches to shadow (0241)"`.

---

### Task 4: Regenerate OpenAPI + client, run frontend typecheck, fix default-baseline fallout, confirm differentials

**Files:**
- Modify: `openapi.json` (regenerated)
- Modify: `frontend/src/api/generated/**` (regenerated)
- Modify: whichever test files assert an initial switch state of `legacy` (now `shadow`)

**Interfaces:** none new — this task makes the tree consistent and green.

- [ ] **Step 1: Regenerate the spec** — `CARGO_TARGET_DIR=target-eo5 cargo run -q --bin egret -- openapi > openapi.json`. Confirm the new `cedar_grants_shadow` operationId and the `shadow` value on the `GrantSource` schema are present and distinct from pdp-source names.

- [ ] **Step 2: Regenerate the client** — `cd frontend && (ls node_modules/.bin/orval >/dev/null 2>&1 || npm ci) && npm run gen:api`.

- [ ] **Step 3: Frontend typecheck** — `cd frontend && npm run typecheck`. Expected: 0 errors (this is the gate the drift gates do NOT cover). Fix any duplicate-identifier clash via a distinct `operation_id`.

- [ ] **Step 4: Run the drift gates** — `CARGO_TARGET_DIR=target-eo5 cargo test -p egret-server --lib committed_openapi_json_is_current generated_client_was_emitted_from_the_committed_spec`. Expected: PASS.

- [ ] **Step 5: Fix default-baseline fallout.** Run the full egret-auth suite: `CARGO_TARGET_DIR=target-eo5 cargo test -p egret-auth`. For each failure that is a test asserting an *initial* switch state of `legacy`, change the expectation to `shadow` (the new seeded default) — but confirm the test's *decisions* are unchanged (shadow serves legacy). Do NOT weaken any decision assertion. Re-run the EO2/EO3/EO4 differentials (`cedar_differential`, `pdp_source_hot_flip`, `cedar_grant_hot_flip`) and confirm green.

- [ ] **Step 6: fmt + clippy** — `CARGO_TARGET_DIR=target-eo5 cargo fmt --all && CARGO_TARGET_DIR=target-eo5 cargo clippy -p egret-auth -p egret-server -- -D warnings`. Expected: clean.

- [ ] **Step 7: Commit** — stage `openapi.json`, `frontend/src/api/generated.digest.json`, the regenerated generated files, and the adjusted tests by explicit path; `git commit -m "chore: regenerate openapi/client and rebase test baselines to shadow default"`.

---

## Self-Review

- **Spec coverage:** EO5-D1 (observe-only) → Tasks 1–3 serve legacy, no serve-flip. EO5-D2 (seed + keep fail-safe) → Task 3 ON CONFLICT DO NOTHING; read_source untouched. EO5-D3 (EO4 shadow) → Tasks 1–2. EO5-D4 (fold-in-migration) → Task 3. EO5-D5 (one-level undo + /shadow) → Task 2. EO5-D6 (no deletion/vocab) → nothing deletes code; no vocab touched. Verification 1–6 → Tasks 2–4 tests.
- **Placeholder scan:** all steps carry concrete SQL/Rust or exact commands.
- **Type consistency:** `GrantSource::Shadow` defined in Task 1 and used in Tasks 2–4; `report()`/`GrantDivergence`/`AccessGrantStore` names flagged "confirm against grants.rs" where quoted from memory; `operation_id = "cedar_grants_shadow"` consistent between Task 2 and Task 4.
- **Owed outside this PR:** the `egret-data/docs` runbook PR (Global Constraints).
