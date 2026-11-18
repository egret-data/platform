# Governance SP5c — iceberg-rest enforcement point — design

> Third and final package of SP5 (the one-grant-model capstone) of the
> governance-layer programme. Authoritative target: the concepts spec
> `2026-11-02-governance-layer-concepts.md` (D5, D7, sub-project 5) and the SP5
> programme spec `2026-11-16-gov-sp5-one-grant-model-design.md`. Builds on
> SP5a (full privilege vocabulary, #873) and SP5b (catalog merge / one grant
> store, #874). After SP5c, sub-projects 0–5 are all done.

## What SP5c is

**Acceptance (concepts spec, sub-project 5 / SP5 programme spec SP5c row):** an
external Iceberg client obeys common grants *and* `withhold`; a table carrying a
mask, row filter or withhold denies `READ` at the catalog; live-verified on the
demo stack.

The embedded Iceberg catalog (Lakekeeper) already enforces the unified common
grants after SP5b (per-catalog switch `source = unified`). SP5c adds the **rule**
layer: the catalog denies `READ` on any table that has an active mask, row filter
or withhold rule, to everyone but that rule's exemptions — plus the capability
matrix entry that records what `iceberg-rest` can and cannot enforce, and a
live-check.

### Why this is a deliberate behaviour change (not equivalence-preserving)

SP5a and SP5b were *zero-behaviour-change, equivalence-proven*. SP5c is **not**.
Today an external Iceberg client with a catalog credential reads a table's data
files directly through vended STS credentials, so a table that carries a mask,
row filter or clearance is served **in full** — the DG mask/filter/clearance is
bypassed. This is the gap SA2 noted ("a catalog credential reads Iceberg directly,
outside DG").

Masks and row filters are per-column / per-row *transforms*; they cannot run at
the storage layer. So the only control the catalog can apply is to **deny the
whole table's `READ`** (concepts spec §"Enforcement point", consequence for
`iceberg-rest`: *"That EP denies `READ` on any table with a mask, row filter or
withhold to everyone but the exemptions."*). This is a new, intentional denial.
Because it changes behaviour, SP5c cannot use an SP5b-style "legacy == new" gate;
instead its gate proves two things:

1. **Grants unchanged.** Every GRANT read/write decision stays identical to
   SP5b's unified decision (SP5c only *adds* rule-denies; it never changes what a
   grant allows or denies).
2. **The new deny set is exactly the ruled tables.** The `plan` endpoint
   enumerates the `(principal, table)` pairs that will flip `allow → deny`
   because a rule applies and the principal is not exempt, for a human to review
   before switching.

## Decisions (user-approved 2026-11-17)

1. **Separate, opt-in per-catalog switch.** A new per-catalog `iceberg-rest`
   rule-enforcement switch, **default off**, independent of SP5b's
   `catalog_grant_switch`, reversible. A catalog begins denying-by-rule only when
   a platform admin turns it on after reviewing the `plan`. (Concepts spec
   §"Migration principles": *one EP at a time, each EP has a switch, old / new.*)
   This keeps SP5b-migrated catalogs behaving exactly as before until an admin
   opts in to SP5c.
2. **Backend-only + live-check.** Enforcement at the guard + authorizer, the
   `iceberg-rest` standalone capability-matrix table, the plan/switch/undo
   endpoints, and a demo-stack live-check. **No** governance-overview badge / DTO
   and **no** frontend this package (deferred, as SP5b deferred its UI).
3. **Credential staleness is a documented known-limitation.** A `READ` on a
   newly-withheld table fails closed on any **new** table load (the authorizer
   returns `ReadData = false`, so Lakekeeper vends no credential). An
   already-vended STS credential persists for up to the token lifetime (default
   ~1h; KI-315). SP5c does **not** add active credential revocation.

## Granularity

Table-level. A mask on a single column, a row filter, or a withhold anywhere on a
table denies `READ` on the **whole** table at the catalog — column-level masking
is impossible at the storage layer (concepts spec §"Enforcement point" table:
*mask on a column → deny that column … ; for `iceberg-rest`, the whole table*).

## Architecture

### A. Extend the per-catalog snapshot to carry rules + classification

`egret_perm_compile::opa::build::build_catalog_snapshot_via(conn, catalog_id)`
(`crates/egret-perm-compile/src/opa/build.rs`) today lowers **only** grants +
owners (`lower_grants`) and builds the `Snapshot` with `classified: Vec::new()`.
It therefore cannot see any mask / row-filter / withhold statement.

SP5c extends the catalog snapshot build so it additionally:

- lowers the catalog's applicable **rules** (mask, row filter from `perm.rules`)
  and **withhold / clearance** inputs into statements — reusing the existing
  lowering (`egret_perm_ir::lower::rules`, `egret_perm_ir::lower::withhold`) and
  the loaders in `egret-perm-compile` that already feed the Ranger/OPA snapshots;
- populates the `classified` index for the catalog's objects (so a rule selected
  by a **classification** resolves to the catalog's tagged tables/columns —
  `Snapshot::candidates` needs the class index to match a `Tag` selector).

