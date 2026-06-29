# Catalog — filter REST table/view listings by token grant — Design

**Date:** 2026-06-29 · **Status:** Design — approved · **Branch:** `fix/catalog-list-filter-by-grant` (off main)
**Program:** Catalog access. Closes the known listing-visibility limitation from the token-grants work (PR #90): a table-scoped token could still *see* other table names in `list_tables`.

## Problem
A token scoped to a specific table (e.g. grant `{sale, "orders", [read]}`) is correctly **denied access** to sibling tables, but `GET /catalog/v1/{prefix}/namespaces/sale/tables` still returns every table name in `sale` (`orders2`, `leads`, …). Information leak: a holder can enumerate table/view names it cannot use.

## Root cause (verified in the pinned lakekeeper fork)
lakekeeper's list path (`server/tabular.rs` `list_entities!`, used by `list_tables`/`list_views`) **does** support per-entity authz filtering, but short-circuits it:
1. it first checks `can_list_everything = is_allowed_namespace_action(ListEverything)`;
2. **if `ListEverything` is allowed → it returns ALL entities, skipping per-entity checks**;
3. only when `ListEverything` is denied does it filter each entity via `are_allowed_<entity>_actions_vec` with the `IncludeInList` action.

egret currently grants `ListEverything` through the navigation-aware `decide_ns_read` (any token that can read the namespace), so the short-circuit always fires and per-table filtering never runs.

## Goal
A token only sees, in a namespace listing, the tables/views it can actually access — without changing access denial (already correct) and without intercepting/parsing the REST response.

## Decision (surgical, `authorizer.rs` only)
Treat `CatalogNamespaceAction::ListEverything` **specially**: allow it only when the token holds a **whole-namespace read grant** covering the namespace (`table == None`, has `read`, `covers_ns`) — i.e. reuse the existing `gate_object(metadata, ns, /*is_write*/ false)` decision instead of `gate_namespace`/`decide_ns_read`.

In `are_allowed_namespace_actions_impl`, branch on the action:
```rust
match action {
    CatalogNamespaceAction::ListEverything => self.gate_object(metadata, levels, false),
    _ => self.gate_namespace(metadata, levels, namespace_action_is_write(action)),
}
```
All other namespace actions are unchanged. (`gate_object`/`decide_object_ns` already exist — they require a `table == None` grant with the needed perm covering the namespace, the exact "can see the whole namespace" semantics.)

### Resulting behaviour
- **Whole-namespace grant** `{sale, null, [read]}` → `ListEverything` allowed → list returns all `sale` tables/views (correct — the token can see the whole namespace).
- **Table-specific grant** `{sale, "orders", [read]}` → `ListEverything` **denied** → lakekeeper falls through to per-entity `IncludeInList`, which routes to egret's existing `are_allowed_table_actions_impl` → `gate_table` → `decide_table`, so only `orders` is returned; `orders2`/`leads` are hidden.
- **`*` grant** → `ListEverything` allowed (covers_ns via `*`) → all listed (correct).
- **Views**: same macro (`are_allowed_view_actions_vec` + `IncludeInList` → `gate_object`), so a table-only grant lists no views in that namespace; a whole-namespace grant lists them.
- The endpoint-level gate (`ListTables`/`ListViews`/`ListNamespaces`) still uses `decide_ns_read` (navigation-aware), so a table-only-grant token can still *call* `list_tables` on the namespace — only the contents are filtered.
- **Admin Browse UI** is unaffected: it authenticates as the system principal, which bypasses the authorizer entirely.

## Components touched
- `crates/egret-catalog-engine/src/authorizer.rs` — the `ListEverything` branch in `are_allowed_namespace_actions_impl` (the only behavioural change) + a unit test.

## Testing
- **Authorizer unit:** for a table-only grant `{sale,"orders",[read]}`: `gate_object`/`decide_object_ns(ns=sale, read)` is **false** while `decide_ns_read(sale)` (endpoint gate) and `decide_table(sale,"orders",read)` stay **true**; for a whole-namespace grant `{sale,null,[read]}`: `decide_object_ns(sale, read)` is **true**. (Direct decision-level assertions; the trait method is exercised live.)
- **Live REST e2e** (the real proof): with a token `[{sale,null,[read]}, {mkt,"campaigns",[read,write]}]`, `GET …/namespaces/mkt/tables` returns **only** `campaigns` (not `leads`), while `GET …/namespaces/sale/tables` returns all `sale` tables; access denials unchanged (load `mkt.leads` → 404).
- **Full local CI** (authz change): fmt + clippy --workspace + `cargo test -p egret-catalog-engine` (incl. the live `front_auth` tests) + FE unchanged.
- **Manual real-Trino test** offered per the standing rule.

## Risks
- **Under-listing:** a whole-namespace grant must keep listing everything (no regression for the common case) — covered by the `decide_object_ns` whole-namespace test + the live e2e's `sale` arm.
- **Endpoint reachability:** denying `ListEverything` must not 403 the list endpoint itself — the endpoint gate is `ListTables` (still `decide_ns_read`), not `ListEverything`; confirmed by the live e2e (`list namespaces` / `list tables` return 200).
- Lakekeeper-internal coupling: relies on the `ListEverything`-short-circuit-then-`IncludeInList` behaviour observed in the pinned fork; the live e2e is the guard against that assumption drifting.

## Out of scope
Filtering child-namespace listings by grant (namespaces a token can't touch may still appear — names only, no access); column-level; the other deferred items (wildcards, TTL).
