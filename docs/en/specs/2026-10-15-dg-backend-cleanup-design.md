# DG backend cleanup after view governance: design

Date: 2026-10-15. Status: approved (user, 2026-10-15). Base: main `58831374` (#674).

This package closes the Data Governance backend items left open by view governance (#674) and the tag-def fix
(#673), before the DG frontend redesign. Sources:

- live findings I3, I4: `docs/en/plans/2026-10-13-view-governance-live-result.md` §2.6;
- the JWT finding: Task 12 report, "I2 investigation" (copied to the SDD workspace `inputs/task-12-report.md`);
- review minors N-1 to N-6 and the test gap: view-governance final re-review (`inputs/final-rereview.md`);
- open live rechecks: live-result §3 and the #674 PR body.

## Decisions

| id | decision | by |
|---|---|---|
| D1 | `/oauth/token` accepts the RFC 8693 token-exchange grant; an exchange is as strong as `client_credentials` (valid while the client exists and its secret is unrotated), no chain-age cap | user, 2026-10-15 |
| D2 | I3 is fixed by caching successful secret verifications, not by raising the limit | user, 2026-10-15 |
| D3 | The JWT baseline is seeded from what a live Trino 483 measurement shows is needed, minimum first | default decision (2026-10-15) |
| D4 | A `tagService` must name a `tag`-typed profile on create; existing rows are not migrated | default decision (2026-10-15) |
| D5 | `no_floating_images` is fixed in the scanner, not by rewriting the frontend regex | default decision (2026-10-15) |
| D6 | No migration is expected; if one becomes necessary it takes the next free number after checking open branches | default decision (2026-10-15) |

## A. I4: token refresh by token exchange

**Defect (measured, live §2.6 I4):** about an hour after start, Trino's Iceberg client refreshes its catalog token
with `grant_type=urn:ietf:params:oauth:grant-type:token-exchange` and no `client_id`. egret answers `422 Failed to
deserialize form body: missing field 'client_id'` because `TokenForm` (`crates/egret-machine-idp/src/lib.rs:149`)
requires `client_id`/`client_secret` and the handler accepts `client_credentials` only (`:215`). Trino retries in a
storm, which feeds I3.

**Design:**

- `TokenForm` becomes grant-shaped: `client_credentials` keeps requiring `client_id` + `client_secret`; the exchange
  grant requires `subject_token` and accepts `subject_token_type` (must be
  `urn:ietf:params:oauth:token-type:access_token` when present), `scope` (ignored), and an optional
  `Authorization: Bearer` header (if both are present they must be the same token).
- An exchange succeeds only when every check passes:
  1. the subject token verifies against egret's own signing keys (active or still-published), `iss` is egret's
     issuer, `exp` is in the future;
  2. its `aud` is one of the audiences egret's verifiers mint (catalog, data-api), and the new token keeps exactly
     that `aud` (no audience change);
  3. the client named by `sub` still exists in the verifier that owns that audience, and its secret has not been
     rotated since the subject token was minted. Tokens carry a new claim `cgen` (a short digest of the stored
     secret hash at mint time); the exchange compares it with the current one. This lookup does not run Argon2.
- The new token is minted from the client's **current** state, exactly as `client_credentials` would mint it
  (current extra claims, current `cgen`, the verifier's TTL). Check 3 runs every revocation condition the verifier
  applies to a secret login: for Data API clients that includes `revoked_at`, `expires_at`, the service account's
  user status and the workspace status (`crates/egret-dataapi/src/clients.rs:92-140`). Only the Argon2 step is
  skipped.
- Both verifiers gain one trait method that returns the client's current state without verifying a secret:
  `ClientVerifier::current(client_id) -> Result<Option<ClientState>, IdpError>`, where
  `ClientState { verified: VerifiedClient, secret_hash: String }` and `None` means "may not mint" for any reason.
  Section B uses the same method.
- Rate limiting (amended by the final review, F3): only refusals are counted, on the same limiter. A subject token
  that fails verification or names no client (`sub`/`aud`/`cgen` missing) counts against the IP's `ip:exchange`
  bucket; one that verifies but does not match its client (gone, other audience, `cgen` mismatch) counts against
  `ip:sub`. An exchange proven against the client's current state is not counted, as a proven repeat of a good secret
  is not counted on `client_credentials` — counting it spent the `ip:client_id` bucket a refresh burst shares with
  `client_credentials` and locked the client out of both for the lockout window. A registry outage is not counted.
- Failures answer OAuth errors (`invalid_grant` 400 for a bad/expired/revoked subject token,
  `unsupported_grant_type` 400 otherwise), never 422.
- Discovery's `grant_types_supported` lists both grants. OpenAPI documents the new form.

**Revocation:** deleting a client or rotating its secret stops further exchanges at once. A token already issued
stays valid until its `exp` (unchanged from today).

## B. I3: verification cache

**Defect (measured, live §2.6 I3):** Trino's Iceberg client takes a fresh token per query session (134
`POST /oauth/token` for 12 concurrent statements). The limiter (60 per minute per `ip:client_id`,
`crates/egret-config/src/lib.rs:16-28`) answers 429 and every statement fails inside Trino. The limit exists to bound
Argon2 CPU, not to stop guessing (secrets are 256-bit random); its own doc says so.

**Design:**

- A process-local cache of successful verifications keyed by `(client_id, sha256(presented secret))`, storing the
  secret hash that verified. On a request whose pair is cached, the IdP calls `current(client_id)` (no Argon2); if
  the client may still mint and its `secret_hash` equals the cached one, it mints from that state.
- Everything else (no cache entry, a changed hash, `current` = `None`) goes the old way: limiter first, then
  `verify`. So unknown ids and wrong secrets stay limited, and only proven-good repeats skip the limiter and
  Argon2. On a successful `verify`, the cache is filled from `current(client_id)`.
- Rotation changes the stored hash, so the next request misses. A deleted client has no row and fails at once.
  The cache never answers "valid" for a client whose row is gone.
- Bounded size (LRU or capped map) and a TTL (default 10 minutes) so the cache cannot grow without limit.
- Applies to both verifiers (catalog clients and Data API clients) through the shared token endpoint.
- The presented secret is never stored, only its SHA-256.

## C. JWT engines: the per-user query baseline

**Defect (code-read, Task 12 report):** egret's Trino connector sends `X-Trino-Session: query_max_run_time=<n>s` on
every statement (`egret-query/src/execution/connector/trino.rs`, execute / describe / prepared). With JWT
delegation the session user is the end user. The only allow for that property, `egret-query-timeout`
(`{USER}`, `alter`, `systemproperty=query_max_run_time`), is seeded only for impersonate engines
(`crates/egret-engine-mgmt/src/engines.rs:730`). JWT engines get only `seed_admin_all_access`, so non-admin users are
very likely denied every statement. Trino 483 may also check `queryid` execute and self-impersonation; unmeasured.

**Design:**

- First, measure on a live JWT Trino 483 (the Task 11 stack): a non-admin user with a table grant runs a query through
  egret's query API with no hand-written baselines. Record each denial. Add one baseline at a time until queries
  succeed. That gives the minimum set.
- Seed exactly that set for JWT engines on the same provision path, idempotent (`policy_exists` then
  `create_infra_policy`), under stable names. At minimum `egret-query-timeout`.
- Test: a provision of a JWT engine seeds the set; an impersonate engine's seeding is unchanged.

## D. Tag services are `tag`-typed

**Today:** `bind_trino_profile` creates `{name}_tag` with type `tag`. The admin API (`POST /policies/services`,
`crates/egret-policy/src/admin/mod.rs:829`) accepts `trino` or `starrocks` only, so a hand-made tag service is a
`trino` profile (finding I5; #673 made the cascade work for it).

**Design:**

- `POST /policies/services` also accepts `type: "tag"` (no `tagService` on it: 0176's CHECK already refuses that).
- `PolicyStore::create_service` refuses (400) a profile whose `tagService` names a profile that does not exist or is
  not `tag`-typed. This is the only `INSERT` path (store.rs's write-path table); no `UPDATE` of `config_jsonb`
  exists in `src/`.
- Existing rows that name a non-tag profile are left as they are. #673's cascade keeps them consistent. Document
  this in the guide.

## E. `no_floating_images` (red on main)

`crates/egret-engine-mgmt/tests/no_floating_images.rs::literal_engine_refs` reads the regex literal
`/…|trino|starrocks/.test(type)` in `frontend/src/components/workflows/topology/topologyModel.ts:94` as the image
ref `starrocks/.test`. Fix: a match counts only when the character after the needle starts an image name
(ASCII alphanumeric). Add a unit test with that exact regex shape and one with a real unpinned ref.

## F. View-governance review minors

All in `crates/egret-catalog-engine/src/view_hold.rs` unless named (details in `inputs/final-rereview.md`):

- **N-1:** wrap the post-permit part of `best_effort` in `uncancellable`, so a client disconnect after
  lakekeeper's commit cannot skip `record_after_commit`. Test: drop the request future after the commit; the
  lineage record still lands.
- **N-2:** refold on hand-written StarRocks mask writes only when the profile has a compiled clearance mask (or is
  `generated`), so a disable or delete is not refused on a profile whose recompile fails for an unrelated reason.
  Test both sides.
- **N-3:** `reads_a_matched_view` (view exposure) also accepts the `<db>.<name>` base-map spelling. Live run settles
  which one StarRocks writes.
- **N-4:** guide (en + vi) and the explain page say the clearance's `MASK_NULL` skip was measured and other
  policies' `MASK_NULL` is assumed the same, until the live run in G measures it.
- **N-5:** `uncancellable` spawns with `.in_current_span()`.
- **N-6:** overflow `record_failed_if_free` tasks are bounded (share the best-effort semaphore with a short wait,
  or skip when none is free and leave it to the sweep). Test the bound.
- **Test gap:** I-2 cancellation tests for rename and drop.

## G. Live run (last task)

On a throwaway stack like Task 11/13 (own `vg-` containers, throwaway DB, never `egret` or `egret-dev-*` services):

1. I3: 12 concurrent `CREATE VIEW` through Trino with the default token limit (no DEV-5): no 429.
2. I4: force a refresh (short catalog token TTL for the run) and observe a `200` token exchange, no 422 storm.
3. C: the minimum baseline for a non-admin JWT user (this measurement feeds C; run it first, before C is coded,
   as its own task).
4. StarRocks: a hand-written `MASK_NULL` with a deny (N-4); a table where every column is `MASK_NULL` for the reader
   (does a no-grant reader learn the row count?); the MV-over-MV base-map spelling (N-3).
5. Update the guide and N-4 wording with what was measured.

## Out of scope

The DG frontend redesign (its own spec, brainstormed first). SP5. Two Ingress objects on a real ingress-nginx
(needs a kind cluster; stays a known limit).

## Testing and gate

TDD for every behaviour change. DB tests on `egret_test` only. Final gate: `cargo fmt --check`, workspace clippy
`-D warnings`, `just test-shipping-resolution`, `cargo test --workspace --no-fail-fast` with the shared store,
`just test-ranger-gates`, and the frontend gate if the frontend or generated client changes.
