# Catalog — token scope v2: resource grants (per-grant read/write) + pickers — Design

**Date:** 2026-06-29 · **Status:** Design — approved · **Branch:** `feat/catalog-token-grants` (off main)
**Program:** Catalog access. Supersedes the flat `namespace_scope` + `table_scope` scope model from PR #89 (migration 0035) with an expressive, OR-evaluated **resource-grant** model and a picker-based builder UI.

## Problem
The shipped model ANDs two flat lists (`decide_table_scope`: namespace_scope prefix must pass, THEN — if `table_scope` non-empty — the table FQN must be listed). Two real gaps:
1. **No pickers** — namespace/table are free-text, not chosen from what exists.
2. **Can't mix resources** — e.g. "all of namespace A" + "only `B.table2`" is impossible: `table_scope` is a *global* filter, so once set it denies every table not in it (including all of A); and namespace_scope+table_scope AND together. Verified live.

## Goal
Token scope = an OR-evaluated list of **resource grants**, each with its own read/write, built in the UI by selecting existing namespaces/tables.

## Model
- **Grant** = `{ namespace: string, table: string | null, permissions: ("read"|"write")[] }`:
  - `namespace`: a dotted prefix (subtree match), or the sentinel **`"*"`** = all namespaces.
  - `table`: `null` = all tables in the namespace (subtree); else an exact table name.
  - `permissions`: non-empty subset of `read`/`write`, **per grant**.
- **Token scope** = `Grant[]`, evaluated **OR**: an action is allowed if **any** grant matches the target resource AND carries the needed permission. **Empty list = no access** (least-privilege). "Read everything" = a single grant `{namespace:"*", table:null, permissions:["read"]}`.
- This resolves the mixed case: `[{"sale", null, ["read"]}, {"marketing","campaigns",["read","write"]}]`.

