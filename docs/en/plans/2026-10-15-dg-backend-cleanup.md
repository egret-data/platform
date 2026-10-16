# DG Backend Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the Data Governance backend items left after view governance (#674): catalog token refresh (I4), the
token rate limit (I3), the JWT per-user baseline, tag-typed tag services, the red `no_floating_images` test, and the
view-governance review minors N-1 to N-6.

**Architecture:** The machine IdP (`egret-machine-idp`) gains one verifier method (`current`) that both client
registries implement. A verification cache and an RFC 8693 token-exchange grant are built on it. The rest are local
fixes in `egret-engine-mgmt`, `egret-policy`, `egret-catalog-engine` and `egret-controlplane`. Two live runs bracket
the work: one measures what the JWT baseline needs, the last proves I3/I4 and the baseline on real engines.

**Tech Stack:** Rust (axum, sqlx, tokio, jsonwebtoken), Postgres, Trino 483 + Ranger plugin 2.5.0, StarRocks 4.1.6 +
plugin 2.9.0, React (only if a generated client changes).

**Spec:** `docs/en/specs/2026-10-15-dg-backend-cleanup-design.md` (decisions D1–D6).

## Global Constraints

- DB tests: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` only. Never DB `egret`. Never
  `EGRET_ALLOW_DB_SKIP=1`.
- Live runs: own `vg-` containers on network `vg-net`, a throwaway DB `egret_dgpkg_live` in `egret-dev-postgres-1`.
  Never start, stop or modify `egret-dev-*` containers or `egret-w7*-postgres`. Ports 5173, 5000, 8080 are not ours.
- Never `git stash`, `git checkout -- <file>`, `git clean`. Kill only PIDs you started. Never commit `.superpowers/`.
- Keep ≥ 30 GiB disk free (`df -h .`). This worktree's `target/` is its own.
- Per-crate tests during tasks (`cargo test -p <crate>`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
  `cargo fmt --all -- --check`). Full suites only at the final gate.
- Regenerate the API client with `cd frontend && npm run gen:api` only (never bare orval), after
  `cargo build -p egret-bin && target/debug/egret openapi > openapi.json`.
- Commits: Conventional Commits, identity hoangquocvietbk2.13@gmail.com, trailer
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not push.
- Migrations: none expected (D6). If one is needed, take the next free number after `0191` and check
  `git branch -a` worktrees for a collision first.
- Token TTLs are a published contract: catalog 3600 s, Data API 600 s. Do not change them.
- Docs: the guide `docs/en/guides/view-governance.md` has a Vietnamese twin `docs/vi/guides/view-governance.md`;
  change both together.

---

### Task 1: `no_floating_images` ignores a regex alternation

**Files:**
- Modify: `crates/egret-engine-mgmt/tests/no_floating_images.rs:42-67` (`literal_engine_refs`)

**Interfaces:** none.

- [ ] **Step 1: Write the failing tests** (in the same file, beside the existing unit tests of
  `literal_engine_refs`; if there are none, add a `#[test]` block):

```rust
#[test]
fn a_regex_alternation_is_not_an_image_ref() {
    let src = r#"if (/jdbc|sql|query|duckdb|postgres|trino|starrocks/.test(type)) return x;"#;
    assert_eq!(literal_engine_refs(src), Vec::<String>::new());
}

#[test]
fn an_unpinned_image_is_still_found() {
    let src = r#"const img = "starrocks/fe-ubuntu:latest";"#;
    assert_eq!(literal_engine_refs(src), vec!["starrocks/fe-ubuntu:latest".to_string()]);
}
```

- [ ] **Step 2: Run** `cargo test -p egret-engine-mgmt --test no_floating_images`. Expected: the new alternation test
  FAILS (`["starrocks/.test"]`), and `the_frontend_names_only_pinned_engine_images_and_displays_the_server_defaults`
  FAILS on `topologyModel.ts:94`.
