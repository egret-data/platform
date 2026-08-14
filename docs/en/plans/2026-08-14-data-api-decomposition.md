# Data API — plan decomposition

> Spec: `docs/en/specs/2026-08-14-data-api-design.md`
> Date: 2026-08-14

The spec covers five subsystems that each produce working, testable software on their own. Writing them as one plan would produce a document too large to execute task-by-task and would couple a security fix that should ship today to a feature that will not land for weeks.

Each part below is a separate plan, merged as a stack in order. When squash-merging the stack, never `--delete-branch` while a child PR still targets the branch — that closes the child unreopenably. After each squash, `git fetch` then `git rebase --onto origin/main <old-parent-tip> <child>`.

## P1 — Dialect-safe parameter escaping · **plan written**

`docs/en/plans/2026-08-14-data-api-p1-dialect-escaping.md`

Spec §3. `egret-query` only. Moves literal escaping into `execution/dialect.rs` keyed on `Engine` and threads the engine through `template::render`.

Ships alone and should merge first regardless of what happens to the rest of Data API: the hole is open in SQL Lab today. Depends on nothing.

## P2 — Service accounts and API keys

Spec §5 (identity portion), §6.1.

- Extend `users.provider CHECK` to `('local','oidc','service')`; synthetic email `svc.<slug>@<ws-slug>.service.egret.local`.
- Migration `0119_dataapi.sql`: `CREATE SCHEMA dataapi` + `dataapi.api_keys`.
- Key mint / verify / revoke: `egt_<prefix>_<secret>`, SHA-256 over a 32-byte random secret, constant-time compare, secret shown once.
- Management endpoints for creating and revoking service accounts and keys, behind Cedar.

Depends on nothing. Deliverable: an admin can create a service account, mint a key, and the key verifies — with no endpoint to call yet.

**The risky part to get right:** service accounts must be reachable by `access_grants`, Ranger and machine-idp exactly as users are. The test that matters is granting one through the existing `access_grants` path and confirming `grants_for` returns it, not a unit test of the key hash.

## P3 — Endpoint definitions, versioning, management API

Spec §4.1–4.4, §5.1–5.3.

- Crates `egret-dataapi-api` (DTOs) and `egret-dataapi` (seam).
- `QueryExecPort` in `egret-query-api` + its `egret-query` implementation, and the module-doc correction that crate's §4.3 note calls for.
- `dataapi.endpoints`, `dataapi.endpoint_versions`; draft CRUD, validate, publish, rollback, disable.
- `management_router()` under `/api`, Cedar-gated. Audit events for publish / rollback.

Depends on P1 (validate renders) and P2 (ownership, authz).

**The risky part to get right:** the §5.3 invariant that serving reads only frozen snapshots. It is cheap to violate accidentally and the test for it (publish, edit draft, call, expect old behaviour) must be written before the serving path exists in P4, then re-asserted through the real route in P4.

## P4 — Serving surface

Spec §6.2–6.4, §7, §8.

- `serving_router()` under `/data-api/v1`, no cookie, no CSRF.
- The ten-step request chain, including 404-not-403 for a cross-workspace key.
- Principal-scoped response cache; `shared` rejected at validate time on delegated connections.
- `TokenBucket` behind `RateLimitStore`, `check_at(key, now)` with an injected clock.
- Error envelope and code table; serving-path timeout; `dataapi.access_log` with monthly partitioning and a gc worker.

Depends on P1, P2, P3.

**Read this before writing a single route:** the stored `path` is canonical — exactly one optional leading slash, no trailing slash, and every segment matching `[A-Za-z0-9._~-]+`. `egret_dataapi::endpoints::normalize_path` is the one function that decides this, and **P4 must call it on the INBOUND path too**. axum percent-decodes a `{*path}` wildcard capture, so an inbound path arrives decoded while the stored one was never encoded — a `WHERE path = $1` written without normalising will miss, and the endpoint becomes permanently unreachable. An inbound path that fails the rule cannot match any stored row, so it is a 404, not a 400.

**The risky part to get right:** two things.

First, the row cap. The connector *truncates* at the cap rather than failing — but it already signals this: `Connector::execute` fetches `max_rows + 1` and, if the extra row appears, sets `TabularResult.truncated` and drops it (`crates/egret-query/src/execution/connector/mod.rs:63`). Spec §7 requires Data API to return `400 row_limit_exceeded` instead of serving truncated data, so the serving path checks `truncated` — it must not reimplement the `max_rows + 1` trick, and must not infer overflow by comparing `row_count` to the cap, which cannot distinguish "exactly at the cap" from "over it".

Second, the cache leak test needs a real Ranger with two service accounts holding different grants. A mocked policy decision would pin the consumer, not the server, and prove nothing.

## P5 — Management UI, OpenAPI, Try-it

Spec §2 (v1 scope), §8 statistics.

- Endpoint list / editor / publish + version history, following the canonical Explore list-page scaffold.
- Per-endpoint generated OpenAPI plus a Try-it console.
- Statistics from `access_log`: calls, error rate, p50/p95, cache hit rate.
- The §5.3 warning that publishing does not grant data access, surfaced where the user publishes.

Depends on P3, P4.

**The risky part to get right:** Explore and control-plane use different header and button idioms — read the canonical Explore list-page scaffold before building the pages, and do not apply generic design-system landing-page advice here. Verify in the browser, not only by test: on this codebase every UI defect that mattered was found live.
