# EO4 — One grant store for egret objects (capstone)

Status: design, 2026-11-18. Sub-project **EO4** of the egret-objects programme, under the decomposition spec [EO0](2026-11-18-gov-eo0-egret-objects-decomposition-design.md). EO4 is the **capstone**: it folds the Cedar grant table `access_grants` into the `perm.*` one grant store, behind a reversible per-enforcement-point switch, so a single store answers both data-plane and egret-object grants.

This spec **refines EO0's EO-D5**. EO-D5 framed `access_grants` as "Cedar's own grant table (allow/deny, platform/workspace/object, expiry)" folding into `perm.grants`. Grounding on the live schema changed two details, recorded as EO4-D1/EO4-D2 below; the intent (one grant store, SP5b-shaped reversible switch) is unchanged.

## Why

After EO1–EO3, egret objects are securables in the IR (EO1), the `egret-cedar` target compiles their Cedar `PolicySet` decision-equivalently (EO2), and `CedarPdp` can consume that compiled policy behind the reversible `perm.pdp_policy_source` switch (EO3). One thing still comes from the **legacy store**: the per-request grant **links**. `CedarPdp` builds a static template set (one `role:{id}` / `role-deny:{id}` per role) and, on every decision, links those templates to the subject's grants read live from `access_grants` (`AccessGrantStore::grants_for` → `pdp::entities::link_grants`). So after EO3 the **policy source** is unified but the **grant source** is still `access_grants`.

EO4 folds that last store. It mirrors `access_grants` into `perm.*`, adds a reversible per-EP **grant-source** switch for the `egret-cedar` EP, routes the PDP's grant read through it, and proves decision-equivalence over a sample before any flip — exactly as SP5b did for catalog grants. After EO4, the one grant model holds egret-object grants too; deleting the legacy read paths is a later, post-soak **finalize** (EO4-D5), as EO3 deferred its own.

## What the live schema says (grounding)

`access_grants` is a **role-assignment** table, not a direct allow/deny permission table:

```text
access_grants(
  id, principal_type ∈ {user,group,service}, principal_id,
  role_id → roles(id), scope_type ∈ {platform,workspace,object}, scope_id,
  object_type, granted_by, created_at,
  effect ∈ {allow,deny} default allow,   -- 0072
  expires_at timestamptz NULL)            -- 0078
```

A row assigns a **role** to a principal at a scope, with an effect and optional expiry. The Cedar decision path reads it through one function:

```rust
// crates/egret-auth/src/grants.rs
AccessGrantStore::grants_for(user_id, ws, object_scope_ids)
  -> Vec<Grant { role_id, scope_type, scope_id, object_type, effect }>
```

filtered to the subject (direct or via `user_groups`), to platform scope, the request's workspace, the request's object-scope ids (object + its folder, for the folder cascade), and to unexpired rows. `pdp::entities::link_grants` then links `role:{role_id}` (or `role-deny:{role_id}` for a deny) to `(principal, scope-resource)`.

The data-plane `perm.grants` table is **privilege-on-catalog shaped** (`catalog_id uuid NOT NULL REFERENCES registry.catalogs`, `actions text[]`, `schema_name/table_name/column_name`). It cannot hold a role-scoped egret-object grant, and reshaping it would re-open the proven data-plane path. EO4 therefore adds a **sibling** table in the `perm.*` namespace, not a column change to `perm.grants` — the same "standalone under one store" pattern EO2 used for the capability-matrix entry and SP5c used for the iceberg-rest path.

## Decisions