- [ ] **Step 3: Implement.** In the loop, after `let mut start = from + off;`, skip a match whose next character
  after the needle is not ASCII alphanumeric:

```rust
let after = from + off + needle.len();
if !src[after..].starts_with(|c: char| c.is_ascii_alphanumeric()) {
    from = after;
    continue;
}
```

- [ ] **Step 4: Run** the same command. Expected: all pass.
- [ ] **Step 5: Commit** `test(engine-mgmt): a regex alternation is not an engine image ref`.

---

### Task 2: `ClientVerifier::current` and the `cgen` claim

**Files:**
- Modify: `crates/egret-machine-idp/src/lib.rs` (trait at `:49-59`, `VerifiedClient` at `:33-47`, minting at
  `:249-265`, test verifiers at `:405-480` and `:755-775`)
- Modify: `crates/egret-catalog-engine/src/store/embedded.rs:540-600` (`verify_client`, the `ClientVerifier` impl)
- Modify: `crates/egret-dataapi/src/clients.rs:92-180` (the `ClientVerifier` impl)
- Test: `crates/egret-catalog-engine/tests/` (the existing catalog-client DB test file; find it with
  `grep -rln verify_client crates/egret-catalog-engine/tests`), `crates/egret-dataapi/tests/` (the existing client
  DB tests; `grep -rln "ClientVerifier\|verify(" crates/egret-dataapi/tests`)

**Interfaces:**
- Produces (in `egret_machine_idp`):

```rust
/// A client's current minting state, read WITHOUT verifying a secret.
pub struct ClientState {
    /// Exactly what a successful `verify` would return now.
    pub verified: VerifiedClient,
    /// The stored secret hash (PHC string) as of this read.
    pub secret_hash: String,
}

#[async_trait::async_trait]
pub trait ClientVerifier: Send + Sync {
    async fn verify(&self, client_id: &str, client_secret: &str)
        -> Result<Option<VerifiedClient>, IdpError>;
    /// `None` when the client is unknown or may not mint for ANY reason the
    /// secret path would refuse (revoked, expired, disabled principal, archived
    /// workspace, …). Never runs Argon2.
    async fn current(&self, client_id: &str) -> Result<Option<ClientState>, IdpError>;
}

/// Digest of a stored secret hash, carried in every minted token as `cgen`.
/// First 16 hex chars of SHA-256 over the PHC string.
pub fn credential_generation(secret_hash: &str) -> String;
```

- `RejectAllVerifier::current` returns `Ok(None)`.

- [ ] **Step 1: Write failing tests.**
  - `egret-machine-idp` unit: `credential_generation("abc")` is 16 lowercase hex chars, stable, and differs for
    `"abd"`.
  - `egret-machine-idp` unit: a token minted by `token_endpoint` with a test verifier carries
    `cgen == credential_generation(<the verifier's hash>)`. To make this possible, minting takes `cgen` from
    `current`: after a successful `verify`, call `current(client_id)` on the same verifier and set
    `claims["cgen"]`. If `current` returns `None` right after a successful verify, answer 401 (the client changed
    under us).
  - catalog DB test: create a client, `current(id)` returns the same `VerifiedClient` fields as `verify(id, secret)`
    and its `secret_hash` equals the stored row; after the client is deleted, `current` is `None`.
  - dataapi DB test: `current` is `None` for each of: revoked, expired, disabled service-account user, archived
    workspace (reuse the setup helpers the existing verify tests use for those four cases); `Some` otherwise.
- [ ] **Step 2: Run** `cargo test -p egret-machine-idp`, `-p egret-catalog-engine`, `-p egret-dataapi`. Expected:
  compile errors / failures on the new method.
