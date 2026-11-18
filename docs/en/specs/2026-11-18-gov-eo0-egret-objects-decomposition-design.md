# Permissions for egret objects — decomposition (sub-project EO0)

Status: approved decomposition, 2026-11-18. This is the umbrella spec for the egret-objects programme (EO0–EO4), the follow-on to the governance-layer programme (sub-projects 0–5, now complete). Each sub-project (EO1–EO4) gets its own spec and plan that must stay inside this document; a sub-project that needs to change a decision here changes this document first.

This document **amends** the governance-layer concepts spec ([2026-11-02](2026-11-02-governance-layer-concepts.md)). That spec's **D1** reads: *"Securables are data and data-plane objects only. egret objects … keep the Cedar RBAC they have today."* EO0 supersedes it: egret objects become securables in the same common grant model, and Cedar becomes an enforcement point under that model (see EO-D1 below). The concepts-spec rows "Cedar RBAC … unchanged" and "`analytics.app_grants` … out of scope" are reopened here.

## Why

After the governance-layer programme, egret has **two disjoint permission stacks**:

- **Cedar** (`egret-auth/pdp`) is the control-plane PDP for egret's own objects and routes. It already **compiles policy from the database** — one Cedar template per role from `roles`/`role_permissions`, per-request template links from `access_grants`, `forbid` policies from `classification_policies`, hot-reloaded through `arc_swap` on `RolesChanged`. There are no hand-written Cedar files. Its action set is `egret_core::rbac::PERMISSIONS` (~60 keys).
- **The governance layer** (`egret-perm*`) is the data-plane model: a target-neutral IR, an enforcement-point abstraction, a capability matrix, compilers to Ranger and OPA, and the embedded-catalog `iceberg-rest` EP.

The two never reference each other. egret objects (dashboard, chart, dataset, folder, saved_query, app, data_api_endpoint) are Cedar resources with `{id, created_by, folder_id/workspace, access_grants}` — already close to securables, but governed by a parallel grant store, a parallel vocabulary, and a parallel compiler.

The goal is one grant model, in the spirit of the governance layer: egret objects become securables in the common tree; the common vocabulary maps onto the object-data permissions; the neutral IR gains an `egret-cedar` target that **compiles** the Cedar policy Cedar runs today; and the `access_grants` store folds into the one grant store `perm.*`. Cedar stays the runtime PDP — we replace what feeds it, not the engine.

## Decisions

| # | Decision |
|---|---|
| EO-D1 | egret objects are securables in the common model. Cedar is an **enforcement point** (`egret-cedar`) under that model, not a separate stack. |
| EO-D2 | Cedar keeps its runtime role: the PDP is still `cedar_policy::Authorizer::is_authorized`. We replace its **policy source** (bespoke templates → IR-compiled Cedar policy) and later its **grant source** (`access_grants` → `perm.*`). |
| EO-D3 | The common vocabulary `BROWSE/READ/WRITE/CREATE/MANAGE/OWNER` applies to **object-data grants**. Everything else in `rbac::PERMISSIONS` — engine/catalog/storage/secret/tag/policy/workspace/user/group/role/workflow/alert/service_account/query capabilities, plus object verbs that are not data access (`dashboard:certify`, `data_api_endpoint:invoke`) — stays as **typed privileges on the Cedar EP**, the way engine-specific privileges live on an engine EP. Entitlements stay typed. |
| EO-D4 | Cedar's classification/clearance gate (`classification_policies` + object `tags` + the clearance `containsAny` check) converges onto the common **rule** concept (the `withhold` effect), compiled for the `egret-cedar` EP. The `forbid`-wins semantics are preserved. |
| EO-D5 | The grant store folds into `perm.*` (one grant store), as sub-project SP5b did for the catalog: a per-EP **reversible** switch, gated by `explain`-equivalence over a sample of principals × objects × actions, with provenance in `perm.migrated_from` and undo while the switch is open. |
| EO-D6 | Scope is the seven object kinds that are securables today: `dashboard`, `chart`, `dataset`, `folder`, `saved_query`, `app`, `data_api_endpoint`. Making `notebook`, `workflow` and `alert` into securables is deferred to a later sub-project; each needs its own owner/parent/share model first. |
| EO-D7 | Fail closed. An EP that cannot enforce something denies and says why (capability matrix). Deny always wins; no priorities; no exclude lists — Cedar's `forbid`-wins already matches. |
| EO-D8 | Order of sub-projects: EO0 → EO1 → EO2 → EO3 → EO4. Each ships working, verifiable software and changes runtime behaviour only behind a reversible switch. |

## Concepts

### egret-object securable

The tree `Platform → Workspace → Folder* → Object{kind}`. It is **looser** than the data tree `catalog → schema → table → column`: folders nest, some objects live under a cross-schema folder set (`analytics.folders`, used by `data_api_endpoint`), and some objects are not foldered at all (they hang directly under their workspace). The IR securable model must generalise to this shape.

