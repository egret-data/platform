# Access Control P2a — Time-bound / expiring grants Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** "Just-in-time access" — let an admin grant access that **expires** at a chosen time, enforced in real time. First write surface is object sharing (the `ShareDialog`); expiry for workspace-member and role assignment is a documented follow-up (P2a-2).

**Architecture:** Grants are already re-queried from `access_grants` at **every** authorization decision via `AccessGrantStore::grants_for` (`crates/egret-auth/src/grants.rs:53`) — templates are per-role, but the *grants* linked to them are a live SQL read per `authorize` call (`pdp/mod.rs:212`). So expiry enforcement is a single SQL predicate — `AND (expires_at IS NULL OR expires_at > now())` — applied to every `access_grants` read that gates or enumerates access. No Cedar `now`/context machinery, no template change; real-time and correct. NULL `expires_at` = never expires (preserves all current behavior).

**Tech Stack:** Rust (sqlx, axum, utoipa) across `egret-auth`, `egret-bi`, `egret-workspace`; migration in `migrations/`; React 18 + shadcn (a date picker) + Vitest.

---

## Context (verified)

- **Enforcement chokepoint:** `AccessGrantStore::grants_for` (`egret-auth/src/grants.rs:47-68`) — the only grant read `CedarPdp::authorize` uses (`pdp/mod.rs:189,212`). Filtering here makes an expired grant produce no linked policy → no access.
- **Principal / FE-gating + `is_platform_admin` context:** `session.rs` has 2 `access_grants` reads (role names ~:136, role ids ~:158) that populate `Principal.platform_roles`/`permissions` (feed FE `/me` gating and the admin-graph `context.is_platform_admin`). Must filter expired too, else an expired `platform_admin` still reads as admin.
- **Enumerable "effective" views:** `effective.rs` — `grant_rows` (:87) for Access Explorer, and `who_can_access`'s three queries (P0). These are "what access is *effective*" → must exclude expired to match enforcement.
- **Admission / listing reads:** `crates/egret-workspace/src/rbac.rs` (object-grant / workspace admission) and `WorkspaceRepo::list_for_principal` (`workspaces.rs:111`, lists a user's workspaces from `access_grants`) — an expired workspace grant must not admit or list.
- **Object-share write:** `grant_put` (`egret-bi/src/shares.rs:200`, `INSERT INTO access_grants` :229) — the write path to accept `expires_at`.
- **Management roll-up (B1):** `AccessViewsRepo::list_object_grants` + `ObjectGrantRollupDto` (`egret-workspace/src/access_views.rs`, `routes.rs`) — the SharedObjects table; this is a *management* view, so it SHOWS all grants incl. expired, returning `expires_at` for display.
- **FE share client:** `ShareDialog.tsx` uses generated `sharesGrantPut` (`api/generated/shares/shares.ts`); the grant body is `ObjectGrantInput`/similar.
- **Migration numbering:** latest is `0077`; next is **`0078`**. Per the sqlx-staleness gotcha, `touch crates/egret-metastore/src/lib.rs` after adding the migration so `serve` re-embeds it.

---

## Non-Goals

- No expiry on **workspace-member add** or **role assignment** writes yet (enforcement already covers them since `grants_for`/`session` filter by `expires_at`; only the *write UI* is deferred → P2a-2).
- No auto-purge/sweep job (expired rows stay in the table, harmless — enforcement excludes them; a janitor is a later optimization).
- No Cedar template/schema change; no `context.now`.
- No access-request/approval workflow (that's P2b, which will reuse this expiry column to grant time-boxed access on approval).

---

## File Structure

- Create `migrations/0078_access_grant_expires_at.sql`.
- Modify `crates/egret-metastore/src/lib.rs` (touch to force re-embed).
- Modify `crates/egret-auth/src/grants.rs` (grants_for), `session.rs` (2 reads), `effective.rs` (grant_rows + who_can_access queries).
- Modify `crates/egret-workspace/src/rbac.rs` (admission reads), `workspaces.rs` (list_for_principal), `access_views.rs` + `routes.rs` (roll-up returns `expires_at`).
- Modify `crates/egret-bi/src/shares.rs` (grant_put accepts `expires_at`).
- Regenerate `frontend/src/api/generated/**`; modify `frontend/src/components/sharing/ShareDialog.tsx` (expiry input), `frontend/src/routes/explore/settings/SharedObjects.tsx` + `api/workspaceAccess.ts` (Expires column).
- Tests alongside each crate + FE.

---

## Tasks

### Task 1: Migration + enforcement sweep (SECURITY — the core)

**Files:** `migrations/0078_access_grant_expires_at.sql`; `egret-metastore/src/lib.rs`; `egret-auth/src/{grants.rs,session.rs,effective.rs}`; `egret-workspace/src/{rbac.rs,workspaces.rs}`. Tests in the owning crates.

- [ ] **Step 1 — Migration.** `0078_access_grant_expires_at.sql`:
  ```sql
  ALTER TABLE access_grants ADD COLUMN expires_at timestamptz;
  -- NULL = never expires. Partial index to keep the "active" predicate cheap.
  CREATE INDEX access_grants_expires_idx ON access_grants (expires_at) WHERE expires_at IS NOT NULL;
  ```
  Then `touch crates/egret-metastore/src/lib.rs` (re-embed).
- [ ] **Step 2 — Failing enforcement test (egret-auth).** Extend the PDP DB tests (mirror `cedar_pdp.rs`/`pdp_explicit_deny.rs`): grant a principal a role on a resource with `expires_at = now() - 1 hour`; assert `authorize`/`require` DENIES. A second grant with `expires_at = now() + 1 hour` (or NULL) ALLOWS. Also: an expired `platform_admin` platform grant → `session` does NOT report `platform_admin` (assert via the session role read).
- [ ] **Step 3 — Run, confirm fail** (currently the expired grant still allows).
- [ ] **Step 4 — Apply the predicate.** Add `AND (ag.expires_at IS NULL OR ag.expires_at > now())` to EVERY `access_grants` read that GATES or ENUMERATES effective access:
  - `grants.rs::grants_for` (the enforcement chokepoint).
  - `session.rs` — both grant reads (role names + role ids).
  - `effective.rs` — `grant_rows` (both UNION arms), and `who_can_access`'s direct-object + workspace + platform-admin queries.
  - `rbac.rs` — any `access_grants` read used for object/workspace admission.
  - `workspaces.rs::list_for_principal` — the workspace-listing grant read.
  Do NOT touch the B1 roll-up `list_object_grants` here (management view — Task 2 keeps it showing all).
  **Grep to be exhaustive:** `grep -rn "FROM access_grants" crates/*/src` — for each hit, decide gating/effective (filter) vs management/audit (leave, but note). The B4 `audit.audit_log` is separate (not `access_grants`) — leave it.
- [ ] **Step 5 — Run, confirm pass** (expired denies; active/NULL allows; expired admin not admin). Also run each touched crate's existing suite to prove no regression (NULL rows behave exactly as before): `DATABASE_URL=… cargo test -p egret-auth` and `-p egret-workspace`.
- [ ] **Step 6 — fmt + clippy + commit.** `feat(authz): time-bound grants — expiry enforced at every access_grants decision (mig 0078)`.

### Task 2: Write path (`grant_put`) + roll-up returns `expires_at`

**Files:** `egret-bi/src/shares.rs`; `egret-workspace/src/access_views.rs` + `routes.rs`. Tests in both crates.

- [ ] **Step 1 — Failing test (egret-bi).** Extend a shares test: `grant_put` with an `expires_at` in the body persists it on the `access_grants` row; without it, the column is NULL. (Mirror `share_deny.rs`/`chart_object_scope.rs`.)
- [ ] **Step 2 — Run, confirm fail.**
- [ ] **Step 3 — Implement `grant_put`.** Add an optional `expires_at: Option<DateTime<Utc>>` to the request body struct; include it in the `INSERT INTO access_grants (… expires_at) VALUES (… $n)` and the `ON CONFLICT … DO UPDATE SET … expires_at = EXCLUDED.expires_at` (so re-sharing can extend/clear it). Keep the existing principal-resolution + effect logic unchanged.
- [ ] **Step 4 — Roll-up returns expiry.** In `access_views.rs::list_object_grants`: add `ag.expires_at` to the SELECT + `ObjectGrantRollup { … expires_at: Option<DateTime<Utc>> }`; in `routes.rs` add `expires_at: Option<String>` (RFC3339) to `ObjectGrantRollupDto` (+ `From`). The roll-up continues to return ALL object grants incl. expired (management view). Extend the B1 roll-up test to assert `expires_at` round-trips.
- [ ] **Step 5 — Run, confirm pass** (grant_put persists expiry; roll-up returns it). fmt + clippy.
- [ ] **Step 6 — Commit.** `feat(share): grant_put accepts expires_at; shares roll-up returns it`.

### Task 3: FE — ShareDialog expiry input (+ client regen)

**Files:** `frontend/src/api/generated/**` (regen); `frontend/src/components/sharing/ShareDialog.tsx` + its test.

- [ ] **Step 1 — Regen client** so `sharesGrantPut`'s body carries the optional `expires_at`. (Run the repo's orval/openapi build; if regen is heavy, instead extend the ShareDialog's grant-put call to pass `expires_at` through the existing generated body type if it's `additionalProperties`-permissive, or add the field to the hand path — prefer regen.)
- [ ] **Step 2 — Failing test.** In `ShareDialog.test.tsx`: setting an expiry date in the add-grant row and submitting calls the grant-put with the chosen `expires_at`; an existing grant with `expires_at` renders its expiry (e.g. "expires {date}"); clearing it sends null.
- [ ] **Step 3 — Run, confirm fail.**
- [ ] **Step 4 — Implement.** Add an optional "Access expires" control to the add-grant row and per-grant row (a native `<input type="date">` or the shadcn date/popover if one exists — check `frontend/src/components/ui/` for a `calendar`/`date-picker`; a plain `<input type="date">` bound to an ISO string is acceptable and keyboard-accessible). Convert the picked date to an RFC3339 timestamp (end-of-day in the user's tz is fine) and pass it to grant_put; show a grant's current expiry as muted text with a clear ("no expiry") option. Default = no expiry (unchanged behavior).
- [ ] **Step 5 — Run, confirm pass**; `npm run typecheck` + `npm run lint` clean.
- [ ] **Step 6 — Commit.** `feat(fe): ShareDialog — optional access-expiry per grant`.

### Task 4: FE — SharedObjects "Expires" column

**Files:** `frontend/src/api/workspaceAccess.ts` (type); `frontend/src/routes/explore/settings/SharedObjects.tsx` + its test.

- [ ] **Step 1 — Failing test.** With a roll-up row carrying `expires_at` in the future → an "expires {relative}" cell; a past `expires_at` → an "Expired" badge (destructive token); null → "—".
- [ ] **Step 2 — Run, confirm fail.**
- [ ] **Step 3 — Implement.** Add `expires_at: string | null` to `ObjectGrantRollup`; add an "Expires" column to the SharedObjects `FilterableTable` (future → `timeAgo`/date muted; past → an "Expired" `Badge` using the `--destructive` token from the shared `EffectBadge` module's styles; null → "—"). Optionally add an "Expired" facet value later (not required).
- [ ] **Step 4 — Run, confirm pass**; typecheck + lint.
- [ ] **Step 5 — Commit.** `feat(fe): shared-objects roll-up shows grant expiry`.

### Task 5: Full verification

- [ ] Backend: `cargo fmt --all --check`; `cargo clippy -p egret-auth -p egret-bi -p egret-workspace --all-targets`; `DATABASE_URL=… cargo test -p egret-auth -p egret-bi -p egret-workspace` (report counts; never two `--workspace` runs at once).
- [ ] Frontend: `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build`.
- [ ] Manual sanity (per the enforcement invariant): confirm the grep from Task 1 Step 4 left no gating `access_grants` read unfiltered, and that a NULL-expiry grant behaves exactly as before (all prior tests green = the regression guard).

---

## Self-Review Checklist

1. **Security completeness:** every gating/effective `access_grants` read filters `expires_at` (grants_for, session×2, effective grant_rows + who_can_access×3, rbac admission, list_for_principal). The grep in Task 1 Step 4 is the audit — record which reads were filtered and which (management/audit) were intentionally left.
2. **Behavior-preserving for NULL:** `expires_at IS NULL OR …` means every existing grant (all NULL after the migration) behaves identically — prior suites must stay green.
3. **Enforcement vs display consistency:** enforcement + effective/who-can-access exclude expired; only the management roll-up (SharedObjects) shows expired (with an "Expired" marker). No surface claims an expired grant is active.
4. **Type consistency:** BE `expires_at` (Option<DateTime<Utc>> → RFC3339 string in DTOs) ↔ FE `string | null` across grant_put body + roll-up.

## Follow-ups (P2a-2 / later)

- Expiry UI on **workspace-member add** and **role assignment** (enforcement already covers them; just add the write-UI + body field).
- An "Expired" facet + a bulk "revoke expired" action on the SharedObjects roll-up; optional janitor job to purge long-expired rows.
- **P2b access-request → approval workflow** — reuses `expires_at` to grant time-boxed access on approval.