- [ ] **Step 3: Implement.**
  - Add `sha2.workspace = true` to `crates/egret-machine-idp/Cargo.toml`.
  - Catalog: split `verify_client` into a row read (`SELECT id, client_secret_hash, warehouse, role …`, the same
    query it runs today) and the Argon2 check; `current` returns the row as `ClientState` via
    `catalog_verified_client`.
  - Data API: factor the existing row query and every non-secret refusal (`revoked_at`, `expires_at`, `u.status`,
    `w.status`) into one function used by both `verify` and `current`, so the two cannot drift.
  - Every test verifier in `egret-machine-idp` (`AlwaysErrorsVerifier`, `FakeVerifier`, `CatalogLikeVerifier`,
    `CountingVerifier`) implements `current` consistently with its `verify`.
- [ ] **Step 4: Run** the three crates' tests + clippy. Expected: pass.
- [ ] **Step 5: Commit** `feat(machine-idp): read a client's current state without its secret`.

---

### Task 3: verification cache; the limiter counts only unproven requests (I3)

**Files:**
- Create: `crates/egret-machine-idp/src/verified_cache.rs`
- Modify: `crates/egret-machine-idp/src/lib.rs` (`IdpState` at `:73-84`, `token_endpoint` at `:195-265`)
- Modify: every `IdpState { … }` literal (`grep -rn "IdpState {" crates`), including
  `crates/egret-bin/src/identity.rs:119-131`

**Interfaces:**
- Consumes: `ClientVerifier::current`, `ClientState` (Task 2).
- Produces:

```rust
/// Successful verifications, keyed by (client_id, sha256(secret)), storing the
/// secret hash that verified. Bounded; entries expire.
#[derive(Clone)]
pub struct VerifiedCache { /* Arc<Mutex<...>> */ }
impl VerifiedCache {
    pub fn new(capacity: usize, ttl: std::time::Duration) -> Self;
    /// The hash that verified this (client_id, secret), if cached and unexpired.
    pub fn get(&self, client_id: &str, secret: &str) -> Option<String>;
    pub fn put(&self, client_id: &str, secret: &str, secret_hash: String);
}
impl Default for VerifiedCache { /* capacity 1024, ttl 600 s */ }
// IdpState gains: pub verified_cache: VerifiedCache
```

- The cache stores `sha256(secret)` (hex), never the secret.

- [ ] **Step 1: Write failing tests** (unit, in `lib.rs` tests with a `CountingVerifier` that counts `verify` calls
  and serves a fixed `secret_hash`):
  - `a_repeat_of_a_good_secret_skips_verify_and_the_limiter`: limiter `max = 2`; 10 requests with the same good
    secret all return 200; `verify` ran exactly once.
  - `a_wrong_secret_is_still_limited`: limiter `max = 2`; 3 requests with a wrong secret: 401, 401, 429.
  - `a_rotated_secret_misses_the_cache`: after one good request, the verifier's `current` starts returning a
    different `secret_hash`; the next request runs `verify` again (count 2).
  - `a_client_gone_is_refused_at_once`: after one good request, `current` returns `None`; the next request with the
    cached secret answers 401 and `verify` runs again only through the normal path.
  - `verified_cache` unit: TTL expiry (use a test-only clock or a 1 ms TTL + `std::thread::sleep(5ms)`), and the
    capacity bound (insert capacity + 1 distinct keys → `len() <= capacity`).
- [ ] **Step 2: Run** `cargo test -p egret-machine-idp`. Expected: fail to compile, then fail.
- [ ] **Step 3: Implement** in `token_endpoint`, for `client_credentials`:
  1. if `st.verified_cache.get(client_id, secret)` is `Some(h)`: for each verifier, `current(client_id)`; the first
     `Some(state)` with `state.secret_hash == h` → mint from `state.verified` (with `cgen`) and return. Errors from a
     verifier here fall through to step 2 (log them).
  2. otherwise the existing path unchanged: limiter check, verifier loop, mint. On success, read `current` (Task 2)
     and `put` the cache with its `secret_hash`.
  - Wire `VerifiedCache::default()` in `identity.rs` and every other `IdpState` literal.
  - Update the `RateLimitConfig::token_max_attempts` doc (`crates/egret-config/src/lib.rs:16-28`): the limit now
    counts requests that are not a proven repeat.