| # | Decision |
|---|---|
| EO4-D1 | **Mirror, don't expand.** The unified store keeps the role-assignment shape: a new table `perm.cedar_grants` saves `(principal_type, principal_id, role_id, scope_type, scope_id, object_type, effect, expires_at)` — a faithful copy of `access_grants`'s decision-relevant columns. `link_grants` is unchanged, so equivalence is tight and the Cedar engine path is untouched. (The alternative — expanding each assignment into per-object `READ/WRITE/MANAGE` privilege grants — loses the role fact, must re-express deny/expiry/cascade, and would fork the link machinery. Rejected.) The capstone's job is one **store**, not one row **shape**; SP5b likewise kept catalog grants as their own rows under `perm.*`. |
| EO4-D2 | **Sibling table, not `perm.grants`.** `perm.cedar_grants` lives in the `perm.*` namespace and shares the one-store machinery (a switch table and the equivalence gate), but does not touch the catalog-shaped `perm.grants`. It is a **trigger-synced replica**: a trigger on `access_grants` mirrors every write, so the `unified` read never goes stale, and `access_grants` stays the write target through the reversible phase (writes redirect here only at the deferred finalize). |
| EO4-D3 | **Per-EP reversible switch**, mirroring EO3 and SP5b. A singleton/per-EP row `perm.cedar_grant_source` with states `legacy` (default) / `unified`, re-read on every PDP reload. `grants_for` reads `access_grants` under `legacy`, `perm.cedar_grants` under `unified`. Default `legacy` ⇒ **zero behaviour change**. |
| EO4-D4 | **Equivalence-gated flip.** The switch to `unified` is refused unless a fresh equivalence report over a sample of (principal × object/scope × action) proves the PDP returns the same allow/deny with grants read from `perm.cedar_grants` as from `access_grants`. The gate has **teeth**, proven by a seeded over-grant/omission it must catch. A non-empty report refuses the flip (409) and lists the divergences — the EO3/SP5b pattern. |
| EO4-D5 | **Reversible; finalize deferred.** EO4 delivers the mirror store, the switch, the gate, and undo-while-open. It keeps the `access_grants` read path alive. Deleting the legacy read paths (`AccessGrantStore` over `access_grants`, the bespoke `templates.rs`/`load_inner` generation from EO3, and eventually `access_grants` itself and the session-building reads in `effective.rs`) is a single **post-soak finalize**, combined with EO3's deferred finalize, taken only after a long clean soak on `unified`. Not in EO4. |
| EO4-D6 | **No new decisions, no vocab change.** EO4 moves the grant **source** only. It adds no permission, changes no grant semantics, and does not touch roles/`role_permissions` (the privilege definitions that back both object-data grants and the typed Cedar-EP privileges of EO-D3). Only the assignment store the PDP links from moves. |

## Concepts

### The mirror store `perm.cedar_grants`

A table holding the decision-relevant columns of `access_grants` under `perm.*`, kept **live-consistent by a trigger** on `access_grants` (every INSERT/UPDATE/DELETE mirrors over). `fold` backfills the rows that predate the trigger. Because the replica always equals the source, `grants_for` reading `perm.cedar_grants` (`unified`) returns exactly what it returns from `access_grants` (`legacy`) — the switch is pure read-routing, never a stale snapshot. `access_grants` stays the write target throughout, so undo (back to `legacy`) is instant and the fold never mutates the source (SP5b's "fold doesn't touch the write source" rule). Provenance is the replica's own row lineage (each `cedar_grants.id` = its `access_grants.id`) plus the switch row's audit columns; no separate provenance table (`perm.migrated_from` is Ranger-policy-specific and does not fit).

### The grant-source switch `perm.cedar_grant_source`

The `egret-cedar` EP's grant source, re-read on every PDP reload (publishing `RolesChanged` on change, exactly as EO3's `pdp_source_routes` do). It composes with EO3's `perm.pdp_policy_source`: EO3 chooses where the **templates** come from, EO4 chooses where the **links** come from. Both default to the legacy source, so an installation that has flipped neither behaves identically to pre-EO.

### Equivalence report

For a sample of subjects × resources (platform, workspace, object + folder-cascade cases) × actions, run the live `CedarPdp` decision with grants from `access_grants` and with grants from `perm.cedar_grants`; a divergence is any (subject, resource, action) whose allow/deny differs. An empty report over the sample is the signal that `unified` is safe. The report is available on demand and is the switch gate (EO4-D4).

## Architecture

```text
 Grant source (EO4)                      Templates (EO3)              Runtime
 access_grants ──┐                        pdp_policy_source ──┐
                 │  grants_for (switch:    legacy|compiled    │
 perm.cedar_   ──┘  cedar_grant_source) ─► link_grants ──────►├─► CedarPdp::is_authorized
   grants           legacy|unified                            │   (arc_swap PolicySet,
                                                               │    AuditingPdp)
```

