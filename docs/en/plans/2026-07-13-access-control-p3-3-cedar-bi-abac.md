# Access Control P3.3 — Cedar BI ABAC (tag-conditioned object access) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Design: `docs/en/plans/2026-07-13-access-control-p3-abac-design-spike.md`. Builds on P3.1 (BI object tags).

**Goal:** Control-plane ABAC on BI objects: **tag a dashboard/chart/dataset `confidential` → only principals holding a cleared role can open it**, with zero per-object grants. Enforced by the existing Cedar PDP via deny-wins. Complements P3.2 (which masks *data columns*; this governs *object visibility*).

**Fully CI-testable** (Cedar decisions run in-process) — unlike P3.2. Normal admin-merge after CI-green.

## Architecture (verified — simpler than the spike)

The spike said "flip the PDP to load the schema." **Not needed:** `Authorizer::is_authorized(&req, &ps, &ents)` runs `schema = None` and evaluates entity/context attributes fine (schema only *validates*). So P3.3 = three additive changes to the Cedar path (`crates/egret-auth/src/pdp/`):

1. **Object `tags` attribute** — `build_entities` (`entities.rs`) currently uses `Entity::new_no_attrs` for the Object. Give the Object-under-decision a `tags: Set<String>` attribute, populated from `tag_assignments` (P3.1) for that `(kind,id)`. `authorize` (`mod.rs:189`) fetches the tags (one indexed query, Object resources only) and passes them in.
2. **`context.roles`** — `authorize` already builds `context={is_platform_admin}`. Add `roles: [<the subject's role names>]` (computed from the `grants` + `inner.role_names` already loaded — no extra query).
3. **Classification forbid policies** — a new static policy source in `PdpInner` (loaded in `load_inner`, hot-reloaded). Per tag with a clearance policy: `forbid(principal, action, resource) when { resource has tags && resource.tags.contains("<tag>") } unless { context has roles && context.roles.containsAny([<required roles>]) };`. `platform_admin` bypass still holds (the admin unrestricted permit + this is a forbid → but forbid beats permit, so **the clearance requirement applies even to admins UNLESS admin is a required role** — include `platform_admin` implicitly in the unless, or document that admins must be listed; DECIDE in Task 2: recommend auto-exempting `platform_admin` in the `unless` so a superuser is never locked out).

Enforcement is automatic: `require_object(read, object)` already runs `authorize`; the added forbid applies (deny-wins) when the object carries a clearance-gated tag and the principal lacks a cleared role.

## Reuse surfaces
- `crates/egret-auth/src/pdp/{mod.rs,entities.rs,templates.rs}` — `authorize` (grants_for → link_grants → build_entities → context → is_authorized), `load_inner` (compiles templates + the static admin-graph forbid — mirror that for classification forbids), `spawn_reload` (RolesChanged bus — reuse or add a `ClassificationChanged` event).
- P3.1: `tags` catalog + `tag_assignments` (BI objects) + `TagRepo` (egret-bi). This phase reads `tag_assignments` from the PDP (egret-auth) via `self.db`.
- `crates/egret-controlplane/src/tags.rs` — the Classifications endpoints (extend with a per-tag clearance policy).
- FE: `frontend/src/routes/control-plane/governance/Classifications.tsx` + `api/tags.ts`.
- Migration: latest `0081`; next **`0082`**.

## Data model — migration `0082_classification_policies.sql`
```sql
CREATE TABLE classification_policies (
    tag_id        uuid PRIMARY KEY REFERENCES tags(id) ON DELETE CASCADE,  -- one clearance policy per tag
    required_roles text[] NOT NULL DEFAULT '{}',   -- role NAMES; a principal needs ≥1 to access objects with this tag
    created_by    uuid REFERENCES users(id),
    updated_at    timestamptz NOT NULL DEFAULT now()
);
```
(A tag with NO row = not access-gated, only informational/masking. A row with empty `required_roles` = effectively no one but platform_admin — DECIDE the empty semantics in Task 1; recommend: empty ⇒ treat as "no gate" to avoid accidental lockout, i.e. only enforce when `required_roles` non-empty.)

## Tasks