- [ ] **Step 4: Run** `cargo test -p egret-machine-idp -p egret-bin` + clippy. Expected: pass.
- [ ] **Step 5: Commit** `fix(machine-idp): a proven repeat skips Argon2 and the token limit (I3)`.

---

### Task 4: the token-exchange grant (I4)

**Files:**
- Modify: `crates/egret-machine-idp/src/lib.rs` (`TokenForm` at `:149-153`, `token_endpoint`, discovery at
  `:125-135`, `IdpError` mapping at `:268-280`, the `#[utoipa::path]` on the token endpoint at `:182-194`)
- Modify: `crates/egret-machine-idp/src/error.rs` (add `InvalidGrant`)
- Modify: `crates/egret-machine-idp/src/token.rs` (a validating decode)
- Regenerate: `openapi.json`, frontend client (`npm run gen:api`)

**Interfaces:**
- Consumes: `ClientVerifier::current`, `credential_generation` (Task 2).
- Produces:

```rust
pub const TOKEN_EXCHANGE_GRANT: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
pub const ACCESS_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:access_token";

#[derive(Deserialize, utoipa::ToSchema)]
struct TokenForm {
    grant_type: String,
    client_id: Option<String>,
    client_secret: Option<String>,
    subject_token: Option<String>,
    subject_token_type: Option<String>,
    scope: Option<String>, // accepted, ignored
}

// token.rs
/// Verify `token` against any of `jwks` (by `kid`), require `iss == issuer` and an
/// unexpired `exp`. Audience is NOT validated here; the caller checks it.
pub fn verify_issued(token: &str, jwks: &[serde_json::Value], issuer: &str)
    -> Result<serde_json::Value, IdpError>;
```

- [ ] **Step 1: Write failing tests** (unit, `lib.rs` tests; mint a real subject token through the endpoint first):
  - `a_live_token_exchanges_for_a_fresh_one`: exchange answers 200; new token has the same `sub`, `aud`, and the
    verifier's current extra claims; `exp` later than the subject's.
  - `the_bearer_header_alone_is_not_enough`: form without `subject_token` but with `Authorization: Bearer <tok>`
    → 400 `invalid_request` (the form field is required; a header that differs from the field → 400 too).
  - `an_expired_subject_token_is_invalid_grant`: subject token signed with `exp` in the past → 400 `invalid_grant`.
  - `a_foreign_signature_is_invalid_grant`: token signed by a key not in the JWKS → 400 `invalid_grant`.
  - `a_wrong_issuer_is_invalid_grant`.
  - `a_rotated_secret_ends_the_chain`: verifier's `current` returns a different `secret_hash` → 400
    `invalid_grant`.
  - `a_deleted_client_ends_the_chain`: `current` → `None` → 400 `invalid_grant`.
  - `the_audience_cannot_change`: subject `aud = "catalog"`, only a `data-api` verifier recognises the `sub` → 400
    `invalid_grant`.
  - `a_wrong_subject_token_type_is_invalid_request`.
  - `discovery_lists_both_grants`.
  - `client_credentials_without_client_id_is_invalid_request_not_422`.
