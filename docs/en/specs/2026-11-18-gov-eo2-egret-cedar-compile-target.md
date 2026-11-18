# EO2 — `egret-cedar` compile target + capability matrix + decision-equivalence differential

Status: approved design, 2026-11-18. Sub-project **EO2** of the egret-objects programme. Its umbrella is the EO0 decomposition spec ([2026-11-18](2026-11-18-gov-eo0-egret-objects-decomposition-design.md)); decisions EO-D1…EO-D8 there govern this document, and any change to them is made in EO0 first. EO1 ([plan](../plans/2026-11-18-gov-eo1-object-securables-vocab.md)) is merged: the pure `egret_object` module (the securable tree + the `classify` vocabulary partition) is in `egret-perm-ir` on `main`.

## Goal

Teach the governance IR to **compile the Cedar policy the control-plane PDP runs today**, add the `egret-cedar` enforcement point to the capability matrix, fold the classification/clearance gate into the `withhold` concept, and **prove the compiled policy is decision-equivalent** to today's `templates.rs`/`entities.rs` output over a corpus of (principal, object, action). Target-neutral and **zero behaviour change**: nothing is wired to the live PDP — that is EO3. (Mirrors governance SP1/SP2: a compile target plus a differential, no runtime switch.)

## Scope

In scope:

- A new compile backend `egret-perm-compile/src/cedar/` that renders a `cedar_policy::PolicySet` from the perm model via the existing `ir_load` + `lower_*` seam.
- The `egret-cedar` capability-matrix entry in `egret-perm-ir/src/matrix.rs` (standalone, like `iceberg-rest`/`embedded-catalog` — not an `EpKind` row; egret has no engine or version axis).
- Folding `classification_policies` + object `tags` + the clearance `containsAny` check into `withhold` rules compiled for `egret-cedar` (EO-D4), kept **in EO2** (one differential covers grants + typed privileges + admin-graph forbids + clearance).
- A differential test proving decision-equivalence against today's PDP.

Out of scope (later sub-projects): switching `CedarPdp::load_inner` to consume the compiled PolicySet (EO3); folding `access_grants` into `perm.*` (EO4); making `notebook`/`workflow`/`alert` securables (EO-D6).

## Decisions (EO2-local; all consistent with EO0)

| # | Decision |
|---|---|
| EO2-D1 | **Emitter reuse.** The compiler reuses the PDP's *pure Cedar text formatters* (`template_for_role`, `deny_template_for_role`, `classification_forbid`, and the entity/uid builders in `entities.rs`), relocated to a shared home both `egret-auth` and `egret-perm-compile` depend on. The compiler **selects and links** policies from the lowered IR, not from the bespoke DB queries. What EO3 replaces is therefore the *data source* (bespoke SQL → IR), not the Cedar text shape. DRY; the differential exercises the IR lowering/linking, not re-typed string templates. |
| EO2-D2 | **Differential shape.** Cedar-vs-Cedar over a seeded test DB: each generated state is seeded into the DB; the "today" `PolicySet` is built through the PDP's own load path and the "IR" `PolicySet` through the new compiler, from the *same* state; `cedar_policy::Authorizer` answers both sides for every (principal, object, action) case; the test asserts identical `Decision.allow`. Corpus = seeded PRNG generator + deterministic regression anchors, mirroring `opa_differential.rs`. Both oracles are Cedar in-process, so there is no external engine and no `EGRET_TEST_*` JVM gate — only a test DB. |
| EO2-D3 | **Matrix placement.** `egret-cedar` is a standalone matrix target (`EGRET_CEDAR` const + privilege table + effect table + `egret_cedar_support(cap)` + `egret_cedar_gap(cap)`), leaving the Ranger/OPA `ENTRIES` untouched — exactly the `iceberg_rest_*` pattern. Object grants and typed privileges are **Native**; `withhold` (clearance) is **Native** (Cedar `forbid`); `mask` and `row filter` are **not applicable** (no columns/rows at this layer); any unmeasured combination → `Missing(NotMeasured)` → deny "not measured" (EO-D7). |
| EO2-D4 | **Action set from one catalog.** The compiler drives its action set from `egret_core::rbac::PERMISSIONS` + `ADMIN_GRAPH_ACTIONS` through `egret_object::classify`, the same catalog the PDP and EO1 read — no second action list. |
| EO2-D5 | **No new policy decisions.** EO2 reproduces today's policy *shape* only. Every allow/deny the compiled set produces must already be produced by the PDP; proving that is the differential's whole job. |

## Architecture

```text
 perm model + egret-object        Neutral IR (lower_*)        egret-cedar backend         Artifact
 securables, grants, rules,  ──►   Lowered{ statements,  ──►   select + link policies ──►  cedar_policy::
 classification, EP config         lowerings }                 via shared formatters       PolicySet (in memory)
   (ir_load::load_ep + inputs)     capability matrix           (EO2-D1)                    + golden for tests
                                        ▲
                                  egret-cedar matrix entry
```

