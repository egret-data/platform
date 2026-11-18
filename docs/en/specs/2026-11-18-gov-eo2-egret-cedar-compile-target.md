# EO2 — `egret-cedar` compile target + capability matrix + decision-equivalence differential

Status: approved design, 2026-11-18. Sub-project **EO2** of the egret-objects programme. Its umbrella is the EO0 decomposition spec ([2026-11-18](2026-11-18-gov-eo0-egret-objects-decomposition-design.md)); decisions EO-D1…EO-D8 there govern this document, and any change to them is made in EO0 first. EO1 ([plan](../plans/2026-11-18-gov-eo1-object-securables-vocab.md)) is merged: the pure `egret_object` module (the securable tree + the `classify` vocabulary partition) is in `egret-perm-ir` on `main`.

## Goal

Teach the governance IR to **compile the Cedar policy the control-plane PDP runs today**, add the `egret-cedar` enforcement point to the capability matrix, fold the classification/clearance gate into the `withhold` concept, and **prove the compiled policy is decision-equivalent** to today's `templates.rs`/`entities.rs` output over a corpus of (principal, object, action). Target-neutral and **zero behaviour change**: nothing is wired to the live PDP — that is EO3. (Mirrors governance SP1/SP2: a compile target plus a differential, no runtime switch.)

## Scope

In scope:

- A new `egret-cedar` crate that renders the `cedar_policy::PolicySet` the PDP compiles today from a neutral input loaded from the legacy control-plane tables (role-templated Cedar does not fit the data-plane `Statement`/`lower_*` IR, so it carries its own input + loader rather than being an `egret-perm-compile` backend).
- The `egret-cedar` capability-matrix entry in `egret-perm-ir/src/matrix.rs` (standalone, like `iceberg-rest`/`embedded-catalog` — not an `EpKind` row; egret has no engine or version axis).
- Folding `classification_policies` + object `tags` + the clearance `containsAny` check into `withhold` rules compiled for `egret-cedar` (EO-D4), kept **in EO2** (one differential covers grants + typed privileges + admin-graph forbids + clearance).
- A differential test proving decision-equivalence against today's PDP.

Out of scope (later sub-projects): switching `CedarPdp::load_inner` to consume the compiled PolicySet (EO3); folding `access_grants` into `perm.*` (EO4); making `notebook`/`workflow`/`alert` securables (EO-D6).

## Decisions (EO2-local; all consistent with EO0)