Every object securable has a stable `id`, exactly one `OWNER` (its `created_by`, falling back to the workspace creator, then the platform-admins group), a parent (its folder, else its workspace), and grants in the common store. Grants inherit down: a folder grant reaches its contents; a workspace grant reaches its objects — exactly today's Cedar cascade, re-expressed.

### Vocabulary mapping (EO-D3)

Two buckets:

| Bucket | `rbac::PERMISSIONS` examples | In the model |
|---|---|---|
| Object-data grants | `dashboard:read/manage`, `chart:read/manage`, `dataset:read/manage`, `saved_query:read/manage`, `app:read/write/manage`, `data_api_endpoint:read/manage` | common grants: `read→READ`, `write→WRITE`, `manage→MANAGE`, create-at-parent→`CREATE`, `created_by→OWNER`; `BROWSE` implied by any of these |
| Typed Cedar-EP privileges (entitlements) | `engine:*`, `catalog:*`, `storage:*`, `secret:*`, `tag:*`, `policy:*`, `workspace:*`, `user:*`, `group:*`, `role:*`, `workflow:*`, `alert:*`, `service_account:*`, `query:run`, and object verbs that are not data access (`dashboard:certify`, `data_api_endpoint:invoke`) | typed privileges on the `egret-cedar` EP; not securable grants |

The non-grantable admin-graph forbids (`ADMIN_GRAPH_ACTIONS`: `role:*`, `user:assign_role`, `user:assign_platform_role`, `group:manage_members`, `audit:read`) and the request-context ABAC (`is_platform_admin`, `roles`, `groups`) stay as typed policy on the EP — they are not expressible as grants and must not be flattened into any.

### `egret-cedar` enforcement point

A new EP kind `egret-cedar` (no version axis — egret is its own runtime). It holds its catalog-equivalent scope (platform + the workspaces/objects it governs), its typed privileges (the second bucket above), and its compile state. Its delivery channel is the running `CedarPdp`: egret is both the compiler and the PDP, so "delivery" is an in-process `PolicySet` swap (`arc_swap`), not a download.

### Capability matrix for `egret-cedar`

Per the EP kind: object grants and the typed privileges are **Native** (Cedar enforces them directly); `withhold` (clearance) is **Native** via `forbid`. `mask` and `row filter` have no meaning on egret objects (there are no columns or rows to rewrite at this layer) — they are **not applicable**, not lowered. Any unmeasured combination is treated as missing → deny with reason "not measured" (EO-D7).

### Grant store (EO-D5)

Today `access_grants` is Cedar's own grant table (direct user grants + group grants via `user_groups`, scoped to platform / workspace / object, with effect and expiry). EO4 folds it into the `perm.*` one grant store, so a single store answers both data-plane and egret-object grants, reusing SP5b's reversible switch, equivalence gate, and `perm.migrated_from` provenance.

## Architecture

```text
 Store (source of truth)        Neutral IR                   Compiler per EP        Delivery to the EP
 securables, principals,  ──►   per-EP statement set   ──►   ranger(trino@2.5)  ──►  Ranger download
 grants, rules, EP config       names resolved, scoped       opa(trino)         ──►  OPA decision API
   + egret-object securables     to the EP's scope,           iceberg-rest       ──►  catalog guard + STS
   + egret-cedar EP config       capability matrix            egret-cedar        ──►  in-process PolicySet
                                       ▲                                              swap (arc_swap), run by
                                capability matrix                                     CedarPdp::is_authorized
```

1. **Store → IR**: the IR gains the egret-object securable tree and the `egret-cedar` EP. Group expansion, inheritance, deny-wins and the capability matrix are the same machinery the data-plane EPs use.
2. **IR → `egret-cedar`**: a new compiler target that emits the Cedar `PolicySet` (templates + links + classification `forbid` + admin-graph `forbid`) that `templates.rs`/`entities.rs` build today — shape only, no new policy decisions.
3. **Delivery**: `CedarPdp` loads the IR-compiled `PolicySet` instead of building its own, keeping the `arc_swap` hot-reload. `explain` evaluates the same IR, so one evaluator answers "who can see this dashboard?" the way it answers "who can read this table?".

## Where today's Cedar mechanisms go