## Enforcement (`EgretAuthorizer`, OR over grants)
Helper `covers_ns(grant, target_ns)` = `grant.namespace == "*" || ns_prefix_matches(grant.namespace, target_ns)`.
- **Table action** (`target_ns`, `name`, `is_write`): allow if ∃ grant `g` with the needed perm where `covers_ns(g, target_ns)` AND (`g.table is None` OR `g.table == name`).
- **Namespace READ** (GetMetadata / List*): allow if ∃ grant `g` with `read` that *relates* to `target_ns` — `covers_ns(g, target_ns)` OR `ns_prefix_matches(target_ns, g.namespace)` (target is an ancestor of the grant, so the client can navigate **down** to a granted child table). This makes a table-only grant (`B.table2`) usable: listing/reading namespace `B` and the root is permitted.
- **Namespace WRITE** (Create*/Delete/UpdateProperties/SetProtection): allow if ∃ grant `g` with `write` where `g.table is None` AND `covers_ns(g, target_ns)` (whole-namespace grant only).
- **View action**: allow if ∃ grant `g` (`g.table is None`) with the needed perm where `covers_ns(g, view_ns)`. (Views aren't individually pickable; whole-namespace grants cover them.)
- **Generic-table action**: same as view (whole-namespace grant covers them).
- **System principal bypass** unchanged.
- **front-auth coarse gate** (`required_permission`): uses the **union** of all grants' permissions (does the token hold ANY read / ANY write) — the per-resource decision happens in the authorizer.

## Storage / wire format
- **DB (migration `0036`)**: add `scope jsonb NOT NULL DEFAULT '[]'` to `catalog_tokens`, storing `[{ "namespace": "...", "table": null|"...", "permissions": ["catalog:read",...] }]` (full perm strings, matching prior storage). **Backfill** from the old columns, then **DROP** `permissions`, `namespace_scope`, `table_scope`:
  - both `namespace_scope` and `table_scope` empty → `[{namespace:"*", table:null, permissions: <old permissions>}]`;
  - each `ns` in `namespace_scope` → `{namespace: ns, table: null, permissions: <old>}`;
  - each `a.b.tbl` in `table_scope` → `{namespace: "a.b", table: "tbl", permissions: <old>}`.
  (The old AND→new OR shift only affects a token that had *both* lists set; the brand-new feature has none in practice — documented.)
- **Claim**: `{ "egret": { "grants": [ { "ns": "...", "table": null|"...", "perms": ["catalog:read",...] } ] } }` (replaces `perms`/`ns`/`tables`).
- **API** (mint/edit request + DTO): grants use the **short** perms `("read"|"write")[]` both directions (backend maps short↔full at the boundary, as `map_permission` already does one way). Request `{ label, scope: [{ namespace, table?, permissions }] }`; DTO `{ id, label, scope: [{ namespace, table, permissions }], created_at, rotated_at? }`.

## Components touched
- Migration `0036_catalog_token_scope.sql`.
- `egret-catalog/src/service/tokens.rs` — `Grant { namespace, table: Option<String>, permissions: Vec<String> }`; `CatalogToken.scope: Vec<Grant>` (drop the 3 fields); `TokenAuth { workspace_id, warehouse_id, grants: Vec<Grant>, permissions: Vec<String> /* union, for the coarse gate */ }`; `create_token`/`update_token_for_warehouse` take `&[Grant]`; `authenticate_token` selects+parses `scope`; `validate_grants` (namespace valid-or-`*`; table name valid when present; permissions a non-empty subset of `{catalog:read,catalog:write}`); store/parse jsonb via `serde_json`.
- `egret-catalog-engine/src/front_auth.rs` — claim `grants`; coarse gate over the union perms.
- `egret-catalog-engine/src/authorizer.rs` — parse `grants` from the claim; replace `decide_scope`/`decide_table_scope`/`gate`/`gate_table` with grant-aware decisions for table/namespace/view/generic actions per the rules above.
- `egret-catalog-engine/src/browse.rs` — `CatalogTokenDto.scope`, `MintTokenRequest`/`UpdateTokenRequest` carry `scope`; map short↔full; mint/update validate ≥1 grant.
- `frontend/src/api/catalog.ts` + `CatalogAccess.tsx` — `Grant` type; a **grant builder** (rows with namespace select + table select + read/write toggles + remove, "＋ Add resource", ≥1 required) in mint + edit; list summarizes grants. Pickers: namespaces from `useCatalogNamespaces(ws, wid)`; tables from `useNamespaceContents(ws, wid, ns)` when a namespace is chosen (plus an "All tables" option and an "All namespaces" option).

## Testing
- **Authorizer unit:** OR cases — mixed `[{A,null,[read]}, {B,t2,[read,write]}]`: read A.x ✓, write A.x ✗ (A is read-only), read/write B.t2 ✓, B.other ✗, C.* ✗; `"*"` grant; namespace-navigation (list B allowed for a `B.t2`-only grant; create-table in B denied for a table-only grant); empty scope = deny-all.
- **Store:** round-trip grants → `TokenAuth.grants` + derived union `permissions`; migration backfill (the three conversion cases).
- **Integration (`catalog_tokens_api`):** mint/edit with a multi-grant scope; the DTO reflects it; the original secret authenticates with the new scope after edit; ≥1-grant validation (empty → 400); cross-warehouse 404.
- **Frontend (Vitest):** grant builder adds/removes rows; namespace select populates the table select; ≥1 grant required; mint/edit submit the parsed `scope`; list renders a grant summary.
- **Manual real-Trino + focused live REST e2e** (authz change): mint a mixed-grant token and confirm allow/deny across two namespaces/tables on the iceberg REST path.

## Risks
- **Authz rewrite** — the gate logic is replaced, not extended. Mitigate: exhaustive OR unit tests (allow/deny matrix incl. navigation + write-vs-read + `*`), live REST e2e, manual Trino test, and `decide`-level helpers kept small and pure.
- **Migration data shift** (AND→OR) — only affects tokens with both old lists set (none in practice); documented; backfill is deterministic.
- **Namespace-navigation rule** — the "target is ancestor of grant" read allowance is what makes table-only grants usable; without it a `B.t2` token couldn't list `B` to reach the table. Covered by a unit test.

## Out of scope (later)
Wildcard table patterns beyond `"*"`-all-namespaces; column-level scope; authz-filtering the REST table listing (the known listing-visibility limitation persists); token expiry/TTL; per-grant expiry.
