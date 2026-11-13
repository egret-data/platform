# Governance layer SP3 — enforcement points replace profiles (design)

Status: design, 2026-11-12. Sub-project 3 of the governance-layer programme. It stays inside the
umbrella [concepts spec](2026-11-02-governance-layer-concepts.md) (D1–D10); any concept change goes
there first. Prior sub-projects: SP1 neutral IR (#833), SP2 Trino OPA (#836/#838/#839/#844).

## Goal

Turn today's "profile" (a Ranger authz service) into a first-class **enforcement point (EP)** and
remove the last non-model enforcement input — hand-written operator policies — by typing what they
express. Unify clearance, role principals and the implicit workspace grant into the common model so
that groups-only clearance is enforced on every engine. No new runtime enforcement switch; the move
is a per-EP, in-place, reversible data migration.

Acceptance (concepts table, row 3): every EP on the demo stack switched to the new model; no
hand-written policies; groups-only clearance works on Trino and StarRocks.

## Scope boundary (confirmed with the user)

- SP3 keeps the **data-grant vocabulary as it is today** — `perm.grants` read/write and `exclude→deny`.
  The full privilege set (`BROWSE`/`CREATE`/`MANAGE`/`OWNER`, D4), the catalog-grant merge and the
  `iceberg-rest` EP are **SP5** (one grant model). SP3 does not introduce new data privileges.
- SP3 works the **EP (enforcement-output) side**: kill manual policies, unify clearance/role/workspace
  representation, re-home catalog scope and engine privileges onto the EP, and give the capability
  matrix an image-patch-level key.
- No new runtime "old model / new model" switch. The only per-EP switch stays `mechanism`
  (ranger/opa, SP2). SP3's migration is an **in-place data migration**, reversible through
  `perm.migrated_from` while open, gated per EP by "no unconvertible operator policy remains".

## Current state (origin/main inventory, 2026-11-12)

The EP concept is already partly present, which keeps SP3 bounded:
- `policy.authz_services` (`ServiceRow`) already has `type` (engine), `mechanism` (ranger/opa, SP2b
  mig 0213) and `origin` (generated/manual, mig 0155). No per-EP model/version field.
- `perm.engine_grants` (mig 0195) already holds **eight typed engine capabilities** via the
  `CAPABILITIES` catalogue in `egret-perm-compile/src/engine_capabilities.rs` (no Rust enum; an
  8-string CHECK list kept equal to the catalogue by a test). These are already "typed engine
  privileges".
- Catalog scope is derived from `registry.engine_catalog_attachments` (mig 0147), surfaced in the IR
  as `EpInput.catalogs`.
- `perm.principal_kind` enum has `role`; clearance lives separately in `public.classification_policies`
  (`required_roles text[]`, `required_groups uuid[]`), loaded by `ir_load.rs`.
- Version is **per engine** (`engine_mgmt.engines.ranger_plugin_version`), not per EP; the IR carries
  it as `BoundEngine.version`. There is **no** image-patch-level (egret.N) key yet.
- Hand-written operator policies: `policy.authz_policies` `managed_by='operator'`, raw Ranger JSON in
  `policy_jsonb`, typed read-side by `egret_ranger_eval::model::RangerPolicy`.

So SP3 is "finish the EP, kill manual policies, unify clearance/role/workspace", not "build the EP".

## Packages

SP3 ships as four packages, each its own plan and PR(s), like SP2a/2b. Each is independently
testable and merges on its own green CI and the user's word.

### SP3a — EP framing, per-EP version, capability matrix image key

**What:**
- "Profile" becomes **"enforcement point"** on the **surface only** — UI, API and the IR already use
  EP. The table stays `policy.authz_services`; no core-table rename migration.
- The capability matrix key becomes per-EP: `(engine, mechanism, plugin_version, egret_image_level)`.
  `egret_image_level` is the running image's egret patch level (e.g. `egret.5`), read from the
  engine's applied image (the provisioner records it; the StarRocks posture package added the image
  fields). Ranger/official engines carry a null image level (matrix falls back to the version key).