The grant/owner statements and the grant decision path are unchanged, so SP5b's
grant equivalence is preserved by construction. A separate build entry point (or
a flag on the existing one) keeps the grant-only snapshot available for the
grant-decision path if that is cleaner — the plan decides, but the grant decision
must not change.

### B. The withheld-read predicate

A new predicate on the catalog `Snapshot`:

```
fn table_read_restricted(snapshot, user, catalog, schema, table) -> bool
```

`true` when any statement in `Snapshot::candidates(catalog, Some(schema),
Some(table), classes)` has an effect that **restricts** `READ`
(`egret_perm_eval` `restricts(effect)`: `Deny`/`Withhold` carrying `Read`, a
`Mask` other than `MASK_NONE`, a non-empty `RowFilter`, or `WithholdMask`) **and**
the user is in the statement's audience and **not** exempt
(`applies(&s.audience, user)`). It aggregates column-level masks to the table (any
masked column ⇒ table restricted). It reuses the exact candidate-scan +
`restricts` + `applies` machinery that `row_filters`, `column_mask_choice` and
`metadata_refusal` already use, so it agrees with how every other enforcement
point reads the same rules.

The catalog READ decision becomes, for a catalog whose `iceberg-rest` switch is on:

```
read_allowed = covers(…, write=false)  AND NOT table_read_restricted(…)
```

`covers` (grants) is unchanged; the second term is the only new denial. (A
withhold/clearance carrying `Read` is already denied by `decide_privilege(Read)`
inside `covers` once it is in the snapshot; the predicate covers all three effects
uniformly so there is one clear gate.)

### C. Two enforcement hook points (both required)

A catalog client's READ is decided in two independent places; a withheld table
must be denied in **both**, or data leaks:

1. **Request guard** — `crates/egret-bin/src/main.rs` `EgretCatalogGuard::check`,
   the per-table `CatalogDecider::covers(ns, table, write=false, …)` path. Denies
   before the request reaches Lakekeeper (stops metadata/table-load).
2. **Lakekeeper authorizer** — `crates/egret-catalog-engine/src/authorizer.rs`
   `client_may` / `client_may_unified`, the per-ask `ReadData` answer. This is the
   **only** place that stops credential vending: a `ReadData = false` here makes
   Lakekeeper refuse the table load and vend **no** STS credential.

Both already resolve the SP5b switch and call the same
`unified_decide::covers(…, write=false)`; SP5c composes `table_read_restricted`
on top at both sites, gated on the new `iceberg-rest` switch being on for the
catalog.

### D. STS / storage credentials — no storage-layer change

There is no separate STS broker; Lakekeeper vends credentials via STS AssumeRole,
downscoped to the table location, with scope driven by the authorizer's
`ReadData`/`WriteData` answer (`crates/egret-catalog-engine/src/warehouse.rs`
`s3_storage_profile`, `sts-enabled: true`). Because the withheld-read deny forces
`ReadData = false` in `client_may` (hook C.2), Lakekeeper vends no credential for a
withheld table. Nothing else in the storage layer changes. Remote signing
(`/aws/s3/sign`) is off and the guard already refuses signer routes.

### E. Capability matrix — a second standalone table

Follow the SP5b precedent exactly. SP5b added `EMBEDDED_CATALOG_PRIVILEGES` as a
**standalone** const table in `crates/egret-perm-ir/src/matrix.rs`, deliberately
NOT a new `EpKind` and NOT rows in `ENTRIES`, so the Ranger/OPA golden output is
untouched. SP5c adds a second standalone table for `iceberg-rest`:

- `pub const ICEBERG_REST: &str = "iceberg-rest";`
- a table of `(Capability, Answer)` (extending the privilege-only shape, because
  SP5c must express rule enforceability, not just privileges):
  - privileges: `Browse`, `Read`, `Write` → `Native`; `Create`, `Manage` →
    `Missing(PrivilegeNotEnforceable)` (same as embedded-catalog; the catalog op
    mapping never asks for create/manage on the data path);
  - rules: `Mask` and `RowFilter` → **`Lowered`** to a whole-table withhold, with
    a `NotEnforced`/lowering reason ("masks and row filters cannot run at the
    storage layer; the table's `READ` is withheld instead");
  - `Withhold` (clearance) → `Native` (deny).
- accessors `iceberg_rest_support(cap)` / `iceberg_rest_gap(cap)` mirroring
  `embedded_catalog_support` / `embedded_catalog_privilege_gap`, consumed by
  `explain` so the UI can say what `iceberg-rest` enforces and why a mask becomes
  a table-withhold there.

`EpKind` / `ENTRIES` and the Ranger/OPA compilers are **not** touched →
byte-identical Ranger/OPA goldens, nothing re-blessed.

## Switch, migration and gate

- **Switch.** A per-catalog `iceberg-rest` rule-enforcement flag, default off,
  independent of `perm.catalog_grant_switch`. Representation (a column on the
  existing switch table vs. a sibling table) is a plan decision; it must be
  readable in one query on the catalog read hot path (reuse / extend
  `resolve_catalog_switch`).
- **Gate (`plan`).** Over authenticatable-client service accounts × the catalog's
  named objects (mirror SP5b's sample population — valid, non-revoked,
  non-expired clients only), the gate asserts:
  1. every GRANT read/write decision is identical to the SP5b unified decision
     (no grant regression), and
  2. enumerates the `(principal, table)` pairs that flip `allow → deny` because a
     rule applies and the principal is not exempt — the intended new denials, for
     human review.
  Unlike SP5b, divergence in (2) is expected and is **reported**, not refused;
  (1) regressing is a hard failure (refuse the switch).
- **Endpoints** (platform_admin only, 403 otherwise; mirror SP5b's
  `catalog_migrate_gate` shape):
  - `POST /api/control-plane/perm/iceberg-rest/{catalog}/plan` — the gate report
    (grants-identical check + the new-deny set), writes nothing.
  - `POST /api/control-plane/perm/iceberg-rest/{catalog}/switch` — turn rule
    enforcement on; refused if the grants-identical check fails.
  - `POST /api/control-plane/perm/iceberg-rest/{catalog}/undo` — turn it back off.
  - **No `finalize`.** SP5c only *adds* enforcement; it drops nothing, so there is
    no irreversible step. `undo` is the full reverse.

## Package decomposition (plan will refine)

| Task | Scope |
|---|---|
| **T1 — catalog snapshot + predicate** | extend `build_catalog_snapshot_via` to carry rule/withhold statements + the `classified` index; add `table_read_restricted`; unit tests (masked / filtered / withheld deny; exempt passes; unruled table passes; grant decisions unchanged). |
| **T2 — enforcement hooks** | compose `table_read_restricted` at the guard (`main.rs`) and the authorizer (`client_may`), gated on the new switch; prove credential vending stops for a withheld table via the `ReadData=false` path. |
| **T3 — switch + endpoints + gate** | the per-catalog `iceberg-rest` switch + migration; `plan`/`switch`/`undo` endpoints (`platform_admin`); the gate (grants-identical hard-fail + new-deny enumeration); gate tests with teeth. |
| **T4 — capability matrix + explain** | the standalone `iceberg-rest` matrix table + accessors; wire into `explain`; tests that mask/row-filter report `Lowered → table withhold` with a reason. |
| **T5 — verify + live-check** | OpenAPI regen if the surface changed; **byte-identical** Ranger + OPA goldens (nothing re-blessed); full gate (fmt/clippy/test on touched crates); a documented demo-stack live-check recipe (external Iceberg client: masked table → READ refused + no credential; normal table → reads). |

## Out of scope

- Governance-overview badge / DTO and any frontend for `iceberg-rest` (deferred).
- Active credential revocation on withhold (KI-315 staleness documented instead).
- Column-level masking at the catalog (impossible at the storage layer — a masked
  column withholds the whole table).
- Any change to the Ranger/OPA compilers or the IR securable schema (goldens stay
  byte-identical).

## Testing

- **Unit (perm-eval / perm-compile):** `table_read_restricted` on masked,
  row-filtered, withheld, exempt, and un-ruled tables; classification-selected
  rules resolve to the catalog's tagged tables; grant read/write decisions
  unchanged from SP5b.
- **Gate:** the `plan` grants-identical check has teeth (a grant regression is
  caught and refuses the switch); the new-deny enumeration lists exactly the ruled
  `(principal, table)` pairs, over authenticatable principals only.
- **Enforcement:** guard and authorizer both deny a withheld table; the authorizer
  path yields `ReadData=false` ⇒ no credential.
- **Byte-identical Ranger + OPA goldens**, nothing re-blessed (SP5c touches
  neither compiler).
- **Live-check (demo stack):** documented recipe, run before merge — an external
  Iceberg client reading a masked/filtered table is refused and gets no
  credential; a normal table still reads; grants still work.

## Known limitation

KI-315 credential staleness: an STS credential vended before a table is withheld
outlives the new rule by up to the token lifetime (default ~1h; Lakekeeper caches
half of it). Only **new** table loads fail closed. Documented, not revoked, in
SP5c.

## Key code anchors (verified on `aab83f678`, post-SP5b)

- `crates/egret-perm-compile/src/opa/build.rs` — `build_catalog_snapshot_via`
  (grants + owners only today, `classified: Vec::new()`).
- `crates/egret-perm-eval/src/` — `decide.rs` `candidates`, `restricts`,
  `decide_privilege`, `check_data` (deny/withhold wins); `values.rs` `row_filters`,
  `column_mask_choice`, `table_classes` / `column_classes`.
- `crates/egret-catalog-engine/src/unified_decide.rs` — `covers`,
  `resolve_catalog_switch`, `catalog_grant_source`, `CatalogSnapshotCache`
  (per-catalog switch `perm.catalog_grant_switch`, migration `0235`).
- `crates/egret-bin/src/main.rs` — `EgretCatalogGuard::check`, `CatalogDecider`
  (`covers` read path).
- `crates/egret-catalog-engine/src/authorizer.rs` — `client_may` /
  `client_may_unified` (`ReadData` answer drives credential vending).
- `crates/egret-catalog-engine/src/warehouse.rs` — `s3_storage_profile`
  (`sts-enabled`, scope follows the grant); `apply_vended_credential_lifetime`
  (KI-315).
- `crates/egret-perm-ir/src/matrix.rs` — `ENTRIES` / `EpKind` (untouched);
  `EMBEDDED_CATALOG_PRIVILEGES` / `embedded_catalog_support` (the standalone
  precedent to follow).
- `crates/egret-catalog-engine/src/catalog_migrate_gate.rs` — SP5b
  `plan`/`switch`/`undo`/`finalize` + `EquivalenceReport` (the endpoint/gate shape
  to mirror).

## Global constraints

- Chat Vietnamese; code/commits/docs English. Conventional Commits; `Closes #N`
  only in PR body. Co-author `Co-Authored-By: Claude Opus 4.8
  <noreply@anthropic.com>`. No secrets. Stage by explicit path (never
  `git add -A`; `target-sp5c/` is not gitignored).
- Branch `feat/gov-sp5c-iceberg-rest-ep` (worktree `../egret-sp5c`, off main
  `aab83f678`). `CARGO_TARGET_DIR=target-sp5c` on every cargo command. Throwaway
  Postgres; DB-gated tests MUST run, not skip.
- Migration numbers are a shared namespace — number provisionally above main's
  highest (`0236`), renumber above main AND open PRs at merge.
- **Byte-identical Ranger/OPA:** SP5c touches neither compiler nor the IR
  securable schema; re-run both mechanisms' goldens — zero diff, nothing
  re-blessed.
- **Grants unchanged:** the SP5b unified grant decision must be identical before
  and after SP5c; only rule-denies are added, and only when the `iceberg-rest`
  switch is on.
- Run `cargo fmt` + `cargo clippy -p <crate> --all-targets -- -D warnings` before
  each commit. `npm run gen:api` (never bare orval) if the API surface changes.
- Docs: a PR changing the API or a permission must update `egret-data/docs`.