- **Store**: `perm.cedar_grants` (trigger-synced replica of `access_grants`) + `perm.cedar_grant_source` switch (two migrations).
- **Read routing**: `AccessGrantStore::grants_for` reads the source the switch names. One call site feeds the PDP; the SQL is the same shape against either table.
- **Fold / undo / report / switch**: control-plane endpoints under `/api/control-plane/perm/cedar-grants` (platform-admin), mirroring EO3's `pdp-source` and SP5b's `catalog` lifecycle: `GET` (state), `POST /report` (equivalence on live data), `POST /fold` (copy + provenance, still `legacy`), `POST /switch` (gated flip to `unified`), `POST /undo` (back to `legacy`). Every mutation publishes `RolesChanged`.

The session-building reads in `effective.rs` (platform roles, effective permissions) are a **separate consumer** of `access_grants`, not the PDP decision path. They keep reading `access_grants` (the write source stays intact under the reversible phase) and move only at the post-soak finalize (EO4-D5). EO4's equivalence gate is about PDP decisions, which flow solely through `grants_for`.

## Scope

**In:** the mirror table, provenance, the grant-source switch, read routing in `grants_for`, the equivalence report + gate, the fold/undo/switch endpoints, and tests (store, per-state routing, fail-safe to legacy on a missing/absent row, an e2e hot-flip proving decisions unchanged across `legacy → fold → unified → undo`, and the equivalence-gate teeth).

**Out:** finalize (deleting `access_grants`, `effective.rs`'s read of it, and EO3's bespoke `templates.rs`/`load_inner`) — the combined post-soak terminal (EO4-D5); any change to roles/`role_permissions`, to the typed Cedar-EP privileges, or to the classification/clearance `forbid` path (EO2); making `notebook`/`workflow`/`alert` securables (EO0 EO-D6, a later sub-project).

## Errors

Fail closed, matching EO3 and SP5b:

- **Switch row absent or unreadable** → treated as `legacy`; the PDP reads `access_grants` (the known-good source). `read_source` is infallible.
- **A `perm.cedar_grants` read errors while `unified`** → fail-safe to `access_grants` for that build and raise an alarm; never serve a partial grant set.
- **Equivalence not proven** → the flip is refused (409) with the divergence list; the EP stays `legacy`.
- **A principal cannot be resolved** → left out of every allow, still covered by every deny (unchanged link semantics).

## Verification

1. **Store tests**: `fold` makes `perm.cedar_grants` a faithful replica; the trigger mirrors a later `access_grants` write/delete; undo restores `legacy`.
2. **Per-state routing**: `grants_for` reads `access_grants` under `legacy` and `perm.cedar_grants` under `unified`; the absent/err fail-safe serves `legacy`.
3. **Equivalence gate with teeth**: over the sample the report is empty when the mirror is faithful; a seeded over-grant (an extra `unified` row) or omission makes the report non-empty and the flip refuse.
4. **e2e hot-flip**: `legacy → fold → unified → undo` leaves PDP decisions unchanged at every step and the live `arc_swap` serves the expected grant set (the EO3 `pdp_source_hot_flip` analogue).
5. **Zero behaviour change by default**: default `legacy` is byte-for-byte the pre-EO4 grant read; `grants_for`'s signature and all its callers are unchanged; the EO2/EO3 differentials stay green.

## After EO4

The egret-objects programme EO0–EO4 is then **feature-complete**: egret objects are securables (EO1), compiled decision-equivalently for the `egret-cedar` EP (EO2), served by `CedarPdp` from IR-compiled policy behind a reversible switch (EO3), and granted from the one `perm.*` store behind a reversible switch (EO4). What remains is operational, not new scope: soak `perm.pdp_policy_source` and `perm.cedar_grant_source` on `compiled`/`unified`, then a single **finalize** that deletes the legacy policy generation, the `access_grants` read paths, and the bespoke templates — closing out both EO3's and EO4's deferred terminals at once.