- **Mixed-engine EP (decision ①):** an EP may serve several engines of different versions/image
  levels. Its effective capability is the **least-capable across its engines** (fail-closed, D7); the
  EP surfaces which engine caps each lowered capability. No refusal of a mixed set.

**What the key buys:** the matrix expresses capabilities that depend on the StarRocks fork image.
A capability the fork patches enable — e.g. a column **mask** or **row filter** that the fork image
enforces through every protocol (patches 0002/0003/0004) but an official/old image does not — is
gated on `egret_image_level`: on an engine below the enabling level the matrix lowers it to deny
(fail-closed, D7), on a fork image at or above it the matrix enforces it. The whole corpus and the
demo stack run on the current fork image, so their compiled output is **byte-identical**; the change
only bites an engine on an image that cannot enforce the capability (which the "fork image only" rule
already forbids).

**Behaviour change:** none to compiled output on fork images; the matrix gains a key dimension and the
`NotEnforced` reasons may now cite an image level.

**Done when:** the matrix is keyed by image level; a StarRocks EP on an image below the enabling level
lowers a mask/row-filter capability to deny while an egret.4+ EP enforces it; UI/API say "enforcement
point"; Ranger JSON is byte-identical on the corpus and demo stack (all on the current fork image).

### SP3b — Remove hand-written operator policies (Migrate)

**What:**
- A **Migrate** step classifies each `authz_policies` `managed_by='operator'` row (read through
  `RangerPolicy`) into the model:
  - access allow/deny items → `perm.grants` (allow / deny);
  - data-mask items → `perm.rules` mask;
  - row-filter items → `perm.rules` row filter;
  - a shape that matches an existing engine capability (e.g. impersonate, session property) →
    `perm.engine_grants` (the typed capability);
  - anything else (hand-written wildcards with priorities, manual tag policies, shapes no capability
    covers) → **unconvertible**: left in place, listed, and the EP **cannot complete the migration**
    while it exists.
- **No speculative typed privileges (decision ②).** The classifier maps only known shapes. A new
  typed engine privilege is added only if the demo stack's real operator policies show a recurring
  convertible shape no capability covers — measured when the plan is written, not invented now.
- Every conversion is a migration with provenance (`perm.migrated_from`) and undo while the EP's
  migration is open.

**Done when:** every convertible operator policy on the demo stack is a grant/rule/engine privilege;
unconvertible ones are listed per EP; an EP with no unconvertible policy reports "ready to switch";
`explain` gives the same allow/deny answers before and after conversion for that EP.

### SP3c — Unify clearance, role principals, implicit workspace grant

**What:**
- **Clearance → `withhold` rule.** Migrate each `classification_policies` row to a `perm.rules` row:
  selector = that classification, effect = `withhold`, exemptions = its `required_roles` (via the role
  groups below) and `required_groups`. Enforcement then comes from the IR's withhold, which resolves
  exemptions like any principal set — so **groups-only clearance is enforced on Trino and StarRocks**,
  fixing the Ranger gap where it compiled to nothing.
- **Role principals → `role:<name> holders` groups.** For each RBAC role referenced by a data grant,
  rule exemption or engine grant, a managed group `role:<name> holders` whose membership mirrors the
  role's holders; migrate every `role` ref in `grant_principals`, `rule_exemptions` and
  `engine_grant_principals` to that group. Per the concepts, the group is transitional: once SP3c
  ships, no new grant may target a `role:<name> holders` group.
- **Implicit workspace grant → explicit.** The catalog-wide read a workspace member gets today
  (`registry.catalog_workspaces` + the implicit grant) becomes an explicit `READ` grant to
  `ws:<slug> members` on the bound catalogs, stored once.
