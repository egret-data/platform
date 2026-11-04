# Governance layer — concepts (sub-project 0)

Status: approved design, 2026-11-02. This is the umbrella spec for the governance-layer programme. Each sub-project (1–5) gets its own spec and plan that must stay inside these concepts; a sub-project that needs to change a concept changes this document first.

## Why

egret's permission model is the source of truth for data access, but it compiles straight to Ranger JSON, and Ranger-shaped choices have leaked into the model: profiles are Ranger services, there is a Trino-only tag service, roles and groups are expanded into usernames inside the Ranger compiler, hand-written Ranger policies live beside compiled ones, and `explain` re-implements Ranger. Twelve live mechanisms decide who may do what on data (inventory below), and seven of them overlap. The goal is one common layer, in the spirit of Unity Catalog, that compiles to several enforcement points (Ranger, OPA, the Iceberg REST catalog) through a target-neutral intermediate representation.

## Decisions

| # | Decision |
|---|---|
| D1 | Securables are data and data-plane objects only. egret objects (dashboards, charts, datasets, Data APIs, notebooks, workflows, apps) keep the Cedar RBAC they have today and consume data through the common layer. |
| D2 | A workspace is a binding (which catalogs are reachable from it) plus an automatically managed group of its members. The catalog-wide read a member gets today becomes an ordinary grant to that group. |
| D3 | Data principals are user, group and service account. RBAC roles are for egret API permissions only. |
| D4 | Privileges: `BROWSE`, `READ`, `WRITE`, `CREATE`, `MANAGE`, plus one `OWNER` per securable. |
| D5 | Masks, row filters and clearance are one concept: a rule (selector + effect + exemptions). Clearance becomes the `withhold` effect. |
| D6 | Grants keep deny; deny always wins; no priorities; no exclude lists. |
| D7 | An enforcement point that cannot enforce something fails closed and says why. |
| D8 | No hand-written policies in the new model; engine-specific needs become typed engine privileges on the enforcement point. |
| D9 | Data privileges are common; they are never declared per enforcement point (at most a restrict-only override). |
| D10 | Order of sub-projects: 0 → 1 → 2 → 3 → 4 → 5. |

## Concepts