| Today | In the new model | Sub-project |
|---|---|---|
| `roles`, `role_permissions` (one Cedar template per role) | typed Cedar-EP privileges + object-grant templates, compiled from the IR | EO1, EO2 |
| `access_grants` (allow/deny, platform/workspace/object, expiry) | common grants in `perm.*`; `allow→grant`, `deny→deny` | EO4 |
| per-request template links (`entities::link_grants`) | IR-compiled links for the request's securable scope | EO2, EO3 |
| `classification_policies` + object `tags` + clearance `containsAny` | `withhold` rules compiled for the `egret-cedar` EP | EO2 |
| admin-graph `forbid` (`ADMIN_GRAPH_ACTIONS`) | typed EP policy, kept verbatim as compiler output | EO2 |
| request-context ABAC (`is_platform_admin`, `roles`, `groups`) | typed EP policy; context assembled unchanged | EO2, EO3 |
| `CedarPdp::load_inner` bespoke generation | consumes the IR-compiled `PolicySet` behind a reversible switch | EO3 |
| bespoke template generation vs. runtime engine | engine unchanged (`cedar_policy::Authorizer`); only the source changes | EO2, EO3 |
| `notebook`, `workflow`, `alert` (workspace-scoped, not securables) | out of scope here; a later sub-project makes them securables | later |

## The sub-projects

- **EO0 — this spec.** Decomposition, decisions EO-D1…EO-D8, sub-project list and order.
- **EO1 — Object securables in the model + vocabulary mapping.** Represent the `Platform → Workspace → Folder* → Object{kind}` tree in the perm model and the neutral IR; map the object-data permissions onto `BROWSE/READ/WRITE/CREATE/MANAGE/OWNER`; classify every `rbac::PERMISSIONS` key as a securable grant or a typed Cedar-EP privilege (EO-D3). Target-neutral, **zero behaviour change** — nothing wired to the runtime yet. (Mirrors SP1.)
- **EO2 — `egret-cedar` compile target + capability matrix.** Teach `egret-perm-ir`/`egret-perm-compile` to emit the Cedar `PolicySet` for the object model, add the `egret-cedar` EP kind and its matrix, and fold the classification gate into `withhold` rules. Prove the compiled policy is **decision-equivalent** to today's `templates.rs`/`entities.rs` output with a differential harness over a corpus of (principal, object, action) — the Cedar analogue of the Ranger/OPA differentials. Still not wired to the runtime. (Mirrors SP1/SP2 compile + differential.)
- **EO3 — `CedarPdp` consumes IR-compiled policy.** Switch `CedarPdp::load_inner` to load the IR-compiled `PolicySet`, behind a **reversible per-EP switch**, gated by `explain`-equivalence, keeping the `arc_swap` hot-reload and the `AuditingPdp` decorator. This is the step that touches the live control-plane PDP — the highest-risk sub-project, so it ships last before the store merge and defaults off. (Mirrors SP3/SP4d evaluator + switch.)
- **EO4 — One grant store (capstone).** Fold `access_grants` into the `perm.*` one grant store, reusing SP5b's reversible switch, equivalence gate, and `perm.migrated_from` provenance. After EO4, egret objects live entirely in the one grant model. (Mirrors SP5b.)

## Migration principles

Inherited from the governance-layer concepts spec:

- **One EP at a time.** The `egret-cedar` EP switches (policy source in EO3, grant source in EO4) only when the new path gives equivalent allow/deny answers, checked by `explain` over a sample; a blocked switch lists why.
- **Reversible while open.** Undo is available until `finalize`; EO3's switch and EO4's store merge are each reversible.
- **Every data change is a migration** with provenance (`perm.migrated_from`) and undo.
- **No long-term backward compatibility.** When the `egret-cedar` EP has switched and been finalised, the bespoke `templates.rs` generation and the standalone `access_grants` read path are deleted.
- **Owners of existing objects**: `created_by` where present; else the workspace creator; else the platform-admins group. EO1 lists the objects that fall back.

## Errors

Fail closed everywhere, matching both stacks today:

- **A compile of the `egret-cedar` EP fails**: `CedarPdp` keeps serving its last good `PolicySet`; no partial policy is swapped in; the error shows on the EP and the compile run records the reason.
- **The capability matrix does not know a combination**: treated as missing → deny with reason "not measured".
- **A principal cannot be resolved**: left out of every allow, still covered by every deny.
- **Equivalence not proven**: the switch is refused (EO3 and EO4), exactly as SP5b refuses a catalog switch that does not prove parity.

## Verification

1. **IR unit tests** (EO1): the object tree, inheritance, group expansion, deny-wins, the grant-vs-typed classification of every `rbac` key, each with a shape test.
2. **Differential** (EO2): the `egret-cedar` target, evaluated over a corpus of (principal, object, action), returns the **same decision** as today's `CedarPdp` for every case; the classification/clearance cases included.
3. **Switch-equivalence gate** (EO3, EO4): `explain` over a sample proves parity before either switch flips, with teeth proven by a seeded over-grant that the gate catches.
4. **No behaviour change until a switch flips**: EO1 and EO2 add code and goldens only; the live PDP is untouched until EO3, and the grant store until EO4.