- **Decision ③:** `role:<name> holders` and `ws:<slug> members` are both managed groups maintained by
  the **same mechanism** (the existing ws-members machinery, reused for role-holders).

**Done when:** `classification_policies`, `role` principal refs and the implicit grant are gone from
the enforcement input; groups-only clearance denies on both engines in a live check; `explain` is
unchanged for every affected principal.

### SP3d — Per-EP migration gate, switch the demo stack, live checks

**What:**
- The per-EP gate: an EP's migration completes only when no unconvertible operator policy remains
  (SP3b) and its clearance/role/workspace input is migrated (SP3c). The gate is a read over the EP's
  own rows, re-checked on each compile (like the OPA switch's `stored_state_refusals`).
- Switch every EP on the demo stack to the new model and live-check each: an unprivileged user
  querying the real engine (Trino and StarRocks), groups-only clearance denying, a converted
  mask/row-filter still masking/filtering.
- After the demo stack is switched, the old inputs are removed per the concepts' "no long-term
  backward compatibility" once every EP has switched (the delete itself may be a short follow-up).

**Done when:** every demo-stack EP is on the new model; live checks pass; the migration is reversible
until the switch is closed.

## Architecture notes

- No change to the SP1 pipeline (Store → IR → target). SP3 changes what the Store feeds the IR
  (grants/rules/engine-grants instead of clearance tables, role groups instead of role refs,
  explicit instead of implicit workspace grants) and adds the image-level dimension to the matrix.
- `explain` evaluates the IR, so one evaluator still serves every EP; the differential oracles
  (`ranger-differential`, `opa-differential`) stay the equivalence check.
- Compile triggers, `lock_profile_rows_in_tx` and in-transaction recompiles are unchanged.

## Errors (fail closed, per concepts D7)

- A capability the matrix does not know for an image level → lowered to deny, reason "not measured".
- An unconvertible operator policy → the EP cannot complete the migration; it keeps serving its last
  good artifact; the reason lists the policy.
- A role whose holders cannot be mirrored into its group → the role group is empty; its members are in
  no allow and still in every deny; the reason shows on the role.

## Verification

1. **IR / matrix unit tests:** image-level key lowering; clearance-as-withhold group resolution; role
   group expansion; implicit-grant equivalence.
2. **Byte-identical (SP3a):** Ranger JSON unchanged on the corpus and demo profiles after the matrix
   key change.
3. **`explain` equivalence** before/after each conversion (SP3b, SP3c) over a sample of principals ×
   objects, per EP.
4. **Differential** against real enforcers (Ranger 2.5/2.8 jars; OPA vs Ranger) stays green.
5. **Live checks** for every behaviour-changing PR: own stack, unprivileged user on a real engine,
   groups-only clearance on both Trino and StarRocks.

## Out of scope (SP3)

- The full privilege vocabulary (`BROWSE`/`CREATE`/`MANAGE`/`OWNER`/`OWNER` transfer), the catalog-grant
  merge and the `iceberg-rest` EP — these are SP5.
- Service accounts and catalog clients — SP4.
- egret BI/object permissions, lineage, Spark, OPA for StarRocks — out of the whole programme.

## Package order and acceptance

| Pkg | Scope | Done when |
|---|---|---|
| SP3a | EP framing + per-EP version + matrix image-level key | matrix keyed by egret.N; byte-identical Ranger; UI/API say "enforcement point" |
| SP3b | Migrate removes hand-written operator policies | convertible policies typed; unconvertible listed per EP; `explain` unchanged per EP |
| SP3c | clearance→withhold, role→groups, implicit→explicit | the three old inputs gone; groups-only clearance denies on Trino + StarRocks live; `explain` unchanged |
| SP3d | per-EP gate + switch demo stack + live checks | every demo-stack EP on the new model; live checks pass; reversible until closed |

Order SP3a → SP3b → SP3c → SP3d. SP3b and SP3c both depend on SP3a's framing; SP3d depends on both.