| # | Decision |
|---|---|
| EO2-D1 | **Emitter reuse, in a new `egret-cedar` crate.** The pure Cedar text formatters (`template_for_role`, `deny_template_for_role`, `classification_forbid`, and the admin-graph `forbid` builder currently inline in `load_inner`) move into a new `egret-cedar` crate; `egret-auth` depends on it and its PDP re-uses them, so behaviour is byte-identical. The new compiler (also in `egret-cedar`) **assembles the same template set** from a neutral input, using those formatters. What EO3 replaces is the *data source* (`load_inner`'s bespoke SQL → the compiler), not the Cedar text shape. Role-templated Cedar does **not** fit the data-plane `Statement`/`lower_*` IR (no `Path`), so `egret-cedar` carries its own small input + loader — it is not an `egret-perm-compile` backend beside ranger/opa. |
| EO2-D2 | **Differential shape and home.** The *compiled artifact* is the template `PolicySet` (per-role permit/forbid templates + admin-graph `forbid` + classification `forbid`s) plus the `role_names` map — exactly `PdpInner`. Grant→resource **links** stay request-time (`entities.rs`) and are applied to *both* sides by the same machinery, so the differential isolates the compiled set. Cedar-vs-Cedar over a seeded test DB: each generated state is seeded; the "today" set is built through the PDP's own `load_inner`, the "IR" set through the new compiler, from the *same* DB; `cedar_policy::Authorizer` answers both for every (principal, object, action) case; the test asserts identical `Decision.allow`. Corpus = seeded PRNG generator + deterministic regression anchors, mirroring `opa_differential.rs`. The differential lives in **`egret-auth/tests`** — the only crate that sees both the live PDP (the oracle) and `egret-cedar` — not in `egret-perm-compile/tests`. Both sides are Cedar in-process, so there is no external engine and no `EGRET_TEST_*` JVM gate; it is a normal test-DB test CI runs. |
| EO2-D3 | **Matrix placement.** `egret-cedar` is a standalone matrix target in `egret-perm-ir/src/matrix.rs` (`EGRET_CEDAR` const + privilege table + effect table + `egret_cedar_support(cap)` + `egret_cedar_gap(cap)`), leaving the Ranger/OPA `ENTRIES` untouched — exactly the `iceberg_rest_*` pattern. Object grants and typed privileges are **Native**; `withhold` (clearance) is **Native** (Cedar `forbid`); `mask` and `row filter` are **not applicable** (no columns/rows at this layer) → a new `ReasonCode::NotApplicable` so "not applicable" is distinguishable from "not measured"; any other combination → `Missing(NotMeasured)` → deny "not measured" (EO-D7). |
| EO2-D4 | **Action set from one catalog.** The compiler drives its action set from `egret_core::rbac::PERMISSIONS` + `ADMIN_GRAPH_ACTIONS` through `egret_object::classify`, the same catalog the PDP and EO1 read — no second action list. |
| EO2-D5 | **No new policy decisions.** EO2 reproduces today's policy *shape* only. Every allow/deny the compiled set produces must already be produced by the PDP; proving that is the differential's whole job. |

## Architecture

```text
 legacy control-plane tables       egret-cedar crate (new)              Artifact / proof
 roles, role_permissions,     ──►  load_inputs(db) ─► CompileInputs ─►  cedar_policy::PolicySet
 classification_policies, tags      compile(&inputs) via shared          (role templates + forbids)
   (source migrates to perm.*        text formatters                     + role_names
    at EO4, not here)                                            ──────► differential (in egret-auth):
                                                                         compile() vs load_inner(),
 egret-auth PDP ─ load_inner() ───────────────────────────────►         same decisions over a corpus
   (unchanged at runtime; now calls the shared formatters)
```

The shared text formatters (EO2-D1) are the single source of Cedar policy text. Today the PDP feeds them from SQL in `load_inner`; EO2 adds a second feeder — `egret-cedar`'s `compile(load_inputs(db))` — and the differential proves the two feeders yield decision-equivalent policy. The grant **source** is still the legacy tables (it migrates to `perm.*` at EO4); the policy **generator** is what EO3 switches (`load_inner` → `compile`). EO2 wires neither — it only adds the compiler, the matrix entry, and the proof.

### What the compiler emits (equivalent to the PDP today)

Mapped from the recon of `egret-auth/src/pdp/`:

| PDP today (`load_inner`) | `egret-cedar` compiler | Output policy |
|---|---|---|
| per-role permit/forbid templates (`roles`+`role_permissions`, effective perms) | `RoleInput{id,name,scope,effective_perms}` from `load_inputs` → `compile` | `template_for_role` / `deny_template_for_role`, PolicyId `role:{id}` / `role-deny:{id}` (every effective perm listed, same as today) |
| `ADMIN_GRAPH_ACTIONS` → one static forbid | `admin_graph_forbid()` (shared) | the same `forbid(… action in [admin-graph] …) unless is_platform_admin` |
| `classification_policies` + `tags` + clearance `containsAny` | `ClassificationInput{tag_id,key,required_roles,required_groups}` → `compile` (both-empty skipped, malformed skipped, as today) | `classification_forbid(tag_key, required_roles, required_groups)`, PolicyId `classification:{tag_id}` |
| per-request grant links (`entities::link_grants`) | **unchanged, request-time**; applied to both sides in the differential | template links `grant-{i}` |
| request context `{is_platform_admin, roles (allow-grants only), groups}` | **unchanged, request-time**; assembled once, fed to both sides | unchanged context JSON |

`egret_object` (EO1) is not needed to reproduce the templates — `template_for_role` already lists every effective perm as an action regardless of bucket. EO1's `classify`/`ObjectKind` instead drive **test coverage** here: the differential's action corpus and the matrix tests iterate `rbac::PERMISSIONS` through `classify` so object-data, typed, and admin-graph buckets are all exercised, and object ids use the `"{kind}:{uuid}"` scheme EO1's `EgretSecurable` already matches. The object-data-vs-typed grant **split** becomes load-bearing at EO4 (when the grant source moves to `perm.*`), not here.

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

- `crates/egret-cedar/` (new workspace member) — `src/text.rs` (the pure formatters moved from `egret-auth/src/pdp/templates.rs`, plus `admin_graph_forbid()` extracted from `load_inner`, plus the PolicyId helpers); `src/compile.rs` (`CompileInputs`/`RoleInput`/`ClassificationInput`, `compile(&CompileInputs) -> Compiled{policy_set, role_names}`); `src/load.rs` (`load_inputs(db) -> CompileInputs`, reading the legacy tables). `Cargo.toml` deps: `cedar-policy`, `egret-core`, `egret-metastore`, `uuid`.
- `egret-auth` — depends on `egret-cedar`; `pdp/templates.rs` re-exports the moved formatters (so `entities.rs`'s call sites are unchanged) and `load_inner` calls `egret_cedar::text::admin_graph_forbid()`. Runtime behaviour byte-identical. A `#[doc(hidden)]` test accessor exposes `load_inner`'s `(PolicySet, RoleNames)` for the differential.
- `egret-perm-ir/src/statement.rs` — add `ReasonCode::NotApplicable`. `egret-perm-ir/src/matrix.rs` — add the `EGRET_CEDAR` entry + `egret_cedar_support`/`gap` (+ tests). `ENTRIES` untouched.
- `egret-auth/tests/cedar_differential.rs` (new) — the differential, mirroring `opa_differential.rs` (seeded state generator + anchors + the Cedar-vs-Cedar comparison), with a teeth test proving a seeded mutation is caught.
- Compiler golden assertions live in `egret-cedar`'s own unit tests.

## Docs

No user-visible feature, UI, API, config key, or permission changes in EO2 (library + tests only). Per the repo docs rule, **no `egret-data/docs` PR** is required.
