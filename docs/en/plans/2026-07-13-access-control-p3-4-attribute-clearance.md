# Access Control P3.4 — Attribute (group) clearance + gate surfacing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Builds on P3.3 (Cedar tag-conditioned object access). Fully CI-testable.

**Goal:** Extend P3.3 classification clearance so a tag gate can be satisfied by **group membership** (in egret, OIDC groups = departments/attributes), not just role — and **surface** classification gates in the effective-access resolver so admins see who's blocked and why. Scoped, dependency-free, CI-testable.

**Scope note:** This is the "attributes" half of the spike's P3.4. **Deferred** (separate future work): StarRocks tag masking (live-only, like P3.2), lineage-based tag propagation (needs the not-yet-built Data-Lineage feature), OIDC arbitrary-claim capture beyond groups.

## Reuse surfaces (P3.3, verified)
- `crates/egret-auth/src/pdp/mod.rs` — `authorize` builds `context = { is_platform_admin, roles: [allow-effect role names] }`; `load_inner` compiles per-gated-tag `forbid(...) unless { context.roles.containsAny([roles]) || context.is_platform_admin }`. The `Subject` (`egret_core::authz`) carries `group_ids: Vec<GroupId>` (uuids). `build_entities` already models `User in Group`.
- `crates/egret-auth/src/pdp/templates.rs` — `classification_forbid(tag_key, required_roles)` → the forbid string. **Security lesson from P3.3: validate anything spliced into the Cedar string; group uuids are inherently safe (uuid format), role names are charset-validated.**
- `crates/egret-bi/src/tags.rs` — `ClassificationPolicyRepo` (`set_policy`/`get_policy`/`list_policies`), `validate_role_name`.
- `crates/egret-controlplane/src/tags.rs` — the access-policy endpoints + `TagDto.required_roles`.
- `crates/egret-auth/src/effective.rs` — `AccessResolver` (grant_rows, who_can_access, the principal's `denies`/`objects`); this is where gates are surfaced.
- FE: `frontend/src/routes/control-plane/governance/Classifications.tsx` (clearance editor), `frontend/src/api/access.ts` (`listGroups`).
- Migration: latest `0082`; next **`0083`**.

## Data model — migration `0083_classification_required_groups.sql`
```sql
ALTER TABLE classification_policies ADD COLUMN required_groups uuid[] NOT NULL DEFAULT '{}';
```
A principal is **cleared** for a gated tag if they hold ≥1 `required_roles` role **OR** belong to ≥1 `required_groups` group (OR is platform_admin). Both empty ⇒ **no gate** (informational tag).

## Tasks

### Task 1: BE — group clearance in the model + PDP (core, CI-tested)
**Files:** `migrations/0083_*`; `egret-metastore/src/lib.rs`; `egret-bi/src/tags.rs` (repo); `egret-controlplane/src/tags.rs` (CRUD + DTO); `egret-auth/src/pdp/{templates.rs,mod.rs}`. Tests in each.
- [ ] **Migration + repo:** `ClassificationPolicyRepo::set_policy(tag_id, required_roles, required_groups, by)` — validate roles (`validate_role_name`) AND that each group entry is a valid `uuid` (parse; reject else → BadRequest). `get_policy`/`list_policies` return `(roles, groups)`. `get_policy` semantics: **no gate** iff BOTH empty.
- [ ] **CRUD/DTO:** `PUT /tags/{id}/access-policy` body `{ required_roles: string[], required_groups: string[] }` (group uuids); `TagDto` gains `required_groups: Vec<String>`. `tag:manage`.
- [ ] **PDP:**
  - `authorize`: add `"groups": [<subject.group_ids as strings>]` to the context json (from `subject.group_ids` — no query).
  - `templates.rs classification_forbid(tag_key, required_roles, required_groups)`: emit
    `forbid(...) when { resource has tags && resource.tags.contains("<key>") } unless { (context has roles && context.roles.containsAny([<roles>])) || (context has groups && context.groups.containsAny([<group uuids>])) || (context has is_platform_admin && context.is_platform_admin) };`
    Splice roles (charset-validated) + group uuids (validate they parse as uuid before splicing — defense in depth). `load_inner` still skips-and-logs a bad row (P3.3 hardening — keep it).
- [ ] **Tests (Cedar, in-process):** a tag gated by group G: a subject in G → Allow; not in G and no cleared role → Deny; in G OR holding a cleared role → Allow; platform_admin → Allow; empty roles+groups ⇒ no gate. Injection: a non-uuid group entry → set_policy BadRequest; a raw bad group row → load_inner skips it (PDP still loads). Parity: existing P3.3 role-only gates unchanged. `cargo test -p egret-auth -p egret-bi -p egret-controlplane`.
- [ ] Commit `feat(authz): classification clearance by group (mig 0083) — attribute-based access`.

### Task 2: BE — surface classification gates in the effective-access resolver
**Files:** `crates/egret-auth/src/effective.rs` (+ the who-can-access path). Tests.
- [ ] For a principal's effective access: add a **classification-blocks** section — objects the principal holds a read grant for BUT is blocked from by a classification gate (object carries a gated tag + the principal lacks any required role/group). Compute from `classification_policies` + the principal's roles/groups + the object's tags. Add to the `EffectiveAccess`/`WhoCanAccess` DTO (a `classification_blocks: [{ object, tag, required_roles, required_groups }]` or fold into `denies` with scope `classification`).
- [ ] (Who-can-access) for a gated object, mark which listed principals are cleared vs blocked by the tag.
- [ ] Keep it bounded — a read-only overlay; if the join gets heavy, ship the principal→blocks side only and note the reverse as a follow-up.
- [ ] Tests: a principal blocked by a classification tag shows in the blocks section; a cleared principal does not. Commit `feat(access): surface classification gates in effective-access`.

### Task 3: FE — group clearance editor + gate display
**Files:** `api/tags.ts` (`required_groups`), `Classifications.tsx` (group multi-select alongside roles), + optionally the Access-explorer/Who-can-access to show classification blocks. Vitest.
- [ ] Clearance editor: add a **groups** multi-select (from `listGroups`) beside the roles one; the Gated badge reflects roles + groups. Save sends both.
- [ ] (If Task 2 surfaced blocks) show a "blocked by classification: {tag}" note in Access explorer / Who-can-access.
- [ ] Commit `feat(fe): classification clearance by group + gate display`.

### Task 4: Full verification
- [ ] `cargo fmt/clippy/test -p egret-auth -p egret-bi -p egret-controlplane`; FE `lint/typecheck/test/build`.
- [ ] Security sanity: group ids validated as uuid before splicing (no Cedar injection — reaffirm P3.3 lesson); parity (role-only gates + untagged objects unchanged); platform_admin never locked out.

## Non-Goals
- StarRocks tag masking (live-only). Lineage tag propagation (needs Data-Lineage). Arbitrary OIDC-claim clearance beyond groups. key=value tags. Tag-based access-requests (P2b reuse — a separate future item).

## Self-Review Checklist
1. Group uuids validated (parse) before splicing into Cedar — no injection (P3.3 lesson).
2. Parity: role-only P3.3 gates + untagged objects unchanged.
3. platform_admin exemption still via `context.is_platform_admin` (scoped), never bypassable by name.
4. Both-empty ⇒ no gate.
5. Gate surfacing is a read-only overlay; no new write/authz path.

## Follow-ups
- StarRocks tag masking parity (live). Lineage-based tag propagation. Tag-based access requests. Per-action clearance. Reverse gate view (which objects a tag gates).