The shared formatters (EO2-D1) are the single source of Cedar policy text. Today the PDP feeds them from SQL (`load_inner`); EO2 adds a second feeder — the IR — and the differential proves the two feeders yield decision-equivalent policy. EO3 will delete the SQL feeder.

### What the compiler emits (equivalent to the PDP today)

Mapped from the recon of `egret-auth/src/pdp/`:

| PDP today | egret-cedar compiler input | Output policy |
|---|---|---|
| per-role permit/forbid templates (`roles`+`role_permissions`, effective-perms) | role statements lowered from the IR; actions partitioned by `classify` (object-grant vs typed) | `template_for_role` / `deny_template_for_role`, PolicyId `role:{id}` / `role-deny:{id}` |
| per-request grant links (`entities::link_grants`) | lowered grant statements (allow/deny, platform/workspace/object scope) | template links `grant-{i}`, slots principal=user, resource=scope uid |
| `ADMIN_GRAPH_ACTIONS` → one static forbid | `classify` → `AdminGraphForbid` | the same `forbid(… action in [admin-graph] …) unless is_platform_admin` |
| `classification_policies` + object `tags` + clearance `containsAny` | `withhold` rules lowered for `egret-cedar` (EO-D4) | `classification_forbid(tag_key, required_roles, required_groups)`, PolicyId `classification:{tag_id}` |
| request context `{is_platform_admin, roles (allow-grants only), groups}` | assembled from the same subject/grants inputs | unchanged context JSON |
| entity scheme `User/Group/Role/Platform/Workspace/Folder/Object"{kind}:{id}"` | `egret_object::EgretSecurable` ids already mesh with this scheme | same `Entities` |

### Capability matrix entry

```text
EGRET_CEDAR = "egret-cedar"
  EnforcePrivilege(Read|Write|Create|Manage|Browse) → Native     (object grants + typed privileges)
  WithholdByClassification                          → Native     (Cedar forbid)
  ClassificationSelector(Mask | RowFilter)          → NotApplicable  (no columns/rows at this layer)
  <anything else>                                   → Missing(NotMeasured) → deny "not measured"
```

(Exact `Capability`/`Answer` variants follow `iceberg_rest_*`; `NotApplicable` is rendered through the existing `Missing`/`Support` machinery with a reason that reads "not applicable to egret objects", not "not measured", so the two causes stay distinguishable.)

## Verification

1. **Matrix unit tests** — `egret_cedar_support`/`gap` answer each capability with the table above; mask/row-filter report not-applicable; unknown → not-measured.
2. **Compiler unit tests** — golden `PolicySet` (serialised) for a small fixed fixture: role templates, grant links, admin-graph forbid, classification forbid all present with the expected PolicyIds and shapes.
3. **Decision-equivalence differential** (the core gate, EO2-D2) — over the seeded corpus + anchors, the IR-compiled `PolicySet` returns the same `Decision.allow` as the PDP-built `PolicySet` for every (principal, object, action), classification/clearance cases included. An allow where the PDP denies is never explainable (hard fail); the reproducer prints the seed.
4. **Teeth** — a seeded mutation (e.g. drop the admin-graph forbid, or widen one role template) makes the differential fail, proving it is not vacuous.
5. **No behaviour change** — the live `CedarPdp` is untouched; no migration; the only runtime-reachable change is additive library code behind no switch.

## Errors (fail closed, per EO-D7)

- A compile of the `egret-cedar` target fails → the compiler returns an error; nothing is produced; no partial PolicySet. (The live PDP is not involved in EO2.)
- The matrix does not know a combination → `Missing(NotMeasured)` → deny "not measured".
- A principal cannot be resolved → left out of every allow, still covered by every deny.
- Equivalence not proven → the differential fails the build; EO2 does not ship until it is green.

## Files (indicative; the plan fixes exact paths)

- Relocate the pure Cedar formatters out of `egret-auth/src/pdp/{templates,entities}.rs` into a shared module both crates use (EO2-D1); `egret-auth` keeps its SQL load path, now calling the shared formatters.
- `egret-perm-ir/src/matrix.rs` — add the `EGRET_CEDAR` entry + `egret_cedar_support`/`gap` (+ tests).
- `egret-perm-compile/src/cedar/` (new) — the backend: `mod.rs` (compile entry), policy selection + linking from the lowered IR, render to `PolicySet`; `Cargo.toml` gains the `cedar-policy` dep.
- `egret-perm-compile/tests/cedar_differential.rs` (new) — the differential, mirroring `opa_differential.rs` (seeded state/question generator + anchors + the Cedar-vs-Cedar comparison).
- Compiler golden fixtures under `egret-perm-compile/tests/` for test 2.

## Docs

No user-visible feature, UI, API, config key, or permission changes in EO2 (library + tests only). Per the repo docs rule, **no `egret-data/docs` PR** is required.