### Securable
The tree `catalog → schema → table | view → column`; volume and function may be added later. Every securable has a stable id, so grants and rules follow a rename (the behaviour packages #757 and the grants-follow-rename work already give). Every securable has exactly one `OWNER`, by default its creator; the owner holds every privilege on it and below it.

### Principal
- **User** — a human; local or SSO.
- **Group** — an SSO group, a local group, or one of two automatically managed kinds:
  - `ws:<slug> members` — the members of a workspace (D2);
  - `role:<name> holders` — the holders of an RBAC role. It exists only to carry existing data grants that name a role across the migration (D3); no new grant may target it once its sub-project ships.
- **Service account** — a machine principal (sub-project 4 unifies Data API service accounts and catalog clients into it).

The model refers to principals by id. Names appear only in compiled output for engines; engine usernames are minted through `ensure_username_via` and the append-only username ledger (#741), so a name is never reissued.

### Grant
`(principal, securable, privilege, allow | deny)`.

| Privilege | Means |
|---|---|
| `BROWSE` | see the object and its metadata |
| `READ` | select data |
| `WRITE` | insert, update, delete, merge |
| `CREATE` | create children (schemas in a catalog, tables and views in a schema) |
| `MANAGE` | alter, drop, and grant on the object |
| `OWNER` | all of the above; exactly one per securable; transferable |

`READ`, `WRITE`, `CREATE` and `MANAGE` each imply `BROWSE` on the object and its ancestors (you can see what you can use). Grants inherit down the tree. A deny anywhere on the path beats any allow (D6). There is no priority and no exclude list: today's exclude principal becomes a deny for that principal.

### Rule (ABAC)
`selector + effect + exemptions`.
- **Selector**: a classification (applied once, effective on every enforcement point) or a path.
- **Effect**: `mask` (with a mask type and expression), `row filter` (with a filter expression), or `withhold` (deny to everyone but the exemptions — today's clearance).
- **Exemptions**: users, groups, service accounts.

Groups-only clearance, which compiles to nothing on Ranger today, works because exemptions are resolved like any principal set. The Cedar `classification_forbid` gate stays only for egret BI objects (D1).

### Enforcement point (EP)
An EP is `engine × mechanism × version`, for example `trino-ranger@2.5`, `trino-opa`, `starrocks-ranger@2.8`, `iceberg-rest`. It holds:
- its **catalog scope** (today's engine–catalog attachments);
- its **typed engine privileges** (today's eight engine capabilities, plus any kind a hand-written policy needs, added as a new typed privilege — D8);
- an optional **restrict-only override** (D9);
- its **compile state** (version, last compile run, reasons).

An EP replaces today's "profile" (a Ranger authz service). Several engines may share one EP, as several share one profile today.

### Capability matrix
Per EP kind and version: which effects and privileges it can enforce, and at what granularity. Where it cannot, the compiler lowers to the strongest thing it can enforce (D7):

| Missing capability | Lowered to |
|---|---|
| mask on a column | deny that column to everyone but the exemptions |
| row filter on a table | deny the table to everyone but the exemptions |
| any effect at all on an object kind (e.g. StarRocks Iceberg metadata tables, KI-340) | a standing warning on the EP (as #822 does) |
| an unknown combination (e.g. a new plugin version) | treated as missing; reason "not measured" |

Each lowering carries a reason that the UI shows on the rule and on the EP; this generalises today's `NotEnforced` reasons.

Consequence for `iceberg-rest`: an external Iceberg client reads files directly, so masks and row filters cannot apply there. That EP denies `READ` on any table with a mask, row filter or withhold to everyone but the exemptions.

## Architecture

```text
 Store (source of truth)        Neutral IR                  Compiler per EP          Delivery to the EP
 securables, principals,  ──►   per-EP statement set  ──►   ranger(trino@2.5)  ──►   Ranger download (as today)
 grants, rules, EP config       names resolved, scoped      ranger(starrocks@2.8)    Ranger download
                                to the EP's catalogs        opa(trino)         ──►   OPA decision API served by egret
                                      ▲                     iceberg-rest       ──►   catalog guard + STS scope
                               capability matrix
```

1. **Store → IR** (new crate `egret-perm-ir`): resolves inheritance, expands groups into users and service accounts (no engine egret runs understands groups today), applies the EP's catalog scope and the capability matrix. It knows nothing about Ranger or OPA. Its output contains only what that EP can enforce, and every lowering carries provenance and a reason.
2. **IR → target**: one module per target, translating shape only; no policy decisions. The current Ranger compiler becomes the `ranger` target. Infrastructure policies (`egret-admin`, the defaults, delegator) become a per-EP generated part (sub-project 2): they still materialise as `managed_by='infra'` rows, but as compiler output, not hand-authored input.
3. **Delivery**: unchanged channels (Ranger download, plugin authentication, version bumps); for OPA, egret answers the OPA REST decision API itself (sub-project 2, [spec](2026-11-04-gov-sp2-trino-opa.md)).

`explain` ("Who can read?", "Why denied?") evaluates the IR, so one evaluator serves every EP. `egret-ranger-eval` stays as a test oracle for differences between the IR and a given plugin version.

Compile triggers, profile locking (`lock_profile_rows_in_tx`) and in-transaction recompiles stay as they are; only the name "profile" becomes "EP" (sub-project 3).

## Where today's mechanisms go

| Today | In the new model | Sub-project |
|---|---|---|
| `perm.grants` (read/write, include/exclude) | grants; read→`READ`, write→`WRITE`, exclude→deny | 3 |
| implicit workspace grant + `registry.catalog_workspaces` | workspace binding + `READ` for `ws:<slug> members`; stored once | 3 |
| `perm.rules` | rules (mask, row filter) | via IR in 1 |
| `perm.engine_grants` | typed engine privileges on the EP | 3 |
| clearance compiled to Ranger | `withhold` rules | 3 |
| `policy.authz_policies` `managed_by='operator'` | removed; Migrate converts each to grants, rules or typed engine privileges; an EP with an unconvertible policy cannot switch | 3 |
| infra policies (`managed_by='infra'`) | generated per EP | 1 (byte-identical), 3 |
| Cedar RBAC (`roles`, `role_permissions`, `access_grants`) | unchanged; egret API and BI objects only | – |
| Cedar classification gate | BI objects only; catalog side becomes `withhold` | 3 |
| `catalog.catalog_grants`, `catalog_roles`, `catalog_clients` | common grants + `iceberg-rest` EP; catalog clients become service accounts; `catalog_roles` removed; engines get their own credential instead of the shared `engine` role with `manage` | 4, 5 |
| `registry.engine_catalog_attachments` | the EP's catalog scope | 3 |
| `engine_mgmt.engine_workspaces` | unchanged (workspace binding) | – |
| `dataapi.service_accounts` | service accounts | 4 |
| `analytics.app_grants`, `app_pod_tokens` | out of scope (egret object permissions) | – |
| `role` principals in grants, exemptions, clearance | `role:<name> holders` groups | 3 |
| two tag tables (BI, catalog) | both kept; only catalog tags feed rules | – |

### Migration principles
- **One EP at a time.** Each EP has a switch, old model / new model. It may switch only when the new compile gives equivalent allow/deny answers, checked by `explain` over a sample of principals × objects; a blocked EP lists why.
- **Owners of existing securables**: the creator recorded in the catalog when there is one; otherwise the platform admins group. Sub-project 3 lists the objects that fall back.
- **No long-term backward compatibility.** When every EP has switched, the old path is deleted (operator `authz_policies`, `role` principals, the implicit-grant duplication).
- **Every data change is a migration** with provenance (extending `perm.migrated_from`) and undo while the switch is open.

## Errors

Fail closed everywhere:
- **A compile of one EP fails**: the EP keeps serving its last good artifact; no partial artifact is published; the EP shows the error and the compile run records the reason (as today).
- **The capability matrix does not know a combination**: treated as missing → lowered to deny with reason "not measured".
- **A principal's username cannot be minted** (refused by the ledger guard): the principal is left out of every allow and still covered by every deny; the reason shows on its People page.
- **OPA or the catalog does not answer**: the engine fails closed; egret configures no fail-open anywhere.

## Verification

1. **IR unit tests**: inheritance, group expansion, deny-wins, capability lowering, each with a shape test.
2. **Byte-identical** (sub-project 1): the Ranger target through the IR produces byte-identical JSON to today's compiler on the whole corpus and the demo stack's profiles.
3. **Differential against real enforcers**: the existing `ranger-differential` (jars 2.5.0 and 2.8.0); for OPA (sub-project 2), egret's evaluator against real Ranger on a neutral corpus, and Trino-on-OPA against Trino-on-Ranger end to end. `explain` on the IR must match the real enforcer, per version.
4. **Live checks** for every behaviour-changing PR: own stack, an unprivileged user querying a real engine.

## Sub-projects and acceptance

| # | Sub-project | Done when |
|---|---|---|
| 0 | Concepts (this spec) | approved; every sub-project has scope and an acceptance gate |
| 1 | Neutral IR + capability matrix | in production; Ranger JSON byte-identical; `NotEnforced` generated from the matrix |
| 2 | Trino OPA target | a Trino engine runs on OPA behind a per-profile switch; differential parity with Ranger on the corpus, every difference explained; latency measured |
| 3 | Enforcement points replace profiles | every EP on the demo stack switched; no hand-written policies; groups-only clearance works on Trino and StarRocks |
| 4 | Service accounts as principals | Data API service accounts and catalog clients are one service-account kind; full credential lifecycle (create, rotate, expire, disable) |
| 5 | One grant model | one grant store; external Iceberg clients obey common grants and `withhold`; engines no longer hold `manage` through the shared role |

## Out of scope
Permissions on egret objects (BI, Data API endpoints, notebooks, workflows, apps); lineage; Spark; OPA for StarRocks (waiting on upstream StarRocks #77386).

## Inventory (origin/main 91f7595e)
Twelve live mechanisms: (1) `perm.grants` + implicit workspace grants; (2) `perm.rules`; (3) `perm.engine_grants`; (4) clearance compiled to Ranger; (5) hand-written and infra Ranger policies in `policy.authz_policies`; (6) Cedar RBAC; (7) Cedar classification gate; (8) embedded Iceberg catalog RBAC (`catalog_grants`, `catalog_roles`, `catalog_clients`); (9) engine–catalog attachments; (10) workspace assignments of catalogs and engines; (11) Data API service accounts; (12) app grants and pod tokens.

Overlaps this design removes: clearance enforced twice and differently (Cedar honours groups, Ranger skips groups-only); two grant stores for the same data (engines bypass `catalog_grants` through the shared `engine` role, external Iceberg clients bypass `perm.grants`); three "role" concepts; hand-written and compiled policies in one table; workspace scope stored twice; three machine-principal mechanisms. Kept on purpose: two tag tables (BI objects and catalog objects are different trees).