### Task 1: BE — model + repo + CRUD (extend Classifications)
**Files:** `migrations/0082_*`; `egret-metastore/src/lib.rs`; a `ClassificationPolicyRepo` (egret-bi, next to `TagRepo`); extend `egret-controlplane/src/tags.rs` endpoints. Tests.
- [ ] TDD the repo: `set_policy(tag_id, required_roles, by)` (upsert), `clear_policy(tag_id)`, `get_policy(tag_id)`, `list_policies()` (for the PDP loader + the UI). Endpoints: `PUT/DELETE /api/control-plane/tags/{id}/access-policy` body `{ required_roles: string[] }`, gated `tag:manage`; include the policy in the `GET /tags` list DTO (`required_roles` or a nested `access_policy`).
- [ ] Emit a reload signal on change (a `ClassificationChanged` event on the egret-events bus, or reuse `RolesChanged` if simplest — the PDP reload re-reads everything).
- [ ] Commit `feat(access): classification access policies (mig 0082) + CRUD`.

### Task 2: BE — Cedar PDP wiring (the core, CI-tested decisions)
**Files:** `crates/egret-auth/src/pdp/{entities.rs,mod.rs}`. Tests in `entities.rs` + a PDP DB test.
- [ ] **Object tags attr:** change `build_entities` to accept the object's tags (e.g. `build_entities(subject, resource, &object_tags)` or a new fn) and build the Object entity with `Entity::new(uid, {"tags": Set<String>}, parents)`. Update all call sites/tests (pass `&[]` for the no-tags cases).
- [ ] **load_inner:** load `classification_policies`; for each with non-empty `required_roles`, add a `forbid(...) when { resource has tags && resource.tags.contains("<tag>") } unless { context has roles && context.roles.containsAny([<roles + "platform_admin">]) };` to `PdpInner` (new field). Compile once; hot-reloaded via the reload task (subscribe to the new event or reuse the loop).
- [ ] **authorize:** add `roles: [subject role names]` to the `context` json; for Object resources, query `tag_assignments` for `(kind,id)` and pass to `build_entities`. Platform/Workspace → no tags.
- [ ] **Failing tests (Cedar, in-process — the key coverage):** an object tagged `confidential` with a clearance policy requiring `finance_cleared`: a subject WITHOUT that role → `authorize(read, obj)` DENIES; a subject WITH `finance_cleared` (or `platform_admin`) → ALLOWS; an object with the tag but NO clearance policy → ALLOWS (informational tag); an object WITHOUT the tag → unaffected. Plus a `build_entities` unit test that the Object entity carries the tags.
- [ ] Confirm parity: existing PDP tests stay green (untagged objects + non-object resources unchanged).
- [ ] Commit `feat(authz): tag-conditioned object access in the Cedar PDP (deny-wins)`.

### Task 3: FE — clearance editor on Classifications
**Files:** `api/tags.ts` (policy hooks) + `Classifications.tsx` + test.
- [ ] In the tag row/edit dialog: a "Required roles (clearance)" multi-select (roles from `listRoles`) — setting it calls the access-policy endpoint; empty = no gate. Show a badge on tags that are access-gated ("gated: {roles}"). Vitest.
- [ ] Commit `feat(fe): classification clearance (required roles) editor`.

### Task 4: Full verification (+ optional Who-can-access surfacing)
- [ ] `cargo fmt/clippy/test -p egret-auth -p egret-bi -p egret-controlplane`; FE `lint/typecheck/test/build`.
- [ ] (Optional, if cheap) surface in the effective/who-can-access resolver: note when access is blocked by a classification policy. Otherwise a follow-up.

## Non-Goals
- No schema flip / Cedar validation (evaluation is schema-less).
- Per-action clearance (start with a single gate covering all object actions; per-action later).
- key=value tags, principal-attribute (department) clearance beyond role membership.
- Data-plane (that's P3.2).

## Self-Review Checklist
1. Additive: untagged objects + Platform/Workspace decisions unchanged (existing PDP tests green).
2. platform_admin never locked out (auto-exempt in the `unless`).
3. Empty/absent policy ⇒ no gate (no accidental lockout).
4. The forbid uses `has` guards so a resource/context without the attr doesn't error.
5. Hot-reload: a clearance change re-polls the PDP (event wired).

## Follow-ups
- Surface classification blocks in Who-can-access ("blocked by tag: confidential"). Per-action clearance. Group/attribute-based clearance. P3.4 propagation.