- [ ] **Step 2: Run** `cargo test -p egret-machine-idp`. Expected: failures.
- [ ] **Step 3: Implement.**
  - `client_credentials`: missing `client_id`/`client_secret` → 400 `invalid_request`; else Task 3's path.
  - `TOKEN_EXCHANGE_GRANT`: limiter keyed `ip:<sub>` after decoding (`ip:exchange` before a token decodes, so junk
    floods are limited); `verify_issued` with `st.keys.all_public_jwks()` and `st.issuer`; find the verifier whose
    `current(sub)` is `Some` **and** whose `verified.audience == aud`; require `claims["cgen"] ==
    credential_generation(state.secret_hash)`; mint from `state.verified` (Task 2's mint, including `cgen`).
  - Anything else → `unsupported_grant_type`.
  - Discovery: `"grant_types_supported": ["client_credentials", TOKEN_EXCHANGE_GRANT]`.
  - OpenAPI: document the form fields and the 400 bodies.
  - Rebuild `egret`, re-emit `openapi.json`, run `npm run gen:api`.
- [ ] **Step 4: Run** `cargo test -p egret-machine-idp -p egret-server --lib` (the latter includes
  `committed_openapi_json_is_current` and the client-digest test) + clippy. Expected: pass.
- [ ] **Step 5: Commit** `feat(machine-idp): refresh a token by RFC 8693 exchange (I4)`.

---

### Task 5: tag services are `tag`-typed (D)

**Files:**
- Modify: `crates/egret-policy/src/admin/mod.rs:826-831` (`create_profile` type check, its utoipa doc)
- Modify: `crates/egret-policy/src/store.rs:812-830` (`create_service`) and add
  `refuse_tag_service_naming_a_non_tag_profile`
- Test: `crates/egret-policy/tests/store_services.rs` (DB), the admin route tests
  (`grep -rln create_profile crates/egret-policy/tests`)
- Docs: `docs/en/guides/view-governance.md` + vi twin (the tag-service known limit section, if present; otherwise
  the section that explains tag services)

**Interfaces:**
- Produces: `POST /policies/services` accepts `type` ∈ {`trino`, `starrocks`, `tag`}.

- [ ] **Step 1: Write failing tests.**
  - route: creating `{type: "tag"}` → 200, row type `tag`.
  - route: `{type: "hive"}` → 400 naming the three allowed types.
  - store: `create_service("p", "trino", {"tagService": "t"})` where `t` is a `trino` profile → 400
    `BadRequest` whose message says the tag service must be a `tag` profile.
  - store: same with `t` missing → 400.
  - store: same with `t` of type `tag` → Ok.
  - store: an existing (raw-SQL-inserted) trino row naming a trino tag service still downloads and its bump still
    cascades (guard that D4 does not touch existing rows: reuse the 0191 cascade test helper and assert it passes
    unchanged).
- [ ] **Step 2: Run** `cargo test -p egret-policy`. Expected: the new tests fail; note any EXISTING test that now
  fails because it creates a `trino` profile naming a non-tag or missing `tagService` — list them in the report.
- [ ] **Step 3: Implement.** In `create_service`, after the two existing refusals, when `config ->> tagService` is a
  string, `SELECT type FROM policy.authz_services WHERE name = $1`; refuse unless it is `Some("tag")`. Fix existing
  tests that relied on the old behaviour by creating a `tag` profile first (do not weaken the rule). Update the
  admin type check and its error text.
- [ ] **Step 4: Run** `cargo test -p egret-policy -p egret-engine-mgmt -p egret-perm-compile` (bind paths create
  tag services) + clippy. Expected: pass.
- [ ] **Step 5: Commit** `fix(policy): a tagService must name a tag profile`.

---

### Task 6: live measurement — the JWT baseline and the open StarRocks cases

No product code. This feeds Task 7 and Task 9.

**Files:**
- Modify: `docs/en/plans/2026-10-13-view-governance-live-result.md` — append `## 4. DG backend package: measurements
  (Task 6)` with `### 4.0 Predictions` committed BEFORE observing.

Build the stack exactly like §2.2 / §3 (Task 11 and Task 13 reports are in the SDD workspace `inputs/`): own `vg-`
containers, image `egret-backend:vg-dg6` from this branch's HEAD, throwaway DB `egret_dgpkg_live`, labelled
deviations (DEV-n). Keep `EGRET_RATE_LIMIT__TOKEN_MAX_ATTEMPTS` at its default; if I3 blocks the measurement before
Task 3 lands in the image, the image includes Task 3 (it is built from HEAD).

- [ ] **Step 1:** Commit predictions.
- [ ] **Step 2 — JWT baseline:** a non-admin user (`vcplain`) with a select grant on one table runs `SELECT` through
  `POST /api/ws/default/queries` on the JWT engine, with **no** hand-written baselines (no `query_max_run_time`
  allow, no `trinouser`, no `queryid`). Record the exact error. Add one hand-written allow at a time
  (`query_max_run_time {USER} alter` first, then `queryid * execute {USER}` if still denied, then `trinouser`
  self-impersonate) until the query succeeds. Record the minimal set and each step's error text.
- [ ] **Step 3 — StarRocks:**
  - (a) a hand-written `{public}` `MASK_NULL` on a column plus a hand-written deny for the reader on that column;
    reader holds a table select grant. Engine result vs explain.
  - (b) a table where every column is `MASK_NULL` for a reader with NO grant: `SELECT count(*)` and `SELECT *`.
    Does the reader learn the row count?
  - (c) an MV over a matched MV: read `information_schema.materialized_views` (or the base-map column egret's
    view-exposure reads) and record whether the base is spelled `default_catalog.<db>.<name>` or `<db>.<name>`.
- [ ] **Step 4:** Tear down; list what is left. Commit `docs(dg): live measurements for the backend package`.

Report: `task-6-report.md` with the minimal JWT set (exact resources/accesses/users) and the three StarRocks answers.

---

### Task 6a: the JWKS is never empty (J-F1) and engine auth failures say why (J-F2)

Found by Task 6 (live-result §4): egret creates its signing key only at the first token mint, and
`/.well-known/jwks.json` serves only stored keys. An engine that reads the JWKS first caches an empty set and rejects
every delegated query with 401 for minutes. Separately, every engine 401/403 becomes `"forbidden"` with no logged
reason, so a login failure looks like an access denial.

**Files:**
- Modify: `crates/egret-machine-idp/src/lib.rs` (`PgSigningKeyStore::all_public_jwks` ~`:581`, and the JWKS handler)
- Modify: `crates/egret-query/src/execution/error.rs` (`EngineError::Forbidden` mapping ~`:12`, `:46`) and the
  connector code that maps engine 401/403 (`grep -rn "Forbidden" crates/egret-query/src/execution`)
- Test: machine-idp unit (fake store) + a DB test on `egret_test`; egret-query unit

- [ ] **Step 1: failing tests:**
  - DB: on a store with no key rows, `all_public_jwks()` returns exactly one key, and `active()` afterwards returns
    that same `kid` (no second key). Concurrent first calls (two tasks) still end with one active key.
  - HTTP: `GET /.well-known/jwks.json` on a fresh store answers one key.
  - egret-query: an engine 401 is distinguishable from a 403 in the error the API returns (e.g. `"engine
    authentication failed"` vs `"forbidden"`), and the 401 path logs the engine's reason at `warn` without any token.
- [ ] **Step 2: run** → fail.
- [ ] **Step 3: implement:** `all_public_jwks` calls `active()` first when no non-retired key exists (reuse
  `active()`'s creation path; do not duplicate it). Map engine 401 to its own variant/message; keep 403 as
  `forbidden`. Never log a token.
- [ ] **Step 4: run** `cargo test -p egret-machine-idp -p egret-query` (+ `-p egret-server --lib` if the OpenAPI
  changes) + clippy.
- [ ] **Step 5: commit** `fix(machine-idp): never serve an empty JWKS; tell engine login failures apart`.

### Task 6b: re-run the JWT baseline measurement

Task 6's step 2, on an image that contains Task 6a. Same stack recipe, same constraints, predictions first
(`### 4.x` in the live-result doc). Run J0–J3 and the minimality check; record the exact minimal set. Report
`task-6b-report.md`. Commit `docs(dg): the JWT per-user baseline, measured`.

---

### Task 7: seed the JWT per-user baseline

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs:722-760` (provision hook), `:2349-2380`
  (`seed_delegator_policies` → split)
- Modify: `crates/egret-engine-mgmt/src/delegator_policy.rs` (new policy constructors, if Task 6 needs more than
  `query_timeout_policy`)
- Modify: `crates/egret-policy-api/src/infra_names.rs` (new stable names, if any)
- Test: the engines provision tests (`grep -rn "is_impersonate_delegation\|seed_delegator" crates/egret-engine-mgmt`)

**Interfaces:**
- Consumes: Task 6's minimal set (read `task-6-report.md`).
- Produces: `async fn seed_user_baseline(st: &CpState, service_name: &str) -> Result<(), CpError>` seeding the
  per-user policies (at least `QUERY_TIMEOUT_POLICY_NAME`) for every delegating Trino engine;
  `seed_delegator_policies` keeps only the delegator impersonate policy and runs only for impersonate engines.

- [ ] **Step 1: Write failing tests:** provisioning a JWT engine with auth seeds every policy in Task 6's set (assert
  by name and body); an impersonate engine still gets the delegator impersonate policy and the same per-user set;
  seeding twice creates no duplicates.
- [ ] **Step 2: Run** `cargo test -p egret-engine-mgmt`. Expected: the JWT test fails.
- [ ] **Step 3: Implement** the split. Call `seed_user_baseline` when `engine.supports_delegation()` (both modes),
  next to `seed_admin_all_access`. Any new name goes into `infra_names` so the guard and backfill see it.
- [ ] **Step 4: Run** `cargo test -p egret-engine-mgmt -p egret-policy` + clippy. Expected: pass.
- [ ] **Step 5: Commit** `fix(engine-mgmt): seed the per-user query baseline on JWT engines`.

---

### Task 8: view-hold minors N-1, N-5, N-6 and the cancellation tests

**Files:**
- Modify: `crates/egret-catalog-engine/src/view_hold.rs` (`best_effort` ~`:1344-1420`, `uncancellable`, the overflow
  `record_failed_if_free` spawn)
- Test: `crates/egret-catalog-engine/tests/` (the existing view-hold DB tests; `grep -rln uncancellable\|best_effort
  crates/egret-catalog-engine/tests`)

**Interfaces:** internal only. Read `inputs/final-rereview.md` N-1, N-5, N-6 and "Test gaps".

- [ ] **Step 1: Write failing tests.**
  - N-1: on an unclassified profile, drop the request future right after lakekeeper's commit (use the same
    injection seam the I-2 create test uses); the lineage record still lands.
  - N-6: with the best-effort cap at 1 and the view-tag lock held by the test, fire 10 over-cap commits; at most
    `cap` record tasks hold a pool connection at once (count with the seam the I-1 burst test uses), and every
    commit returns in < 1.2 s.
  - Cancellation for rename and for drop on a classified profile (mirror the I-2 create test).
- [ ] **Step 2: Run** `cargo test -p egret-catalog-engine`. Expected: N-1 and N-6 fail; rename/drop may already pass
  (they share the helper) — say so in the report, do not invent a failure.
- [ ] **Step 3: Implement.** Wrap the post-permit part of `best_effort` in `uncancellable`. Make `uncancellable`
  spawn `fut.in_current_span()` (N-5; `use tracing::Instrument`). Bound overflow record tasks: try a permit from the
  best-effort semaphore with no wait; if none, skip the record (the 15-minute sweep covers it) and log at debug.
- [ ] **Step 4: Run** the crate tests + clippy. Expected: pass.
- [ ] **Step 5: Commit** `fix(view-hold): best-effort survives cancellation; bound overflow records`.

---

### Task 9: N-2 refold scope, N-3 base-map spelling, N-4 wording

**Files:**
- Modify: `crates/egret-policy/src/admin/mod.rs` (`changes_the_clearance_fold`; `grep -n changes_the_clearance_fold`)
- Modify: `crates/egret-controlplane/src/view_exposure.rs` (`reads_a_matched_view`)
- Modify: `docs/en/guides/view-governance.md`, `docs/vi/guides/view-governance.md`,
  `frontend/src/routes/governance/ExplainAccess.tsx` (+ its test) for N-4
- Test: the I-3 refold test in `crates/egret-perm-compile/tests/` (`grep -rln refold\|clearance_fold
  crates/egret-perm-compile/tests`), view-exposure tests in `crates/egret-controlplane/tests/view_exposure.rs`

**Interfaces:**
- Consumes: Task 6 answers (a) and (c).

- [ ] **Step 1: Write failing tests.**
  - N-2: on a StarRocks profile with NO compiled clearance mask whose recompile is refused (seed the refusal the same
    way the existing I-3 test seeds a profile), disabling and deleting a hand-written mask succeed; on a profile WITH
    a compiled clearance mask the refold still runs (existing test stays green).
  - N-3: an MV whose base map names the matched MV as `<db>.<name>` is listed (and still as
    `default_catalog.<db>.<name>`).
- [ ] **Step 2: Run** `cargo test -p egret-perm-compile -p egret-controlplane -p egret-policy`. Expected: fail.
- [ ] **Step 3: Implement.** N-2: refold only when the profile has a compiled clearance mask row or
  `origin = 'generated'`. N-3: accept both spellings. N-4: if Task 6 (a) showed NULL, state the hand-written skip as
  measured; if it showed a refusal, narrow the rule back to the clearance's own mask in explain (`routes.rs`
  `masked_read_skips_access`) with a test, and say so in the report. Update the "every column `MASK_NULL`" limit with
  Task 6 (b)'s answer.
- [ ] **Step 4: Run** the three crates + `cd frontend && npx vitest run src/routes/governance/ExplainAccess.test.tsx`,
  `npm run lint`, `npx tsc -b`. Expected: pass.
- [ ] **Step 5: Commit** `fix(view-governance): narrow the refold, accept both base-map spellings, measured wording`.

---

### Task 10: final live verification — I3, I4, the seeded baseline

**Files:**
- Modify: the live-result doc: `### 4.5 Verification (Task 10)` with predictions committed first.

Same stack recipe as Task 6, image from HEAD (`egret-backend:vg-dg10`), DEV-5 NOT applied (default token limit).

- [ ] **Step 1:** Commit predictions.
- [ ] **Step 2 — I3:** 12 concurrent `CREATE VIEW` through Trino on a classified profile: no `429` in the proxy log,
  holds behave as §3 (≤ 8 held, the rest refused with the cap message or succeed).
- [ ] **Step 3 — I4:** run with a short catalog token TTL for this run only (DEV-n: if TTL is not configurable,
  wait for Trino's refresh after the real 3600 s or use a test-only override and label it). Observe Trino's
  `token-exchange` request answered `200`, no `422`, no retry storm.
- [ ] **Step 4 — baseline:** a fresh JWT engine provisioned through egret's path (DEV-1 rows as before) has the
  seeded set; `vcplain` queries succeed with no hand-written baselines.
- [ ] **Step 5:** Tear down. Commit `docs(dg): live verification of the backend package`.

---

## Final gate (controller)

Whole-branch review on the most capable model, one fix dispatch + scoped re-review, then on a branch rebased on fresh
main: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`;
`just test-shipping-resolution`; `cargo test --workspace --no-fail-fast` with `DATABASE_URL=…/egret_test` and
`EGRET_TEST_SHARED_STORE_URL=redis://127.0.0.1:6399` (no skip opt-outs except engine/docker as `just check` sets);
`just test-ranger-gates`; frontend gate (`npm run lint`, `npx tsc -b`, `npx vitest run`, `npm run build`, bundle
tests) because the generated client changes.
