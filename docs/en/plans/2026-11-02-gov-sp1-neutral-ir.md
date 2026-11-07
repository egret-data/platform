# Governance sub-project 1 — neutral IR + capability matrix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put a target-neutral intermediate representation (new crate `egret-perm-ir`) between the `perm` store and the Ranger compiler. The IR makes every policy decision. A capability matrix makes every "this enforcement point cannot do that" decision. The Ranger compiler shrinks to a target that only translates shape. Nothing observable changes: the Ranger JSON every profile and tag service serves stays byte-identical, and `NotEnforced` (and the clearance "groups not enforced" reasons) are generated from the matrix.

**Architecture:** `egret-perm-compile::trigger::compile_desired` becomes a five-stage pipeline: **load** (every DB read, in `egret-perm-compile/src/ir_load.rs`, on the caller's transaction, under the locks the caller already holds) → **lower** (pure: `egret_perm_ir::lower::*`, store model + loaded facts + matrix → `StatementSet`) → **render** (pure: `egret-perm-compile/src/ranger/*`, `Statement` → Ranger body) → **complete** (the two served-unit passes that read hand-written Ranger policies: hidden columns and metadata tables, gated by matrix entries) → **refuse/diff** (overlap check, profile ceiling, `apply_diff_in_tx`, both unchanged). The migration is a strangler, one policy family per task. A golden harness built in Task 1, on unchanged code, keeps every task honest.

**Tech Stack:** Rust (workspace crates), sqlx/Postgres tests, the Ranger differential harness.

## Global Constraints

- The Ranger JSON each profile and its tag service serve stays byte-identical. The golden corpus (`crates/egret-perm-compile/tests/golden/*.json`) changes in no task after Task 1.
- No schema change, no migration file, no migration number. No API change: `openapi.json` and `frontend/src/api/generated/**` stay byte-identical. No UI change: nothing under `frontend/src` changes.
- `CompileSummary::skipped` keeps every reason string byte-for-byte and in the same order. The golden harness captures both.
- No new lock site: `crates/egret-perm-compile/tests/profile_lock_sites.rs` keeps its `SITES` table unchanged. `egret-perm-ir` has no `sqlx` dependency and does no I/O.
- `egret-perm-ir` never depends on `egret-policy`, `egret-policy-api` or `egret-ranger-eval`, and has no Ranger word in a public type name. `crates/egret-perm-ir/tests/boundary.rs` enforces both.
- `egret_perm_compile::{compile_grant, compile_rule, validate_grant_shape, validate_rule_shape}` keep their signatures. The Ranger differential corpus, `equivalence.rs`, `compile_grants.rs` and `compile_rules.rs` call them and must not change.
- Code is moved, not copied. The old path is deleted in the same task that replaces it. No commit carries two implementations of one family.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` pass on every commit.
- DB tests need `DATABASE_URL=postgres://egret:egret@localhost:5432/egret` (never `EGRET_ALLOW_DB_SKIP`).
- One test suite at a time, so no concurrent `cargo test` runs. Use your own `CARGO_TARGET_DIR` (the shared target collides across checkouts).
- Conventional Commits. Each commit ends with the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Compile time on the largest golden fixture, measured in release, stays at or below 1.10× the Task 1 baseline. This is checked in Task 11.

---

## Design

All line numbers are against `origin/main` `91f7595e`.

### D1. Today's compile pipeline

**Entry and locks.** Every recompile path ends in `recompile_locked_service_in_tx` (`crates/egret-perm-compile/src/trigger.rs:1653-1676`). The order is:

1. `recompile_service_in_tx` (`:1631-1641`) runs `lock_services_in_tx`, then `refuse_engine_of_another_type`, then `premint_model_usernames`.
2. `sync_admin_policy_in_tx` converges the infra `egret-admin - …` rows (`:1684-1709`).
3. `compile_desired` (`:379-832`).
4. `apply_diff_in_tx` (`:1237`), which takes `lock_profile_rows_in_tx` at `:1272`, reads the compiler-owned rows by name prefix (`:1274-1298`), writes only the difference, and bumps `policy_version` at most once.
5. `record_compile_ok_in_tx`.

**Inputs read inside `compile_desired`, all on the caller's connection:**

- The profile row and its engine type (`:383-389`, `engine_type_for` `:352-358`: `trino` | `starrocks`, anything else compiles nothing).
- The profile's catalog scope (`catalogs_for_service_via`, `:390-394`).
- Held-rename "leaving" coordinates (`:397`).
- The Trino tag service, resolved by name from `config_jsonb.tagService` (`:412-441`). StarRocks has none.
- Every grant and every rule, installation-wide (`:449-454`).
- The engine grants of every engine bound to the profile (`:464-466`).
- Then `members::expand_enumerated_principals` (`:467-473`).

**Outputs.** `desired: BTreeMap<(service_id, name), DesiredPolicy>`. A Trino tag-selected body names the tag service, and `target_service_id` (`:310-329`) files it there. `sweep` lists the profile and, when configured, its tag service. `skipped: Vec<SkippedItem>`. The profile and its tag service are the only services written.

**Families, in emission order** (the order is load-bearing for `skipped`):

| # | Family | Code | Output |
|---|---|---|---|
| 1 | grants | `trigger.rs:486-555` → `grants::compile_grant` (`grants.rs:679`) | `perm-grant-{id}` access policy on the profile |
| 2 | engine grants | `trigger.rs:557-567`, `834-903` → `engine_grants::compile_engine_grant` (`engine_grants.rs:57`) | `perm-engine-{id}` |
| 3 | rules | `trigger.rs:569-584`, `1027-1235` → `rules::compile_rule_counting` (`rules.rs:453`) | Trino tag rule: one tag policy (tag service) plus KI-287 per-table row filters (`rules.rs:502-536`). StarRocks tag rule: per-column masks / per-table row filters (`rules.rs:539-621`). Path rule: `perm-rule-{id}` (`rules.rs:622-633`) |
| 4 | clearance | `trigger.rs:600-614` → `clearance::compile_clearance` (`clearance.rs:236-401`) | Trino: `perm-clearance-{tag}` tag deny. StarRocks: a per-column / whole-table deny plus `perm-clearance-mv` (`clearance.rs:420-605`, `1005-1039`) |
| 5 | clearance view-guard masks (StarRocks) | `trigger.rs:616-629`, `945-1025`, `clearance_mask.rs` | priority-1 `MASK_NULL` per classified column, folded through `egret-ranger-eval` |
| 6 | hidden columns (StarRocks) | `trigger.rs:636-669`, `clearance.rs:674-800` | `perm-hidden-{c}.{s}.{t}` deny on `_file`, `_pos` |
| 7 | metadata tables (Trino, #821) | `trigger.rs:676-742`, `metadata_tables.rs:258-340` | `perm-hidden-meta-{source}` deny on `t$*` |
| 8 | overlap refusal | `trigger.rs:744-797`, `overlap.rs:249` | refusal only |
| 9 | profile ceiling | inside `apply_diff_in_tx` | refusal only |

**Group, role and workspace expansion to usernames.** This happens in `members::expand_enumerated_principals` (`members.rs:782-923`). It is the only place:

- Workspace members come from `workspace_members_via` and group members from `group_members_via`. Missing usernames are minted (`mint_for_compile`).
- Roles come from `role_usernames(…, MissingUsername::Mint)`.
- The rewrite is `substitute` / `substitute_one` (`members.rs:937-1013`). It is first-appearance ordered and dedupes on `(kind, ref)`, but **only when the list holds at least one expandable principal** (`:943-951`). A list of plain users is left untouched, duplicates included. The IR must reproduce this exactly.
- `public` is never expanded (`is_expanded_group`, `:690-692`).
- Clearance expands roles separately, through the same `role_usernames` (`clearance.rs:268-280`). Its `required_groups` are not expanded.

**NotEnforced and related reasons. Produced and consumed:**

| Reason | Produced | Consumed |
|---|---|---|
| `NotEnforced::{NoAssignment, NoEngine, NoProfile, NoTagService}` | `effect.rs:56-69` (enum). The tag-capable flag is computed **in SQL**, `effect.rs:393-415`. `enforcement()` `:435-497`. `catalog_not_enforced` `:420-430` | API: `RuleEffect.notEnforced`, `EffectCatalog.notEnforced`, `RuleEnforcement` (`GET /perm/rules/{id}/effect`, `GET /perm/rules/enforcement`, `POST /perm/preview`). Explain's C5 line (`routes.rs:2996-3020`, `explain_effect.rs:179-194`). UI: `frontend/src/routes/governance/NotEnforced.tsx`, `RulePreview.tsx`, `TagRulesSection.tsx`, `RuleDialog.tsx`, `classifications/EffectStrip.tsx`, `data/tabs/MasksFiltersTab.tsx`, `data/ColumnPanel.tsx`, `people/RulePage.tsx` |
| `EngineClearance::NotEnforcedGroupsOnly`, `EngineNotClearedReason::GroupsNotEnforced` | `people.rs:414`, `:430`, set at `:1023`, `:1037`, `:1234`, `:1242`, `:1322` | API: people/clearance reads. UI: `people/tabs/ClearancesTab.tsx` |
| compile skips (`SkippedItem.reason`) | `trigger.rs:498`, `531`, `851-899`, `982-1008`, `1064`, `1135`, `1173-1191`; `clearance.rs:293`, `336`, `547`, `558`; `clearance_mask.rs:381`, `390`, `402`, `440` | API: `CompileResponse.skipped` (`routes.rs:1184-1209`), the compile-run record |
| StarRocks metadata tables are never asked (#822) | **UI only**: `frontend/src/routes/control-plane/StarRocksMetadataTablesWarning.tsx` | engine pages |

**What is engine-specific:**

- Resource level names: Trino `schema`, StarRocks `database` (`grants.rs:218-223`).
- `serviceType` stamping (`rules.rs:804-812`).
- Action → access names (`egret_perm::actions::engine_accesses`, `actions.rs:717-724`).
- Tag service: Trino only. StarRocks expands per column / table.
- Trino tag row filters reach only table-level attachments (KI-287).
- StarRocks clearance withholds `select` only (`withheld_accesses`, `clearance.rs:1143-1158`), adds an MV deny and view-guard masks, and has hidden columns. Trino has metadata-table denies.
- Declared mask types are read from the embedded service-def (`rules.rs:825-865`).

**Plugin versions.** Ranger plugin 2.5.0 (`trinodb/trino:470`) is **dropped**: `DROPPED_ENGINE_IMAGES` is at `crates/egret-perm/src/engine_images.rs:138-139` and the floor is 2.8.0 (`:132`). The modelled pairs are `MODELLED_RANGER_PLUGINS = [(trino,2.8.0),(starrocks,2.8.0),(starrocks,2.9.0)]` (`actions.rs:83-87`). The only compile-time use of a version is the view-guard fold (`clearance_mask.rs:425-460`). Every other family is version-blind today.

**Where hand-written and infra policies merge.** They do not merge in the compiler; they merge at **download**. `download_policies` (`crates/egret-policy/src/ranger/api/download.rs:120-238`) serves `enabled_policies_for_service` (every enabled row, `ORDER BY id`, with `id`/`version`/`isEnabled` injected: `store.rs:2325-2350`) and embeds the tag service's rows as `tagPolicies`. The compiler reads hand-written rows in three places:

- the view-guard fold's `others` (`trigger.rs:962-975`);
- hidden columns (`:646-656`);
- metadata tables (`:688-707`).

There are three infra writers:

- `seed_default_trino_policies` (`default_policies.rs:117`);
- `sync_admin_all_access_in_tx` (`:235`, run on every recompile);
- delegator seeding (`egret-engine-mgmt/src/delegator_policy.rs`, `engines.rs`).

All three stamp `managed_by = 'infra'` (`migrations/0180_policy_managed_by.sql`).

**Where the overlap and priority checks run.**

- On the complete desired set, before any write: `overlap::refuse_created_by_compile_via` (`trigger.rs:791-797`, `overlap.rs:249`), on `policyPriority` / evaluation-order semantics. This is pure Ranger.
- The editor's `egret_policy::unit_overlaps`.
- The per-rule ceiling (`DEFAULT_EXPANSION_CEILING = 500`, `trigger.rs:149`), the clearance ceiling (`clearance.rs:503-511`) and the profile ceiling (`PROFILE_EXPANSION_CEILING = 1000`, `trigger.rs:196`, inside `apply_diff_in_tx`).

### D2. IR types (`crates/egret-perm-ir/src/statement.rs`, `audience.rs`, `ep.rs`)

```rust
// ep.rs — what the IR knows about the enforcement point it lowers for.
use std::collections::BTreeMap;
use egret_perm::EngineType;
use uuid::Uuid;

/// How an engine is asked. Ranger is the only mechanism in sub-project 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mechanism { Ranger }

/// `engine × mechanism`; the version is per bound engine (several share one profile).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EpKind { pub engine: EngineType, pub mechanism: Mechanism }

/// One engine bound to the profile and the mechanism version it runs, or why
/// the version is unknown (the `Err` text is today's `engine_plugin_version` error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundEngine { pub name: String, pub version: Result<String, String> }

/// Everything about one enforcement point (today: one profile) the lowering needs.
#[derive(Debug, Clone)]
pub struct EpInput {
    pub profile_id: i64,
    pub profile_name: String,
    pub kind: EpKind,
    /// Bound engines, by name; empty when none is bound.
    pub engines: Vec<BoundEngine>,
    /// The version egret's default image runs, used when `engines` is empty.
    pub default_version: Result<String, String>,
    /// The EP can resolve classifications itself (today: a Trino profile whose
    /// `tagService` names an existing service).
    pub classification_channel: bool,
    /// Catalog scope: registry catalog id → engine-facing name.
    pub catalogs: BTreeMap<Uuid, String>,
}
```

```rust
// audience.rs — who a statement applies to, with provenance.
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemberName {
    /// An engine username (minted through the ledger, #741).
    User(String),
    /// Every principal (rendered as the engine's universal group).
    Everyone,
    /// A group the IR leaves unexpanded. Only reachable from `compile_grant` /
    /// `compile_rule` called directly with a non-`public` group (the grammar
    /// differential); a recompile has always expanded it.
    UnexpandedGroup(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Via {
    Direct,
    Group(String),
    Role(String),
    WorkspaceMembers(Uuid),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member { pub name: MemberName, pub via: Via }

/// Ordered (first appearance), as today's item arrays are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Audience { pub include: Vec<Member>, pub except: Vec<Member> }

/// Membership already resolved to usernames by the loader (DB reads + mints stay
/// in `egret-perm-compile::members`).
#[derive(Debug, Clone, Default)]
pub struct Directory {
    pub workspaces: BTreeMap<Uuid, Vec<String>>,
    pub roles: BTreeMap<String, Vec<String>>,
    pub groups: BTreeMap<String, Vec<String>>,
}
```

```rust
// statement.rs — the neutral statement set.
use egret_perm::CompileError;
use uuid::Uuid;
use crate::audience::Audience;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Grant { grant_id: Uuid },
    Rule { rule_id: Uuid, rule_name: String },
    Withhold { classification_id: Uuid, classification_name: String },
    EnginePrivilege { engine_grant_id: Uuid },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Level { Named(String), Unnamed }

/// A path in the EP's scope. `catalog_id` is `None` only for a path the EP
/// owns outright (the StarRocks MV guard's `default_catalog`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    pub catalog_id: Option<Uuid>,
    pub catalog: String,
    pub schema: Level,
    pub table: Level,
    pub column: Level,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Path(Path),
    /// Resolved by the EP itself (matrix: `ClassificationSelector` native).
    Classification { name: String },
    /// An engine-typed object (D8): the capability's own values.
    EngineObject { capability: String, values: Vec<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Privilege { Read, Write, CreateMaterializedView }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Allow { privileges: Vec<Privilege> },
    Deny { privileges: Vec<Privilege> },
    Mask { mask_type: String, expression: Option<String> },
    RowFilter { expression: String },
    /// Deny to `audience.include` except `audience.except` (today's clearance).
    Withhold { privileges: Vec<Privilege> },
    /// The view guard of a withhold: everyone but the exempt sees NULL.
    WithholdMask,
    EnginePrivilege { capability: String },
}

/// How the statement came to exist. Drives nothing but naming in a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Derivation {
    AsDeclared,
    /// The matrix lowered a classification selector to concrete paths.
    ExpandedFromClassification,
    /// A table-level classification on an EP that cannot select by classification.
    WholeTable,
    /// The matrix's materialized-view guard of a withhold.
    MaterializedViewGuard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub source: Source,
    pub audience: Audience,
    pub target: Target,
    pub effect: Effect,
    pub derivation: Derivation,
}

/// Why something was not compiled (or compiled weaker), with today's exact text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasonCode {
    EmptyAudience,
    UnexpandedPrincipal,
    UnknownClassification,
    NoClassificationChannel,
    WithholdExemptionByGroupOnly,
    ViewNotMeasured,
    TableNotInCatalog,
    NotMeasured,
    VersionsDisagree,
    MaskPrincipalGap,
    ImpersonationTarget,
    InvalidEnginePrivilege,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lowering { pub source: Source, pub code: ReasonCode, pub detail: String }

#[derive(Debug, Clone, Default)]
pub struct Lowered {
    pub statements: Vec<Statement>,
    pub lowerings: Vec<Lowering>,
}

pub type LowerResult = Result<Lowered, CompileError>;
```

`CompileError` is reused from `egret_perm` (a model crate with no Ranger dependency). A refusal that aborts today aborts the same way.

### D3. Capability matrix (`crates/egret-perm-ir/src/matrix.rs`)

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    ClassificationSelector(RuleEffectKind),   // Mask | RowFilter
    WithholdByClassification,
    ClassificationRowFilterOnColumnAttachment,
    GroupPrincipal,
    RolePrincipal,
    WithholdExemptionByGroup,
    WithholdPrivilege(Privilege),
    WithholdThroughView,
    WithholdThroughMaterializedView,
    MaskCoversHiddenColumns,
    PoliciesReachMetadataTables,
    EngineAsksAboutMetadataTables,
    MaskType(String),
}

pub enum LoweringKind { ExpandPerColumn, ExpandPerTable, ExpandColumnOnlyTablesPerTable, ExpandToUsers,
                        ViewGuardMask, DenyCreateMaterializedView, DenyHiddenColumns, DenyMetadataTables }

pub enum Support { Native, Lowered(LoweringKind), Missing(ReasonCode) }  // channel requirement resolved inside `support`
pub enum Versions { Any, Exactly(&'static [&'static str]) }
pub struct Entry { /* kind, versions, key, answer, evidence: see Task 2 */ }

/// Absent combination ⇒ `Support::Missing(ReasonCode::NotMeasured)` (spec D7).
pub fn support(ep: &EpInput, cap: &Capability) -> Support;
```

The matrix lives in `egret-perm-ir` as a `const ENTRIES: &[Entry]` table. Every row carries an `evidence` string that names the code it replaces. The entries are derived from what the compiler does today. Every current reason is one entry:

| # | Kind / versions | Capability | Support | Replaces |
|---|---|---|---|---|
| M1 | Trino·Ranger / Any | `ClassificationSelector(Mask)` | `NativeWithChannel`, else `Missing(NoClassificationChannel)` | `rules.rs:497-500` (`MissingTagService`) → skip `trigger.rs:1173-1191`; `NotEnforced::NoTagService` `effect.rs:405-409`, `:460`, `:477-479` |
| M2 | Trino·Ranger / Any | `ClassificationSelector(RowFilter)` | `NativeWithChannel`, else `Missing(NoClassificationChannel)` | same |
| M3 | Trino·Ranger / Any | `ClassificationRowFilterOnColumnAttachment` | `Lowered(ExpandColumnOnlyTablesPerTable)` | KI-287 `rules.rs:502-536`, fetch decision `trigger.rs:1076-1079` |
| M4 | StarRocks·Ranger / Any | `ClassificationSelector(Mask)` | `Lowered(ExpandPerColumn)` | `rules.rs:539-621` (mask arm) |
| M5 | StarRocks·Ranger / Any | `ClassificationSelector(RowFilter)` | `Lowered(ExpandPerTable)` | `rules.rs:567-585` (row-filter arm) |
| M6 | Trino·Ranger / Any | `WithholdByClassification` | `NativeWithChannel`, else `Missing(NoClassificationChannel)` | `clearance.rs:334-345` |
| M7 | StarRocks·Ranger / Any | `WithholdByClassification` | `Lowered(ExpandPerColumn)` (whole table → `*` + every column) | `clearance.rs:350-363`, `420-605` |
| M8 | any·Ranger / Any | `GroupPrincipal` | `Lowered(ExpandToUsers)` (D7) | `members.rs:690-692`, `937-1013` |
| M9 | any·Ranger / Any | `RolePrincipal` | `Lowered(ExpandToUsers)` (D0) | `members.rs:174`, `964-980` |
| M10 | any·Ranger / Any | `WithholdExemptionByGroup` | `Missing(WithholdExemptionByGroupOnly)` | `clearance.rs:289-299`; `people.rs:414`, `:430`, `:1023`, `:1037`, `:1234`, `:1242`, `:1322` |
| M11 | Trino·Ranger / Any | `WithholdPrivilege(Read)`, `(Write)` | `Native` | `clearance.rs:1143-1146` |
| M12 | StarRocks·Ranger / Any | `WithholdPrivilege(Read)` `Native`; `(Write)` | `Missing(NotMeasured)` → dropped from the statement | `clearance.rs:1123-1158` |
| M13 | StarRocks·Ranger / `Exactly(["2.8.0","2.9.0"])` | `WithholdThroughView` | `Lowered(ViewGuardMask)`. An unknown version gives `Missing(NotMeasured)` with today's text | `clearance_mask.rs:425-460`; `trigger.rs:609-629`, `945-1025` |
| M14 | StarRocks·Ranger / Any | `WithholdThroughView` on a whole-table target that is a view | `Missing(ViewNotMeasured)` | `clearance.rs:547-557` |
| M15 | StarRocks·Ranger / Any | `WithholdThroughMaterializedView` | `Lowered(DenyCreateMaterializedView)` | `clearance.rs:971-1039`, `393-398` |
| M16 | StarRocks·Ranger / Any | `MaskCoversHiddenColumns` | `Lowered(DenyHiddenColumns)` | `trigger.rs:631-669`; `clearance.rs:654-800` |
| M17 | Trino·Ranger / Any | `PoliciesReachMetadataTables` | `Lowered(DenyMetadataTables)` | `trigger.rs:671-742`; `metadata_tables.rs` |
| M18 | StarRocks·Ranger / Any | `EngineAsksAboutMetadataTables` | `Missing(NotMeasured)`: a standing EP warning. Read by no compile code in SP1; the UI warning (#822) stays as is (no UI change) | `metadata_tables.rs:39-43`; `StarRocksMetadataTablesWarning.tsx` |
| M19 | Trino·Ranger, StarRocks·Ranger / Any | `MaskType(name)` | `Native` for each name the embedded service-def declares; anything else gives `CompileError::UnsupportedMaskType` (a refusal, as today) | `rules.rs:825-865` |

**`NotEnforced` from the matrix.** `NoTagService` is M1 and M2 answering `Missing(NoClassificationChannel)`. `NoEngine`, `NoProfile` and `NoAssignment` are **scope** outcomes, not capabilities (no EP serves the catalog, or nothing is reached). The same IR function produces them: `egret_perm_ir::enforcement::rule_enforcement`, from the matrix plus the scope. The wire enum `effect::NotEnforced` is unchanged and is mapped 1:1.

`GroupsNotEnforced` and `NotEnforcedGroupsOnly` are M10.

One exception keeps today's answer. A bound profile whose type is neither `trino` nor `starrocks` counts as enforcing today: the SQL `s.type IS DISTINCT FROM 'trino'` (`effect.rs:403-406`). The matrix knows no such kind, so it maps that case to `Native` explicitly. A test pins this, and it is open question Q2.

**Unknown combination ⇒ missing, reason "not measured".** `matrix::support` returns `Missing(NotMeasured)` for any `(kind, version, capability)` with no row. In SP1 every entry except M13 is `Versions::Any`, because every other family is version-blind today. Tightening those entries to measured versions changes behaviour, so it belongs to sub-project 3 (Q1).

### D4. Where the boundary goes

**Store → IR (decisions; `egret-perm-ir::lower`, pure):**

- catalog-scope filtering;
- empty-audience skips;
- principal expansion and dedupe (moved from `members::substitute_one`);
- user-ref validation;
- scope-shape validation (`ensure_scope_levels_are_named`, the wildcard rule);
- action → privilege;
- mask-type and custom-mask validation;
- exemption expressibility;
- classification-selector handling per matrix (native, per-column, per-table, KI-287 column-only);
- the rule ceiling and the clearance ceiling;
- withhold: groups-only skip, no channel, privileges per engine, whole-table, view and not-a-table skips, the MV guard;
- the view-guard decision and its version check (M13);
- engine-privilege include and target checks.

**IR → Ranger (shape only; `egret-perm-compile/src/ranger/`):**

- policy names (`perm-grant-`, `perm-rule-…`, `perm-clearance-…`, `perm-engine-`);
- resource maps per policy type (`scope_to_resources`, `column_resources`, `table_resources`, the 2-level MV map);
- level names (`schema` / `database`);
- access names (`engine_accesses`, `cap.access`);
- item layout (exempt item first, then `public`; `allowExceptions` / `denyExceptions`);
- `serviceType` stamping;
- `tagify`;
- routing by the body's `service`.

**Code that mixes the two today, and how it is split:**

1. `rules::compile_rule_counting` (`rules.rs:453-634`) decides expansion and ceilings **and** builds bodies. It is split into `lower::rules` (Task 6) and `ranger::rules` (render).
2. `grants::compile_grant` (`grants.rs:679-750`) validates and maps actions **and** builds an `AccessPolicyInput`. It is split in Task 5.
3. `clearance::compile_clearance` / `compile_starrocks_gates` (`clearance.rs:236-605`) decide **and** render via `starrocks_deny` / `clearance_input`. It is split in Task 7.
4. `trigger::fold_clearance_masks` + `clearance_mask::fold_plugin_versions` (`trigger.rs:945-1025`, `clearance_mask.rs:425-460`) mix the version decision (IR, M13) with the fold (Ranger: priority-1 mask reproduced through `egret-ranger-eval` against other Ranger masks, hand-written ones included). The version decision moves to the IR; the fold stays in the target (Task 8).
5. `trigger::compile_engine_grants` + `engine_grants::compile_engine_grant` (`trigger.rs:834-903`, `engine_grants.rs:57-150`) decide **and** render. The decision moves to the IR. The capability catalogue check (`engine_capabilities.rs`, which carries Ranger resource keys) runs in the loader and is handed to the IR as a verdict (Task 9).
6. `effect.rs:393-415` decides "tag-capable" **in SQL**. The decision moves to the matrix and the SQL returns raw facts (Task 2).
7. `members::expand_enumerated_principals` reads and mints (DB) **and** rewrites the model in place. The reads and mints stay in the loader; the rewrite becomes `audience::audience` (Task 3).
8. The hidden-column and metadata-table passes (`trigger.rs:636-742`) decide who is restricted **by parsing Ranger bodies**, hand-written ones included. They stay in the target as **served-unit completion passes**, gated by M16 and M17 (Task 10). The reason: their input includes hand-written Ranger policies, which have no IR form until sub-project 3 removes them (spec D8). Moving them now would mean parsing hand-written Ranger into the IR, which is a feature, not a refactor (Q3).
9. The overlap refusal (`trigger.rs:744-797`) is Ranger arbitration semantics (`policyPriority`, evaluation order). It stays in the target unchanged.

### D5. Proving byte-identical before any refactor (Task 1)

There are three instruments, all built and green on unchanged code in Task 1.

- **(b) Golden corpus, committed.** `crates/egret-perm-compile/tests/golden_compile.rs` runs as one test function that executes fixtures G01–G21 sequentially. Each fixture:
  1. creates its services (committed, fixture-unique fixed names);
  2. runs everything else inside **one transaction that it rolls back**, so installation-wide state (clearance gates, tag rules) cannot leak between fixtures;
  3. calls `recompile_service_in_tx` for each fixture profile;
  4. captures, per profile: the desired set (from a `harness`-feature hook into `compile_desired`) as `(service name, policy name, canonical body)` in `BTreeMap` order; `skipped` in order; the **served** lists (`enabled_policies_for_service_via` for the profile and its tag service: hand-written and infra rows included, `ORDER BY id`); or the compile error's `Display`.

  Normalisation is deterministic and complete. The fixture registers every id it creates (grant, rule, tag, engine-grant, user, workspace) under a role name. The harness replaces each registered UUID with `{role}`, the injected `id` with its ordinal in the served list, and fails on any unregistered UUID left in the output. Bodies are written with sorted keys. `EGRET_GOLDEN_BLESS=1` writes the files; without it the harness compares and writes `target/golden-actual/<fixture>.json` on a mismatch.

- **(a) Every existing DB-backed compile test, by shadow comparison.**
  - Task 1 adds the cargo feature `shadow-compile` to `egret-perm-compile`. Under it, `compile_desired` runs twice on the same transaction and asserts that both results are identical (desired bytes, skipped, sweep), appending one line per comparison to `$EGRET_TEST_SHADOW_LOG`.
  - Running `cargo test --workspace --features egret-perm-compile/shadow-compile` therefore turns every recompile in every existing test (egret-perm-compile, egret-bi, egret-workspace, egret-auth, egret-catalog-engine, …) into a determinism check. Determinism is the precondition for a golden.
  - Each family task temporarily points the shadow at **old vs new** (old kept only within the task), runs the workspace suite, records the comparison count in the commit body, then deletes the old path before committing.
- **(c) Demo stack recipe.** `scripts/golden-demo-capture.sh` runs before and after the deploy:
  1. `POST /api/control-plane/perm/compile/{id}` for every profile; it must report `created = updated = deleted = 0` once converged;
  2. then dumps `GET /ranger/service/plugins/policies/download/{name}` for every profile, raw bytes, after the plugin-visible `policyUpdateTime` is pinned.

  Byte-identical on the stack means the same SHA-256 per profile before and after, and a 0/0/0 recompile after the deploy.

### D6. Migration strategy

Strangler, one family per task, in the order **grants (T5) → rules (T6) → withhold (T7) → withhold view guard (T8) → engine privileges (T9) → served-unit completion and the end of the old driver (T10)**. Before that come the matrix (T2), the expansion (T3) and the loader (T4).

Every family task follows the same steps:

1. write the IR unit tests (red);
2. implement `lower::<family>` and `ranger::<family>`;
3. wire `compile_desired`;
4. run the shadow (old vs new) over the workspace suite;
5. delete the old code;
6. run the golden harness plus the crate suite;
7. commit.

The metadata-table family (#821) is completion work in T10, not a lowering (D4.8).

### D7. Scope fences

- **Explain on the IR is out of sub-project 1.** The spec's acceptance does not list it. Today `explain` evaluates the exact download each plugin receives through `egret-ranger-eval` at the engine's plugin version, which is the property the SP4d programme built and measured. Re-pointing it at the IR is a behaviour change, and its natural home is sub-project 2: OPA needs one evaluator over the IR, and `egret-ranger-eval` then becomes the oracle that checks it.
- **Infra policies stay stored until sub-project 3.**
  - Their three writers run at lifecycle events (service create, engine create, admin role change), not at compile.
  - Generating them per EP would delete and re-insert rows, which changes the `id`s the download is ordered by (`ORDER BY id`, `store.rs:2333-2335`). That breaks byte-identity on the demo stack.
  - `managed_by = 'infra'` guards (`egret-policy/tests/infra_guard.rs`) and `classify` read them as rows.
  - The spec's "1 (byte-identical)" is honoured by including every infra row in the golden served lists (fixture G16), so any accidental change shows.
- **No migration number is needed.** There is no schema change; `perm.compiled`, `policy.*` and every CHECK stay as they are.

### D8. Risks

- **Compile performance.** The pipeline adds one allocation layer (statements) per policy. The baseline is measured on G21 (the largest fixture: a StarRocks profile with 480 tagged columns under one mask, 400 under one clearance, and 300 grants) in release: median of 7 `compile_desired` calls in Task 1, compared in Task 11 against the ≤ 1.10× constraint. The super-linear region past ~1000 policies per profile (`trigger.rs:149-195`) is unchanged because the ceilings are unchanged.
- **Lock ordering.** All reads stay on the caller's connection after `lock_services_in_tx`, in the same statements (`ir_load.rs` calls the same `_via` functions). The IR crate has no `sqlx`. `profile_lock_sites.rs` (`SITES`, 28 sites) is run in every task and never edited. The mint lock is taken by the same `members` code (`lock_compile_mints_in_tx`, `members.rs:316`).
- **Known flakes.**
  - `TAG_FANOUT_LOCK` serialises tag-fanout tests in `tests/routes.rs` and `tests/grant_rule_by_id.rs`. Run them as their own binaries when a workspace run reports them, and do not "fix" them in this plan.
  - The `crates/egret-catalog-engine/tests/table_hook_db.rs` timing flake: re-run that binary alone before attributing a failure to the refactor.
  - A shadow mismatch is never a flake. The comparison is in-process, on one transaction.
- **`serde_json` `preserve_order`** is on in the shipping binary (`crates/egret-workflow/Cargo.toml:15-30`) but not in `cargo test -p egret-perm-compile`. The golden therefore compares canonical (sorted-key) bytes. Stored rows are `jsonb`, which canonicalises anyway, so the served bytes on the stack are what (c) compares.
- **The `substitute_one` early return** (D1) is a byte-identity trap: a no-expansion list keeps duplicates. Task 3 has a test for it.

---

## File Structure

**Create:**

- `crates/egret-perm-ir/Cargo.toml`: deps `egret-perm`, `uuid`, `serde`; dev-deps none beyond std.
- `crates/egret-perm-ir/src/lib.rs`: module list and re-exports.
- `crates/egret-perm-ir/src/ep.rs`: `Mechanism`, `EpKind`, `BoundEngine`, `EpInput`.
- `crates/egret-perm-ir/src/matrix.rs`: `Capability`, `LoweringKind`, `Support`, `Versions`, `Entry`, `ENTRIES`, `support`.
- `crates/egret-perm-ir/src/enforcement.rs`: `AttachedEngine`, `NotEnforcedReason`, `CatalogEnforcement`, `rule_enforcement`, `withhold_engine_reason`.
- `crates/egret-perm-ir/src/audience.rs`: `MemberName`, `Via`, `Member`, `Audience`, `Directory`, `audience`, `audience_from_expanded`.
- `crates/egret-perm-ir/src/statement.rs`: D2 types.
- `crates/egret-perm-ir/src/lower/mod.rs`, `lower/grants.rs`, `lower/rules.rs`, `lower/withhold.rs`, `lower/engine_privileges.rs`.
- `crates/egret-perm-ir/tests/boundary.rs`: dependency and vocabulary guard.
- `crates/egret-perm-compile/src/ir_load.rs`: every DB read the lowering needs.
- `crates/egret-perm-compile/src/ranger/mod.rs`, `ranger/grants.rs`, `ranger/rules.rs`, `ranger/withhold.rs`, `ranger/engine_privileges.rs`, `ranger/completion.rs`.
- `crates/egret-perm-compile/src/shadow.rs`: `cfg(feature = "shadow-compile")`.
- `crates/egret-perm-compile/tests/golden_compile.rs`, `crates/egret-perm-compile/tests/golden/fixtures.rs`, `crates/egret-perm-compile/tests/golden/G01…G21.json`.
- `scripts/golden-demo-capture.sh`.

**Modify:**

- `Cargo.toml` (workspace member dep `egret-perm-ir`).
- `crates/egret-perm-compile/Cargo.toml` (dep `egret-perm-ir`; features `shadow-compile`; `[[test]] golden_compile` `required-features = ["harness"]`).
- `crates/egret-perm-compile/src/{lib.rs, trigger.rs, members.rs, effect.rs, people.rs, grants.rs, rules.rs, clearance.rs, clearance_mask.rs, engine_grants.rs, harness_support.rs}`.
- `justfile` (`test-golden`, called from `check`).
- `.github/workflows/ci.yml` (one step).

**Moved** (in the family task that replaces them):

- `grants.rs` render half → `ranger/grants.rs`.
- `rules.rs` render half → `ranger/rules.rs`.
- `clearance.rs` render half → `ranger/withhold.rs`. `hidden_column_denies` and the helpers under it → `ranger/completion.rs`.
- `metadata_tables.rs` → `ranger/completion.rs` (the module stays and is re-homed as `ranger::completion::metadata_tables`).
- `engine_grants::compile_engine_grant` → `ranger/engine_privileges.rs`.

---

### Task 1: Golden harness, determinism shadow, demo recipe (on unchanged code)

**Files:**
- Create: `crates/egret-perm-compile/tests/golden_compile.rs`, `crates/egret-perm-compile/tests/golden/fixtures.rs`, `crates/egret-perm-compile/tests/golden/*.json` (generated), `crates/egret-perm-compile/src/shadow.rs`, `scripts/golden-demo-capture.sh`.
- Modify:
  - `crates/egret-perm-compile/src/trigger.rs`: make `compile_desired` and `Compiled` / `DesiredPolicy` fields `pub(crate)`, and add the shadow call in `recompile_locked_service_in_tx` (`:1653-1676`).
  - `crates/egret-perm-compile/src/harness_support.rs`: add the `golden_desired_in_tx` export.
  - `crates/egret-perm-compile/Cargo.toml`: feature `shadow-compile = []`, plus a `[[test]] name = "golden_compile" required-features = ["harness"]`.
  - `justfile`: add the recipe `test-golden` and call it from `check` after `cargo test --workspace`.
  - `.github/workflows/ci.yml`: add a step after "Test".

**Interfaces:**
- Produces:
  - `pub async fn egret_perm_compile::harness_support::golden_desired_in_tx(conn: &mut sqlx::PgConnection, service_id: i64) -> Result<Option<GoldenCompiled>, egret_perm::CompileError>`
  - `pub struct GoldenCompiled { pub desired: Vec<(String /*service name*/, String /*policy name*/, serde_json::Value)>, pub skipped: Vec<(String /*kind*/, uuid::Uuid, String)>, pub sweep: Vec<i64> }`
  - `#[cfg(feature = "shadow-compile")] pub(crate) fn shadow::assert_same(context: &str, a: &trigger::Compiled, b: &trigger::Compiled)`

- [ ] **Step 1: Write the harness and fixtures.**

`crates/egret-perm-compile/tests/golden_compile.rs`:

```rust
//! Golden corpus for governance sub-project 1: the Ranger artifacts every
//! fixture profile compiles to, captured on unchanged code and compared byte
//! for byte after every refactor task. Bless with EGRET_GOLDEN_BLESS=1.
//!   DATABASE_URL=postgres://egret:egret@localhost:5432/egret \
//!   cargo test -p egret-perm-compile --features harness --test golden_compile
#[path = "golden/fixtures.rs"]
mod fixtures;

use std::collections::BTreeMap;
use std::path::PathBuf;

use egret_metastore::Db;
use serde_json::{json, Value};
use uuid::Uuid;

/// Ids the fixture created, by role name; anything else in the output fails.
#[derive(Default)]
pub struct Names(pub BTreeMap<String, String>);

impl Names {
    pub fn id(&mut self, role: &str, id: Uuid) -> Uuid {
        self.0.insert(id.to_string(), format!("{{{role}}}"));
        id
    }
    pub fn text(&mut self, role: &str, value: &str) {
        self.0.insert(value.to_string(), format!("{{{role}}}"));
    }
}

fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let sorted: BTreeMap<_, _> = m.iter().map(|(k, v)| (k.clone(), canonical(v))).collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn normalise(text: String, names: &Names) -> String {
    let mut out = text;
    // Longest first, so a username that is a prefix of another is not split.
    let mut keys: Vec<&String> = names.0.keys().collect();
    keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
    for k in keys {
        out = out.replace(k.as_str(), &names.0[k]);
    }
    out
}

fn assert_no_unregistered_uuid(fixture: &str, text: &str) {
    let re = |s: &str| {
        s.split(|c: char| !(c.is_ascii_hexdigit() || c == '-'))
            .find(|w| w.len() == 36 && Uuid::parse_str(w).is_ok())
            .map(str::to_string)
    };
    if let Some(u) = re(text) {
        panic!("{fixture}: unregistered uuid {u} in the golden output — register it in the fixture");
    }
}

pub async fn capture(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    profiles: &[(i64, Option<i64>)],
) -> Value {
    let mut out = Vec::new();
    for &(sid, tag_sid) in profiles {
        let compile = egret_perm_compile::recompile_service_in_tx(tx, sid).await;
        let mut entry = serde_json::Map::new();
        match compile {
            Err(e) => {
                entry.insert("error".into(), json!(e.to_string()));
            }
            Ok(summary) => {
                let g = egret_perm_compile::harness_support::golden_desired_in_tx(&mut **tx, sid)
                    .await
                    .expect("golden desired")
                    .expect("a compiler-targeted profile");
                entry.insert(
                    "desired".into(),
                    json!(g.desired.iter().map(|(s, n, b)| json!({"service": s, "name": n, "body": canonical(b)})).collect::<Vec<_>>()),
                );
                entry.insert(
                    "skipped".into(),
                    json!(summary.skipped.iter().map(|s| json!({"kind": format!("{:?}", s.source_kind), "id": s.id, "reason": s.reason})).collect::<Vec<_>>()),
                );
                for (key, id) in [("served", Some(sid)), ("servedTag", tag_sid)] {
                    let Some(id) = id else { continue };
                    let rows = egret_policy::enabled_policies_for_service_via(&mut **tx, id)
                        .await
                        .expect("served");
                    let rows: Vec<Value> = rows
                        .into_iter()
                        .enumerate()
                        .map(|(i, mut b)| {
                            b["id"] = json!(i); // DB sequence → ordinal; order is kept
                            canonical(&b)
                        })
                        .collect();
                    entry.insert(key.into(), json!(rows));
                }
            }
        }
        out.push(Value::Object(entry));
    }
    json!(out)
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

pub fn compare_or_bless(fixture: &str, captured: Value, names: &Names) {
    let text = normalise(serde_json::to_string_pretty(&captured).unwrap(), names);
    assert_no_unregistered_uuid(fixture, &text);
    let path = golden_dir().join(format!("{fixture}.json"));
    if std::env::var_os("EGRET_GOLDEN_BLESS").is_some() {
        std::fs::write(&path, format!("{text}\n")).expect("bless");
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("{fixture}: no golden at {path:?}; bless it on unchanged code"));
    if want.trim_end() != text {
        let actual = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/golden-actual")
            .join(format!("{fixture}.json"));
        std::fs::create_dir_all(actual.parent().unwrap()).ok();
        std::fs::write(&actual, &text).ok();
        panic!("{fixture}: compiled artifacts differ from the golden; diff {path:?} {actual:?}");
    }
}

#[tokio::test]
async fn every_golden_fixture_compiles_to_its_golden_bytes() {
    let dsn = egret_test_db::url().await.expect("a test database");
    let db = Db::connect(&dsn, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    fixtures::seed_service_defs(&db).await;
    let mut ran = 0;
    for f in fixtures::ALL {
        (f.run)(&db).await;
        ran += 1;
    }
    assert_eq!(ran, fixtures::ALL.len());
    assert!(ran >= 21, "the corpus shrank: {ran} fixtures");
}
```

`crates/egret-perm-compile/tests/golden/fixtures.rs` defines the shared builders and `pub const ALL: &[Fixture]`. Each `Fixture { name: &'static str, run: fn(&Db) -> BoxFuture<'_, ()> }` follows the same shape:

```rust
pub struct Fixture {
    pub name: &'static str,
    pub run: for<'a> fn(&'a Db) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>,
}

pub async fn seed_service_defs(db: &Db) {
    let policy = egret_policy::PolicyStore::new(db.clone());
    egret_policy::seed_trino(&policy).await.expect("seed trino");
    egret_policy::seed_starrocks(&policy).await.expect("seed starrocks");
    egret_policy::seed_tag(&policy).await.expect("seed tag");
}

/// A profile (committed: `create_service` uses its own pool) and, for Trino, its
/// tag service. Fixed names so the output is stable; one fixture, one prefix.
pub async fn profile(db: &Db, prefix: &str, engine: &str, tag: bool, names: &mut crate::Names) -> (i64, Option<i64>, String) {
    let policy = egret_policy::PolicyStore::new(db.clone());
    let name = format!("gold_{prefix}_{engine}");
    names.text(&format!("{prefix}.profile"), &name);
    let tag_id = if tag {
        let tag_name = format!("{name}_tag");
        let t = policy.create_service(&tag_name, "tag", &serde_json::json!({})).await.expect("tag svc");
        Some(t.id)
    } else {
        None
    };
    let cfg = if tag { serde_json::json!({ "tagService": format!("{name}_tag") }) } else { serde_json::json!({}) };
    let p = policy.create_service(&name, engine, &cfg).await.expect("profile");
    (p.id, tag_id, name)
}
```

The other builders (`warehouse`, `engine_bound`, `catalog_attached`, `user`, `role_held`, `workspace_member`, `group_member`, `tag`, `tag_column`, `tag_table`, `gate`, `grant`, `rule`, `engine_grant`, `operator_policy`, `seed_default_trino_policies`, `delegator_policies`, `held_rename`) each take `&mut sqlx::Transaction` plus `&mut Names`. They insert with the SQL already used by `tests/clearance_starrocks.rs:160-260`, `tests/metadata_tables.rs:40-123` and `egret_perm::store::{create_grant_in_tx, create_rule_in_tx}` (`store.rs:366`, `566`), and they register every returned id. A fixture body:

```rust
pub fn g08_tag_mask_trino_and_starrocks(db: &Db) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
    Box::pin(async move {
        let mut names = crate::Names::default();
        let (tr, tr_tag, _) = profile(db, "g08", "trino", true, &mut names).await;
        let (sr, _, _) = profile(db, "g08", "starrocks", false, &mut names).await;
        let mut tx = db.pool.begin().await.expect("tx");
        let wh = warehouse(&mut tx, "g08", &mut names).await;
        let cat = catalog_attached(&mut tx, "g08", wh, &[(tr, "trino"), (sr, "starrocks")], &mut names).await;
        let tag = tag(&mut tx, "g08.pii", &mut names).await;
        tag_column(&mut tx, tag, wh, "sales", "orders", "email").await;
        tag_column(&mut tx, tag, wh, "sales", "orders", "phone").await;
        tag_table(&mut tx, tag, wh, "sales", "customers").await;
        let alice = user(&mut tx, "g08.alice", "g08_alice", &mut names).await;
        let _ = (cat, alice);
        rule(&mut tx, "g08.mask", tag_mask(&tag_key("g08.pii"), "MASK_HASH", None, &[user_ref("g08_alice")]), &mut names).await;
        let captured = crate::capture(&mut tx, &[(tr, tr_tag), (sr, None)]).await;
        tx.rollback().await.expect("rollback");
        crate::compare_or_bless("G08_tag_mask_trino_and_starrocks", captured, &names);
    })
}
```

The corpus (each line is one fixture):

- G01: Trino `allow read` on a catalog, two users.
- G02: Trino `deny read` on a schema with an exclude user (`denyExceptions`).
- G03: StarRocks `allow write` on a table with an exclude (`allowExceptions`, `database` level, `serviceType: starrocks`, `insert/update/delete`).
- G04: the implicit workspace grant with one member who has no username (mint), plus an empty workspace (empty-audience skip).
- G05: group and role principals, plus `public`, plus a plain-user list with a duplicate (no-expansion early return).
- G06: path masks (column) on Trino and StarRocks, `CUSTOM` with `{col}`, `MASK_NULL`, exemptions by user and group.
- G07: path row filters on a table and a schema (wildcard levels).
- G08: a tag mask on Trino (tag service) and StarRocks (per-column, whole-table attachment).
- G09: a tag row filter: Trino column-only plus table-level attachments (KI-287), StarRocks per-table.
- G10: a Trino profile without `tagService` (tag rule skip, clearance skip), a dangling tag key, and a `tagService` naming nothing.
- G11: Trino clearance by role, groups-only clearance (skip), and a role with no holders.
- G12: StarRocks clearance (column, whole known table, view, unknown table), MV deny, a view-guard fold with another rule mask and a hand-written mask.
- G13: engine grants: Trino session property, impersonation (ok, reserved `root` skip, deleted target), StarRocks blacklist, empty-principal skip.
- G14: metadata-table denies on Trino (#821): resource mask, row filter, a deny grant, a tag mask, a clearance, and a hand-written restricting policy.
- G15: hidden columns on StarRocks: an exact mask, a wildcard-table mask, and a hand-written mask.
- G16: hand-written plus infra: `seed_default_trino_policies`, egret-admin (an engine-admin user), delegator policies, one operator policy, one disabled operator policy.
- G17: several engines per profile: two StarRocks engines at 2.8.0 and 2.9.0 (fold agreement); then an engine with an unknown image (M13 not-measured skip); two catalogs.
- G18: held renames (view-rename patterns): a table hold with leaving coordinates; a grant body staying on the old name; tag-rule bodies leaving.
- G19: ceilings: a StarRocks tag mask on 501 columns (`ExpansionCeiling` error text) and a clearance over the cap.
- G20: an overlap refusal: two path row filters on one table with different expressions.
- G21: the large profile for timing (StarRocks: 480 tagged columns under one mask, 400 under one clearance, 300 grants).

- [ ] **Step 2: Add the harness hook and the shadow.**

In `crates/egret-perm-compile/src/trigger.rs`, change `async fn compile_desired` (`:379`) to `pub(crate) async fn compile_desired`, `struct Compiled` (`:922`) to `pub(crate) struct Compiled` with `pub(crate)` fields, and `struct DesiredPolicy` (`:283`) likewise. In `recompile_locked_service_in_tx` (`:1653`), replace the compile line:

```rust
    let Some(compiled) = compile_desired(tx, service_id).await? else {
        return Ok(CompileSummary::default());
    };
    #[cfg(feature = "shadow-compile")]
    crate::shadow::compare(tx, service_id, &compiled).await?;
```

`crates/egret-perm-compile/src/shadow.rs`:

```rust
//! `shadow-compile`: every recompile compiles a second time on the same
//! transaction and asserts byte-identical output. During a sub-project 1
//! family task the second compile is the OLD path; between tasks it is the same
//! path (a determinism check, the precondition for a golden).
use crate::trigger::{compile_desired, Compiled};
use egret_perm::CompileError;

fn bytes(c: &Compiled) -> String {
    let desired: Vec<_> = c
        .desired
        .iter()
        .map(|((sid, name), dp)| (sid, name, serde_json::to_string(&dp.body).unwrap(), dp.source_kind as u8, dp.grant_id, dp.rule_id))
        .collect();
    let skipped: Vec<_> = c.skipped.iter().map(|s| (s.source_kind as u8, s.id, s.reason.clone())).collect();
    format!("{desired:?}\n{skipped:?}\n{:?}\n{}", c.sweep, c.uncounted)
}

pub(crate) async fn compare(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    first: &Compiled,
) -> Result<(), CompileError> {
    let second = compile_desired(conn, service_id)
        .await?
        .expect("the same profile compiled the first time");
    let (a, b) = (bytes(first), bytes(&second));
    assert_eq!(a, b, "shadow compile of service {service_id} differs");
    if let Some(path) = std::env::var_os("EGRET_TEST_SHADOW_LOG") {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).expect("shadow log");
        writeln!(f, "{service_id}").ok();
    }
    Ok(())
}
```

`SourceKind` gains `#[repr(u8)]`. In `harness_support.rs`:

```rust
pub struct GoldenCompiled {
    pub desired: Vec<(String, String, Value)>,
    pub skipped: Vec<(String, Uuid, String)>,
    pub sweep: Vec<i64>,
}

pub async fn golden_desired_in_tx(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Option<GoldenCompiled>, CompileError> {
    let Some(c) = crate::trigger::compile_desired(conn, service_id).await? else {
        return Ok(None);
    };
    let mut names: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
    for sid in &c.sweep {
        let n: String = sqlx::query_scalar("SELECT name FROM policy.authz_services WHERE id = $1")
            .bind(sid).fetch_one(&mut *conn).await.map_err(CompileError::from_db)?;
        names.insert(*sid, n);
    }
    Ok(Some(GoldenCompiled {
        desired: c.desired.iter().map(|((sid, name), dp)| (names[sid].clone(), name.clone(), dp.body.clone())).collect(),
        skipped: c.skipped.iter().map(|s| (format!("{:?}", s.source_kind), s.id, s.reason.clone())).collect(),
        sweep: c.sweep.clone(),
    }))
}
```

- [ ] **Step 3: Run the harness without goldens. Expected: FAIL** with `G01_…: no golden at …; bless it on unchanged code` for the first fixture.

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret \
  cargo test -p egret-perm-compile --features harness --test golden_compile
```

- [ ] **Step 4: Bless twice and prove the output is deterministic.**

```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
EGRET_GOLDEN_BLESS=1 cargo test -p egret-perm-compile --features harness --test golden_compile
cp -r crates/egret-perm-compile/tests/golden /tmp/golden-1   # scratch copy
EGRET_GOLDEN_BLESS=1 cargo test -p egret-perm-compile --features harness --test golden_compile
diff -r /tmp/golden-1 crates/egret-perm-compile/tests/golden && echo DETERMINISTIC
```

Expected: `DETERMINISTIC`. If the diff is not empty, the difference names the nondeterministic field. Fix the **fixture** (an unregistered id or a random name), never the compiler, in this task.

Then open every golden file and check it against the fixture's intent. Each golden is the evidence and must show the shape its line in the corpus list promises; for example, G09 must hold both the tag row filter and `perm-rule-{g09.rf}-…orders` for the column-only table.

- [ ] **Step 5: Run the harness compare mode. Expected: PASS** (`1 passed`, a runtime of seconds, not `0.00s`).

- [ ] **Step 6: Run the determinism shadow over the workspace. Expected: PASS**, with the log holding at least one line per recompiling test.

```bash
EGRET_TEST_SHADOW_LOG=/tmp/shadow.log cargo test --workspace --features egret-perm-compile/shadow-compile --no-fail-fast
wc -l /tmp/shadow.log
```

Record the count in the commit body. If a known flake (`TAG_FANOUT_LOCK` tests, `table_hook_db`) fails, re-run that binary alone. A shadow assertion failure is a real nondeterminism. Stop and report it: it blocks the whole plan.

- [ ] **Step 7: Take the performance baseline.** In `golden_compile.rs`, add an `#[ignore]` test `g21_compile_timing` that builds G21 and calls `golden_desired_in_tx` 7 times, printing the median.

```bash
cargo test -p egret-perm-compile --release --features harness --test golden_compile -- --ignored --nocapture g21_compile_timing
```

Record `median=…s` in the commit body as the Task 11 baseline.

- [ ] **Step 8: Write the demo-stack recipe** `scripts/golden-demo-capture.sh`:

```bash
#!/usr/bin/env bash
# Capture every profile's download, raw bytes, after a converging recompile.
# Usage: EGRET_URL=http://localhost:18080 EGRET_TOKEN=… scripts/golden-demo-capture.sh OUT_DIR
set -euo pipefail
out="${1:?output dir}"; mkdir -p "$out"
api="$EGRET_URL/api/control-plane"
auth=(-H "Authorization: Bearer $EGRET_TOKEN")
curl -fsS "${auth[@]}" "$api/policies/services" \
  | jq -r '.[] | select(.type=="trino" or .type=="starrocks") | "\(.id) \(.name)"' \
  | while read -r id name; do
      curl -fsS -X POST "${auth[@]}" "$api/perm/compile/$id" > "$out/$name.compile.json"
      curl -fsS "${auth[@]}" "$EGRET_URL/ranger/service/plugins/policies/download/$name" > "$out/$name.download.json"
      sha256sum "$out/$name.download.json" >> "$out/SHA256SUMS"
    done
jq -s 'map(.created + .updated + .deleted) | add' "$out"/*.compile.json
```

Run it twice on the current stack. The second run must print `0`, and `SHA256SUMS` must be identical between the two runs. Record both results in the commit body.

- [ ] **Step 9: Wire the gates.** In `justfile`, add:

```make
# Governance SP1: the golden corpus. Needs the `harness` feature, so
# `cargo test --workspace` never runs it.
test-golden:
    DATABASE_URL={{ test_database_url }} cargo test -p egret-perm-compile --features harness --test golden_compile
```

Add `just test-golden` to `check`, right after the `cargo test --workspace` line. In `.github/workflows/ci.yml` (rust job, after "Test"), add:

```yaml
      - name: Golden compile corpus
        run: cargo test -p egret-perm-compile --features harness --test golden_compile
```

- [ ] **Step 10: fmt, clippy, crate tests, lock sites.**

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets --features egret-perm-compile/shadow-compile,egret-perm-compile/harness -- -D warnings
cargo test -p egret-perm-compile --test profile_lock_sites
```

- [ ] **Step 11: Commit.**

```bash
git add crates/egret-perm-compile scripts/golden-demo-capture.sh justfile .github/workflows/ci.yml
git commit -m "test(perm): golden corpus and determinism shadow for the compiler

Captures 21 fixtures' compiled and served Ranger artifacts on unchanged code.
Shadow comparisons: <N>. G21 median: <t>s.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: `egret-perm-ir` crate, capability matrix, `NotEnforced` from the matrix

**Files:**
- Create: `crates/egret-perm-ir/{Cargo.toml, src/lib.rs, src/ep.rs, src/matrix.rs, src/enforcement.rs, src/statement.rs (only `ReasonCode` and `Privilege` now), tests/boundary.rs}`.
- Modify:
  - `Cargo.toml` (workspace dep `egret-perm-ir = { path = "crates/egret-perm-ir" }`).
  - `crates/egret-perm-compile/Cargo.toml` (dep).
  - `crates/egret-perm-compile/src/effect.rs:390-497`: replace `type Attached`, the SQL tag-capable column, `enforcement()` and `catalog_not_enforced`.
  - `crates/egret-perm-compile/src/people.rs`: at `:1023`, `:1037`, `:1234`, `:1242`, `:1322`, the groups-only decision comes from `egret_perm_ir::enforcement::withhold_engine_reason`.
  - `crates/egret-perm-compile/src/rules.rs:825-865`: `declared_mask_types` stays; a test pins it against M19.

**Interfaces:**
- Produces:
  - `pub fn egret_perm_ir::matrix::support(ep: &EpInput, cap: &Capability) -> Support`
  - `pub fn egret_perm_ir::matrix::support_for_kind(kind: EpKind, version: Option<&str>, classification_channel: bool, cap: &Capability) -> Support`
  - `pub fn egret_perm_ir::enforcement::rule_enforcement(reached: &[(Uuid, String)], attached: &[AttachedEngine], classification_selector: bool, effect: RuleEffectKind) -> (Vec<CatalogEnforcement>, u32, Option<NotEnforcedReason>)`
  - `pub fn egret_perm_ir::enforcement::withhold_engine_reason(required_roles_empty: bool, required_groups_empty: bool) -> Option<ReasonCode>`
- Consumes: `egret_perm::EngineType`.

- [ ] **Step 1: Write the failing matrix tests** in `crates/egret-perm-ir/src/matrix.rs` (`#[cfg(test)]`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use egret_perm::EngineType;

    fn trino() -> EpKind { EpKind { engine: EngineType::Trino, mechanism: Mechanism::Ranger } }
    fn starrocks() -> EpKind { EpKind { engine: EngineType::StarRocks, mechanism: Mechanism::Ranger } }

    #[test]
    fn a_trino_classification_mask_needs_the_channel() {
        let cap = Capability::ClassificationSelector(RuleEffectKind::Mask);
        assert_eq!(support_for_kind(trino(), None, true, &cap), Support::Native);
        assert_eq!(
            support_for_kind(trino(), None, false, &cap),
            Support::Missing(ReasonCode::NoClassificationChannel)
        );
    }

    #[test]
    fn starrocks_expands_classification_rules() {
        assert_eq!(
            support_for_kind(starrocks(), None, false, &Capability::ClassificationSelector(RuleEffectKind::Mask)),
            Support::Lowered(LoweringKind::ExpandPerColumn)
        );
        assert_eq!(
            support_for_kind(starrocks(), None, false, &Capability::ClassificationSelector(RuleEffectKind::RowFilter)),
            Support::Lowered(LoweringKind::ExpandPerTable)
        );
    }

    #[test]
    fn the_view_guard_is_measured_at_two_starrocks_versions_only() {
        let cap = Capability::WithholdThroughView;
        for v in ["2.8.0", "2.9.0"] {
            assert_eq!(support_for_kind(starrocks(), Some(v), false, &cap), Support::Lowered(LoweringKind::ViewGuardMask));
        }
        assert_eq!(
            support_for_kind(starrocks(), Some("2.10.0"), false, &cap),
            Support::Missing(ReasonCode::NotMeasured)
        );
    }

    #[test]
    fn an_unknown_combination_is_missing_not_measured() {
        assert_eq!(
            support_for_kind(trino(), None, true, &Capability::MaskCoversHiddenColumns),
            Support::Missing(ReasonCode::NotMeasured)
        );
    }

    #[test]
    fn group_only_withhold_exemptions_are_missing_on_ranger() {
        for k in [trino(), starrocks()] {
            assert_eq!(
                support_for_kind(k, None, true, &Capability::WithholdExemptionByGroup),
                Support::Missing(ReasonCode::WithholdExemptionByGroupOnly)
            );
        }
    }

    #[test]
    fn starrocks_withholds_read_only() {
        assert_eq!(support_for_kind(starrocks(), None, false, &Capability::WithholdPrivilege(Privilege::Read)), Support::Native);
        assert_eq!(
            support_for_kind(starrocks(), None, false, &Capability::WithholdPrivilege(Privilege::Write)),
            Support::Missing(ReasonCode::NotMeasured)
        );
        assert_eq!(support_for_kind(trino(), None, true, &Capability::WithholdPrivilege(Privilege::Write)), Support::Native);
    }

    #[test]
    fn every_entry_names_its_evidence() {
        for e in ENTRIES {
            assert!(e.evidence.contains(".rs:") || e.evidence.contains(".tsx"), "{:?} has no evidence", e.capability);
        }
    }
}
```

`MaskCoversHiddenColumns` is deliberately absent for Trino in the test above, and `support` answers `NotMeasured`. Task 10 asks only StarRocks.

Add `crates/egret-perm-ir/tests/boundary.rs`:

```rust
//! The IR is target-neutral: no Ranger dependency, no Ranger vocabulary, no I/O.
#[test]
fn the_ir_depends_on_no_target_and_no_database() {
    let manifest = include_str!("../Cargo.toml");
    for forbidden in ["egret-policy", "egret-policy-api", "egret-ranger-eval", "sqlx", "tokio"] {
        assert!(!manifest.contains(forbidden), "egret-perm-ir must not depend on {forbidden}");
    }
}

#[test]
fn no_public_ranger_vocabulary() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() { stack.push(p); continue; }
            let text = std::fs::read_to_string(&p).unwrap();
            for line in text.lines().filter(|l| l.trim_start().starts_with("pub ")) {
                for word in ["Ranger", "Policy", "policyItems", "tagService", "serviceType"] {
                    if word == "Ranger" && line.contains("Mechanism") { continue; }
                    assert!(!line.contains(word), "{p:?}: public item uses {word:?}: {line}");
                }
            }
        }
    }
}
```

`Mechanism::Ranger` is the one allowed spelling: it names the mechanism, which is what an EP kind is.

In `crates/egret-perm-compile/src/rules.rs` tests, add:

```rust
#[test]
fn matrix_mask_types_equal_the_service_def() {
    use egret_perm_ir::matrix::{support_for_kind, Capability, EpKind, Mechanism, Support};
    for e in [EngineType::Trino, EngineType::StarRocks] {
        for t in declared_mask_types(e).unwrap() {
            let k = EpKind { engine: e, mechanism: Mechanism::Ranger };
            assert_eq!(support_for_kind(k, None, true, &Capability::MaskType(t.clone())), Support::Native, "{e:?} {t}");
        }
        let k = EpKind { engine: e, mechanism: Mechanism::Ranger };
        assert_ne!(support_for_kind(k, None, true, &Capability::MaskType("MASK_NOT_A_TYPE".into())), Support::Native);
    }
}
```

In `crates/egret-perm-compile/src/clearance_mask.rs` tests, add `matrix_view_guard_versions_equal_the_modelled_starrocks_plugins`. It asserts that the `Versions::Exactly` list of M13 equals the StarRocks pairs of `egret_perm::MODELLED_RANGER_PLUGINS`.

- [ ] **Step 2: Run the tests. Expected: FAIL** (compile errors: crate `egret_perm_ir` not found).

```bash
cargo test -p egret-perm-ir && cargo test -p egret-perm-compile --lib rules::tests::matrix_mask_types_equal_the_service_def
```

- [ ] **Step 3: Implement.**

`crates/egret-perm-ir/Cargo.toml`:

```toml
[package]
name = "egret-perm-ir"
version = "0.1.0"
edition.workspace = true
license.workspace = true

# Governance sub-project 1: the target-neutral IR between the `perm` store and
# every compiler target. Pure: no database, no Ranger. See
# docs/en/specs/2026-11-02-governance-layer-concepts.md.
[dependencies]
egret-perm.workspace = true
uuid.workspace = true
serde.workspace = true
```

`ep.rs` follows D2. `statement.rs` (partial) holds `Privilege` and `ReasonCode` from D2. `matrix.rs`:

```rust
use egret_perm::EngineType;
use crate::ep::{EpInput, EpKind, Mechanism};
use crate::statement::{Privilege, ReasonCode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleEffectKind { Mask, RowFilter }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    ClassificationSelector(RuleEffectKind),
    WithholdByClassification,
    ClassificationRowFilterOnColumnAttachment,
    GroupPrincipal,
    RolePrincipal,
    WithholdExemptionByGroup,
    WithholdPrivilege(Privilege),
    WithholdThroughView,
    WithholdThroughMaterializedView,
    MaskCoversHiddenColumns,
    PoliciesReachMetadataTables,
    EngineAsksAboutMetadataTables,
    MaskType(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoweringKind {
    ExpandPerColumn,
    ExpandPerTable,
    ExpandColumnOnlyTablesPerTable,
    ExpandToUsers,
    ViewGuardMask,
    DenyCreateMaterializedView,
    DenyHiddenColumns,
    DenyMetadataTables,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support { Native, Lowered(LoweringKind), Missing(ReasonCode) }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer { Native, NativeWithChannel, Lowered(LoweringKind), Missing(ReasonCode) }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Versions { Any, Exactly(&'static [&'static str]) }

/// Which capabilities a row answers: a fixed one, or every mask type the
/// engine's own declaration lists (M19).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Fixed(fn(&Capability) -> bool),
    MaskTypes(&'static [&'static str]),
}

pub struct Entry {
    pub kind: EpKind,
    pub versions: Versions,
    pub capability: &'static str,
    key: Key,
    answer: Answer,
    pub evidence: &'static str,
}

const TRINO: EpKind = EpKind { engine: EngineType::Trino, mechanism: Mechanism::Ranger };
const STARROCKS: EpKind = EpKind { engine: EngineType::StarRocks, mechanism: Mechanism::Ranger };

/// The mask types each service-def declares (`service-defs/{trino,starrocks}.json`
/// `dataMaskDef.maskTypes`); pinned to the file by
/// `rules::tests::matrix_mask_types_equal_the_service_def`.
pub const TRINO_MASK_TYPES: &[&str] = &[/* copied from trino.json in Step 3; the pin test proves it */];
pub const STARROCKS_MASK_TYPES: &[&str] = &[/* copied from starrocks.json in Step 3 */];

pub const ENTRIES: &[Entry] = &[
    Entry { kind: TRINO, versions: Versions::Any, capability: "ClassificationSelector(Mask)",
        key: Key::Fixed(|c| *c == Capability::ClassificationSelector(RuleEffectKind::Mask)),
        answer: Answer::NativeWithChannel,
        evidence: "rules.rs:497-500 MissingTagService; trigger.rs:1173-1191; effect.rs:405-409" },
    Entry { kind: TRINO, versions: Versions::Any, capability: "ClassificationSelector(RowFilter)",
        key: Key::Fixed(|c| *c == Capability::ClassificationSelector(RuleEffectKind::RowFilter)),
        answer: Answer::NativeWithChannel, evidence: "rules.rs:497-500; effect.rs:405-409" },
    Entry { kind: TRINO, versions: Versions::Any, capability: "ClassificationRowFilterOnColumnAttachment",
        key: Key::Fixed(|c| *c == Capability::ClassificationRowFilterOnColumnAttachment),
        answer: Answer::Lowered(LoweringKind::ExpandColumnOnlyTablesPerTable), evidence: "rules.rs:502-536 (KI-287)" },
    Entry { kind: STARROCKS, versions: Versions::Any, capability: "ClassificationSelector(Mask)",
        key: Key::Fixed(|c| *c == Capability::ClassificationSelector(RuleEffectKind::Mask)),
        answer: Answer::Lowered(LoweringKind::ExpandPerColumn), evidence: "rules.rs:539-621" },
    Entry { kind: STARROCKS, versions: Versions::Any, capability: "ClassificationSelector(RowFilter)",
        key: Key::Fixed(|c| *c == Capability::ClassificationSelector(RuleEffectKind::RowFilter)),
        answer: Answer::Lowered(LoweringKind::ExpandPerTable), evidence: "rules.rs:567-585" },
    Entry { kind: TRINO, versions: Versions::Any, capability: "WithholdByClassification",
        key: Key::Fixed(|c| *c == Capability::WithholdByClassification),
        answer: Answer::NativeWithChannel, evidence: "clearance.rs:334-345" },
    Entry { kind: STARROCKS, versions: Versions::Any, capability: "WithholdByClassification",
        key: Key::Fixed(|c| *c == Capability::WithholdByClassification),
        answer: Answer::Lowered(LoweringKind::ExpandPerColumn), evidence: "clearance.rs:350-363, 420-605" },
    // M8, M9 for both kinds: GroupPrincipal / RolePrincipal → Lowered(ExpandToUsers),
    //   evidence "members.rs:690-692, 937-1013 (D7)" / "members.rs:174, 964-980 (D0)".
    // M10 for both kinds: WithholdExemptionByGroup → Missing(WithholdExemptionByGroupOnly),
    //   evidence "clearance.rs:289-299; people.rs:414,430,1023,1037,1234,1242,1322".
    // M11: TRINO WithholdPrivilege(Read|Write) → Native, "clearance.rs:1143-1146".
    // M12: STARROCKS WithholdPrivilege(Read) → Native, "clearance.rs:1123-1158" (Write: no row).
    // M13: STARROCKS Exactly(&["2.8.0","2.9.0"]) WithholdThroughView → Lowered(ViewGuardMask),
    //   "clearance_mask.rs:425-460; trigger.rs:609-629".
    // M15: STARROCKS WithholdThroughMaterializedView → Lowered(DenyCreateMaterializedView), "clearance.rs:971-1039".
    // M16: STARROCKS MaskCoversHiddenColumns → Lowered(DenyHiddenColumns), "trigger.rs:631-669; clearance.rs:654-800".
    // M17: TRINO PoliciesReachMetadataTables → Lowered(DenyMetadataTables), "trigger.rs:671-742; metadata_tables.rs".
    // M18: STARROCKS EngineAsksAboutMetadataTables → Missing(NotMeasured),
    //   "metadata_tables.rs:39-43; StarRocksMetadataTablesWarning.tsx".
    // M19: TRINO Key::MaskTypes(TRINO_MASK_TYPES), STARROCKS Key::MaskTypes(STARROCKS_MASK_TYPES) → Native, "rules.rs:825-865".
];
```

The implementer writes every commented row out as an `Entry` literal. The comments only list them here for review. They are not left as comments in the code.

```rust
fn version_matches(v: Versions, version: Option<&str>) -> bool {
    match (v, version) {
        (Versions::Any, _) => true,
        (Versions::Exactly(list), Some(x)) => list.contains(&x),
        (Versions::Exactly(_), None) => false,
    }
}

fn key_matches(k: Key, cap: &Capability) -> bool {
    match (k, cap) {
        (Key::Fixed(f), c) => f(c),
        (Key::MaskTypes(list), Capability::MaskType(t)) => list.contains(&t.as_str()),
        (Key::MaskTypes(_), _) => false,
    }
}

/// Spec D7: an absent combination is missing, reason "not measured".
pub fn support_for_kind(kind: EpKind, version: Option<&str>, classification_channel: bool, cap: &Capability) -> Support {
    let Some(e) = ENTRIES
        .iter()
        .find(|e| e.kind == kind && key_matches(e.key, cap) && version_matches(e.versions, version))
    else {
        return Support::Missing(ReasonCode::NotMeasured);
    };
    match e.answer {
        Answer::Native => Support::Native,
        Answer::NativeWithChannel if classification_channel => Support::Native,
        Answer::NativeWithChannel => Support::Missing(ReasonCode::NoClassificationChannel),
        Answer::Lowered(l) => Support::Lowered(l),
        Answer::Missing(r) => Support::Missing(r),
    }
}

/// Every bound engine's version must support `cap`; with none bound, egret's
/// default image decides (as `clearance_mask::fold_plugin_versions` does).
pub fn support(ep: &EpInput, cap: &Capability) -> Support {
    let versions: Vec<Option<&str>> = if ep.engines.is_empty() {
        vec![ep.default_version.as_deref().ok()]
    } else {
        ep.engines.iter().map(|e| e.version.as_deref().ok()).collect()
    };
    let mut answer = None;
    for v in versions {
        let s = support_for_kind(ep.kind, v, ep.classification_channel, cap);
        match answer {
            None => answer = Some(s),
            Some(a) if a == s => {}
            Some(_) => return Support::Missing(ReasonCode::VersionsDisagree),
        }
    }
    answer.unwrap_or(Support::Missing(ReasonCode::NotMeasured))
}
```

`enforcement.rs`:

```rust
use uuid::Uuid;
use egret_perm::EngineType;
use crate::ep::{EpKind, Mechanism};
use crate::matrix::{support_for_kind, Capability, RuleEffectKind, Support};
use crate::statement::ReasonCode;

/// An engine attached to a catalog, with its profile's kind when bound.
#[derive(Debug, Clone)]
pub struct AttachedEngine {
    pub catalog_id: Uuid,
    pub engine_id: Uuid,
    pub engine_name: String,
    pub engine_type: String,
    /// `None`: no profile. `Some(None)`: a profile of a type no EP kind models
    /// (today's SQL counts it as enforcing; kept, see Q2).
    pub profile: Option<Option<EngineType>>,
    pub classification_channel: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotEnforcedReason { NoAssignment, NoEngine, NoProfile, NoClassificationChannel }

#[derive(Debug, Clone)]
pub struct CatalogEnforcement {
    pub catalog_id: Uuid,
    pub catalog_name: String,
    pub engines: Vec<(Uuid, String, String)>,
    pub not_enforced: Option<NotEnforcedReason>,
}

fn enforces(a: &AttachedEngine, classification_selector: bool, effect: RuleEffectKind) -> bool {
    match a.profile {
        None => false,
        Some(None) => true,
        Some(Some(engine)) => {
            !classification_selector
                || !matches!(
                    support_for_kind(EpKind { engine, mechanism: Mechanism::Ranger }, None,
                        a.classification_channel, &Capability::ClassificationSelector(effect)),
                    Support::Missing(_)
                )
        }
    }
}

/// `effect::enforcement` (effect.rs:435-497), moved: scope outcomes plus the matrix.
pub fn rule_enforcement(
    reached: &[(Uuid, String)],
    attached: &[AttachedEngine],
    classification_selector: bool,
    effect: RuleEffectKind,
) -> (Vec<CatalogEnforcement>, u32, Option<NotEnforcedReason>) {
    let mut catalogs = Vec::with_capacity(reached.len());
    let mut engine_ids = std::collections::BTreeSet::new();
    for (catalog_id, catalog_name) in reached {
        let here: Vec<&AttachedEngine> = attached.iter().filter(|a| a.catalog_id == *catalog_id).collect();
        let bound = here.iter().any(|a| a.profile.is_some());
        let engines: Vec<(Uuid, String, String)> = here
            .iter()
            .filter(|a| enforces(a, classification_selector, effect))
            .map(|a| (a.engine_id, a.engine_name.clone(), a.engine_type.clone()))
            .collect();
        engine_ids.extend(engines.iter().map(|e| e.0));
        let not_enforced = if here.is_empty() {
            Some(NotEnforcedReason::NoEngine)
        } else if !bound {
            Some(NotEnforcedReason::NoProfile)
        } else if engines.is_empty() {
            Some(NotEnforcedReason::NoClassificationChannel)
        } else {
            None
        };
        catalogs.push(CatalogEnforcement { catalog_id: *catalog_id, catalog_name: catalog_name.clone(), engines, not_enforced });
    }
    let has = |r| catalogs.iter().any(|c| c.not_enforced == Some(r));
    let not_enforced = if catalogs.is_empty() {
        Some(NotEnforcedReason::NoAssignment)
    } else if !engine_ids.is_empty() {
        None
    } else if has(NotEnforcedReason::NoClassificationChannel) {
        Some(NotEnforcedReason::NoClassificationChannel)
    } else if has(NotEnforcedReason::NoProfile) {
        Some(NotEnforcedReason::NoProfile)
    } else {
        Some(NotEnforcedReason::NoEngine)
    };
    (catalogs, u32::try_from(engine_ids.len()).unwrap_or(u32::MAX), not_enforced)
}

/// M10: a withhold whose exemptions are groups only is not enforced on a
/// Ranger EP; one with roles enforces only its role half.
pub fn withhold_engine_reason(required_roles_empty: bool, required_groups_empty: bool) -> Option<ReasonCode> {
    let k = EpKind { engine: EngineType::Trino, mechanism: Mechanism::Ranger };
    match support_for_kind(k, None, true, &Capability::WithholdExemptionByGroup) {
        Support::Missing(r) if !required_groups_empty => {
            let _ = required_roles_empty;
            Some(r)
        }
        _ => None,
    }
}
```

In `effect.rs`, the `Attached` tuple becomes `egret_perm_ir::enforcement::AttachedEngine`. The SQL drops the computed boolean and returns `s.type` and the channel fact:

```sql
SELECT a.catalog_id, e.id, e.name, e.type, s.type AS profile_type,
       (jsonb_typeof(s.config_jsonb -> 'tagService') = 'string'
        AND EXISTS (SELECT 1 FROM policy.authz_services t
                     WHERE t.name = s.config_jsonb ->> 'tagService')) AS channel
  FROM registry.engine_catalog_attachments a
  JOIN engine_mgmt.engines e ON e.id = a.engine_id
  LEFT JOIN policy.authz_services s ON s.id = e.authz_service_id
 WHERE a.catalog_id = ANY($1) ORDER BY e.name, e.id
```

The row maps `profile: profile_type.map(|t| crate::trigger::engine_type_for(&t))` and `classification_channel: channel.unwrap_or(false)`. `fn enforcement` is deleted. Its two call sites (`:267`, `:339`) call `egret_perm_ir::enforcement::rule_enforcement(…, matches!(selector, Selector::Tag{..}), effect_kind_of(&rule.kind))` and map the results into `EffectCatalog`, `EffectEngine` and `NotEnforced`:

```rust
fn wire(r: egret_perm_ir::enforcement::NotEnforcedReason) -> NotEnforced {
    use egret_perm_ir::enforcement::NotEnforcedReason as R;
    match r {
        R::NoAssignment => NotEnforced::NoAssignment,
        R::NoEngine => NotEnforced::NoEngine,
        R::NoProfile => NotEnforced::NoProfile,
        R::NoClassificationChannel => NotEnforced::NoTagService,
    }
}
```

`catalog_not_enforced` (`:420-430`) keeps its body: it is scope only.

In `people.rs`, each of the five sites replaces its local "only groups" predicate with `egret_perm_ir::enforcement::withhold_engine_reason(roles.is_empty(), groups.is_empty()).is_some()`. Read each site first and keep its own surrounding control flow; only the predicate changes.

Add the Q2 pin test in `enforcement.rs`: `a_profile_of_an_unmodelled_type_still_counts_as_enforcing`.

- [ ] **Step 4: Run the tests. Expected: PASS.**

```bash
cargo test -p egret-perm-ir
cargo test -p egret-perm-compile --lib
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-perm-compile --test routes --test people --test classify_route
```

The `routes` / `people` integration tests assert the `notEnforced` and clearance wire values today. They must pass unchanged.

- [ ] **Step 5: Run the golden harness and check API identity.** No compile code changed, so the golden must be untouched. Regenerate the OpenAPI and confirm it is byte-identical:

```bash
just test-golden
cargo run -p egret-bin -- openapi > /tmp/openapi.json && diff -q /tmp/openapi.json openapi.json
git diff --exit-code frontend/src
```

If the repository regenerates `openapi.json` through a different recipe, use that recipe. Grep `justfile` for `openapi` first.

- [ ] **Step 6: fmt, clippy, lock sites, commit.**

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings
cargo test -p egret-perm-compile --test profile_lock_sites
git add Cargo.toml Cargo.lock crates/egret-perm-ir crates/egret-perm-compile
git commit -m "feat(perm): capability matrix in egret-perm-ir; NotEnforced generated from it

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Principal expansion moves into the IR (audience with provenance)

**Files:**
- Create: `crates/egret-perm-ir/src/audience.rs`.
- Modify: `crates/egret-perm-compile/src/members.rs:782-1021`. `expand_enumerated_principals` keeps every read and mint. It builds a `Directory` and calls `egret_perm_ir::audience::audience` + `Audience::to_principal_set` in place of `substitute`. `Expansions`, `substitute`, `substitute_one` and `as_users` are deleted.

**Interfaces:**
- Produces:
  - `pub fn egret_perm_ir::audience::audience(set: &PrincipalSet, dir: &Directory) -> Audience`
  - `pub fn egret_perm_ir::audience::audience_from_expanded(set: &PrincipalSet) -> Audience`: no directory. Role and WorkspaceMembers are kept as unexpanded members, for direct `compile_grant` calls.
  - `impl Audience { pub fn to_principal_set(&self, original: &PrincipalSet) -> PrincipalSet }`: the bridge for families not yet lowered. It returns `original.clone()` when no expandable principal is present (the early return). It is deleted in Task 10.
- Consumes: `egret_perm::{PrincipalSet, Principal, PrincipalKind, PUBLIC_GROUP}`.

- [ ] **Step 1: Write the failing tests** in `audience.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use egret_perm::{Principal, PrincipalKind, PrincipalSet};

    fn p(kind: PrincipalKind, r: &str) -> Principal { Principal { kind, r#ref: r.into() } }

    #[test]
    fn a_plain_user_list_is_left_alone_duplicates_included() {
        let set = PrincipalSet { include: vec![p(PrincipalKind::User, "a"), p(PrincipalKind::User, "a")], exclude: vec![] };
        let out = audience(&set, &Directory::default()).to_principal_set(&set);
        assert_eq!(out, set, "substitute_one returns early when nothing expands (members.rs:943-951)");
    }

    #[test]
    fn expansion_is_first_appearance_ordered_and_deduped() {
        let ws = uuid::Uuid::from_u128(7);
        let mut dir = Directory::default();
        dir.workspaces.insert(ws, vec!["b".into(), "c".into()]);
        dir.roles.insert("analyst".into(), vec!["c".into(), "d".into()]);
        dir.groups.insert("eng".into(), vec!["a".into()]);
        let set = PrincipalSet {
            include: vec![
                p(PrincipalKind::User, "a"),
                p(PrincipalKind::WorkspaceMembers, &ws.to_string()),
                p(PrincipalKind::Role, "analyst"),
                p(PrincipalKind::Group, "eng"),
                p(PrincipalKind::Group, "public"),
            ],
            exclude: vec![],
        };
        let a = audience(&set, &dir);
        let names: Vec<_> = a.include.iter().map(|m| m.name.clone()).collect();
        assert_eq!(names, vec![
            MemberName::User("a".into()), MemberName::User("b".into()),
            MemberName::User("c".into()), MemberName::User("d".into()), MemberName::Everyone,
        ]);
        assert_eq!(a.include[1].via, Via::WorkspaceMembers(ws));
        assert_eq!(a.include[3].via, Via::Role("analyst".into()));
    }

    #[test]
    fn an_unparseable_workspace_ref_expands_to_nobody() {
        let set = PrincipalSet { include: vec![p(PrincipalKind::WorkspaceMembers, "not-a-uuid")], exclude: vec![] };
        assert!(audience(&set, &Directory::default()).include.is_empty());
    }
}
```

The provenance of a deduped member is the **first** source that named them (`a` above stays `Direct`, not `Group("eng")`). That is what the order already encodes.

- [ ] **Step 2: Run them. Expected: FAIL** (module does not exist).

```bash
cargo test -p egret-perm-ir audience
```

- [ ] **Step 3: Implement.** The `audience` body is `substitute_one` (`members.rs:942-1013`), applied to `include` and `except` separately, with these changes:

- each expanded name becomes `Member { name: MemberName::User(n), via }`;
- `public` becomes `MemberName::Everyone`;
- a non-`public` `Group` with no directory entry expands to nobody, as today;
- the three `tracing::warn!` calls move with it, and `egret-perm-ir` gains `tracing.workspace = true` (logging is not I/O on the store);
- the early return becomes a flag, `expanded: bool`, stored on `Audience` (`pub(crate) touched: [bool; 2]`), which `to_principal_set` uses to return the original list unchanged.

`MemberName::User` keeps the user ref as given. Validation (`validate_user_ref`) stays where it is today, in the grant and rule compilers.

In `members.rs`, the tail of `expand_enumerated_principals` becomes:

```rust
    let dir = egret_perm_ir::audience::Directory {
        workspaces: members.into_iter().collect(),
        roles: role_members,
        groups: group_members,
    };
    for g in grants.iter_mut() {
        g.principals = egret_perm_ir::audience::audience(&g.principals, &dir).to_principal_set(&g.principals);
    }
    for r in rules.iter_mut() {
        r.exempt = egret_perm_ir::audience::audience(&r.exempt, &dir).to_principal_set(&r.exempt);
    }
    for g in engine_grants.iter_mut() {
        g.principals = egret_perm_ir::audience::audience(&g.principals, &dir).to_principal_set(&g.principals);
    }
    Ok(())
```

`members` changes from `HashMap<Uuid, Vec<String>>` to `BTreeMap`. Return the `Directory` too (signature `-> Result<Directory, CompileError>`); Task 4 consumes it. Delete `Expansions`, `substitute`, `substitute_one` and `as_users`. Move their unit tests, if any exist in `members.rs` `mod tests`, to `audience.rs`, rewritten against `audience`.

- [ ] **Step 4: Run the tests. Expected: PASS.**

```bash
cargo test -p egret-perm-ir && cargo test -p egret-perm-compile --lib
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-perm-compile --test workspace_members --test group_grants --test role_grants
```

- [ ] **Step 5: Run the golden harness and the determinism shadow.**

```bash
just test-golden
EGRET_TEST_SHADOW_LOG=/tmp/shadow3.log cargo test --workspace --features egret-perm-compile/shadow-compile --no-fail-fast -- --test-threads=1
```

Expected: golden PASS and no shadow assertion.

- [ ] **Step 6: Lock sites, fmt, clippy, commit.**

```bash
cargo test -p egret-perm-compile --test profile_lock_sites
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings
git commit -am "refactor(perm): expand principals in egret-perm-ir with provenance

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The loader: every read the lowering needs, in one place

**Files:**
- Create: `crates/egret-perm-compile/src/ir_load.rs`.
- Modify: `crates/egret-perm-compile/src/trigger.rs:379-473`. The head of `compile_desired` (profile, engine, catalogs, leaving, writable/sweep, tag service, model lists, expansion) is replaced by one `ir_load::load_ep` call. Everything after `:473` reads from the returned `Loaded`.

**Interfaces:**
- Produces:

```rust
pub(crate) struct Loaded {
    pub ep: egret_perm_ir::ep::EpInput,
    pub profile_name: String,
    pub tag_service: Option<String>,
    pub writable: std::collections::HashMap<String, i64>,
    pub sweep: Vec<i64>,
    pub leaving: crate::table_columns::Leaving,
    pub grants: Vec<egret_perm::Grant>,
    pub rules: Vec<egret_perm::Rule>,
    pub engine_grants: Vec<egret_perm::EngineGrant>,
    pub directory: egret_perm_ir::audience::Directory,
}
pub(crate) async fn load_ep(conn: &mut sqlx::PgConnection, service_id: i64) -> Result<Option<Loaded>, egret_perm::CompileError>;
```

- Consumes: `members::expand_enumerated_principals` (now returns `Directory`); `clearance_mask::fold_plugin_versions`'s engine read (`clearance_mask.rs:431-440`), lifted into `ir_load::bound_engines` so the EP's `engines` and `default_version` are read once.

- [ ] **Step 1: Write the failing test** in `ir_load.rs` (`#[cfg(test)]` cannot reach a DB, so it goes in the integration tests). Add to `crates/egret-perm-compile/tests/recompile.rs`:

```rust
#[tokio::test]
async fn the_loader_reads_the_ep_as_the_compiler_does() {
    let Some(db) = test_db().await else { return };
    let f = trino_fixture_with_tag_service(&db).await; // existing helper in this file
    let mut conn = db.pool.acquire().await.unwrap();
    let l = egret_perm_compile::harness_support::load_ep_for_test(&mut conn, f.service_id)
        .await.unwrap().expect("a trino profile");
    assert_eq!(l.ep.kind.engine, egret_perm::EngineType::Trino);
    assert!(l.ep.classification_channel);
    assert!(l.ep.catalogs.contains_key(&f.catalog_id));
    assert_eq!(l.sweep.len(), 2, "profile plus tag service");
}
```

If `recompile.rs` has no Trino-with-tag fixture helper, add the test to `tests/metadata_tables.rs` instead, using its `trino_fixture` (`:44-123`). Expose `load_ep_for_test` in `harness_support.rs` behind `feature = "harness"`, and run it with `--features harness`.

- [ ] **Step 2: Run it. Expected: FAIL** (`load_ep_for_test` not found).

```bash
DATABASE_URL=… cargo test -p egret-perm-compile --features harness --test metadata_tables the_loader_reads_the_ep
```

- [ ] **Step 3: Implement** `load_ep` by **moving** `trigger.rs:383-473` verbatim (same statements, same order), and adding:

- `catalogs` becomes a `BTreeMap<Uuid, String>`. The only lookup today is by key, so the order is irrelevant.
- `classification_channel = tag_service.is_some()`.
- `engines` and `default_version` are read with the query moved from `clearance_mask.rs:431-440`. `fold_plugin_versions` now takes `&EpInput` instead of reading. In this task its body still decides; Task 8 moves the decision.
- `directory` is the return value of `expand_enumerated_principals`.

`compile_desired` starts:

```rust
pub(crate) async fn compile_desired(conn: &mut sqlx::PgConnection, service_id: i64) -> Result<Option<Compiled>, CompileError> {
    let Some(l) = crate::ir_load::load_ep(conn, service_id).await? else { return Ok(None) };
    let engine = l.ep.kind.engine;
    // … the rest unchanged, reading l.profile_name / l.tag_service / l.writable / l.sweep / l.leaving / l.grants / l.rules / l.engine_grants / l.ep.catalogs
```

The engine-version query adds one SELECT per compile, for every profile. Today only StarRocks profiles with classified columns run it. To keep reads identical, `load_ep` runs it only when `engine == StarRocks`; on Trino it leaves `engines` empty and sets `default_version = Err("not read")`. No Trino path consults the versions in sub-project 1 (M13 is StarRocks-only). This is documented on the field.

- [ ] **Step 4: Run the test. Expected: PASS.** Then run the crate suite:

```bash
DATABASE_URL=… cargo test -p egret-perm-compile --features harness
```

- [ ] **Step 5: Run the golden harness and the determinism shadow** (as in Task 3, Step 5). Expected: PASS with no assertion.

- [ ] **Step 6: Lock sites, fmt, clippy, commit.**

```bash
git commit -am "refactor(perm): one loader for everything the compile reads

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Grants through the IR

**Files:**
- Create: `crates/egret-perm-ir/src/lower/mod.rs`, `crates/egret-perm-ir/src/lower/grants.rs`, the rest of `crates/egret-perm-ir/src/statement.rs` (D2), `crates/egret-perm-compile/src/ranger/mod.rs`, `crates/egret-perm-compile/src/ranger/grants.rs`.
- Modify: `crates/egret-perm-compile/src/grants.rs`. Validation (`validate_user_ref`, `ensure_scope_levels_are_named`, `level_is_a_wildcard_run`, `ensure_a_grant_wildcard_level_has_nothing_named_beneath_it`, `validate_grant_shape`) moves to `egret-perm-ir/src/lower/grants.rs`. Rendering (`principal_item`, `expanded_principal_item`, `resource_level_names`, `level`, `WILDCARD`, `PolicyTarget`, `scope_to_resources`, `ROW_FILTER_LEVELS`) moves to `ranger/grants.rs`. `grants.rs` is deleted (`git mv` the render half, then cut the decision half). `trigger.rs:486-555` becomes one call.
- Modify: `crates/egret-perm-compile/src/lib.rs`. `pub mod grants` becomes `pub(crate) mod ranger`. `pub use grants::{compile_grant, validate_grant_shape}` becomes `pub use ranger::grants::compile_grant` plus `pub use egret_perm_ir::lower::grants::validate_grant_shape`. Fix every `crate::grants::` import (in `rules.rs`, `clearance.rs`, `metadata_tables.rs`, `engine_grants.rs`, `grammar.rs`, `members.rs`, `principal_refs.rs`, `attribution.rs`) to `crate::ranger::grants::` or `egret_perm_ir::lower::grants::`.

**Interfaces:**
- Produces:
  - `pub fn egret_perm_ir::lower::grants::lower_grants(ep: &EpInput, grants: &[Grant]) -> LowerResult`: the principals were already expanded by the loader; the grant's audience is `audience_from_expanded`.
  - `pub fn egret_perm_ir::lower::grants::grant_statement(g: &Grant, catalog: &str, catalog_id: Uuid) -> Result<Statement, CompileError>`
  - `pub fn egret_perm_ir::lower::grants::validate_grant_shape(g: &Grant) -> Result<(), CompileError>`
  - `pub(crate) fn ranger::grants::render_grant(s: &Statement, service: &str, e: EngineType) -> Result<AccessPolicyInput, CompileError>`
  - `pub fn egret_perm_compile::compile_grant(g: &Grant, catalog_name: &str, service: &str, e: EngineType) -> Result<AccessPolicyInput, CompileError>` (unchanged signature) = `render_grant(&grant_statement(g, catalog_name, g.scope.catalog_id)?, service, e)`
  - `pub(crate) struct ranger::Rendered { pub body: serde_json::Value, pub name: String, pub source_kind: SourceKind, pub grant_id: Option<Uuid>, pub rule_id: Option<Uuid> }`
  - `pub(crate) fn ranger::skipped(l: &Lowering) -> SkippedItem`

- [ ] **Step 1: Write the failing IR tests** in `lower/grants.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use egret_perm::{Action, Effect, Grant, GrantOrigin, Principal, PrincipalKind, PrincipalSet, Scope};
    use crate::ep::{EpInput, EpKind, Mechanism};
    use crate::statement::*;

    fn ep(catalog: Uuid) -> EpInput {
        EpInput {
            profile_id: 1, profile_name: "p".into(),
            kind: EpKind { engine: egret_perm::EngineType::Trino, mechanism: Mechanism::Ranger },
            engines: vec![], default_version: Err("unread".into()), classification_channel: false,
            catalogs: [(catalog, "sales".to_string())].into_iter().collect(),
        }
    }
    fn grant(catalog: Uuid, include: Vec<Principal>, exclude: Vec<Principal>, effect: Effect) -> Grant {
        Grant {
            id: Uuid::from_u128(1), effect, actions: vec![Action::Read, Action::Write],
            scope: Scope { catalog_id: catalog, schema: Some("s".into()), table: None, column: None },
            principals: PrincipalSet { include, exclude }, origin: GrantOrigin::Explicit,
        }
    }
    fn user(n: &str) -> Principal { Principal { kind: PrincipalKind::User, r#ref: n.into() } }

    #[test]
    fn a_deny_with_an_exclude_lowers_to_deny_except() {
        let c = Uuid::from_u128(9);
        let out = lower_grants(&ep(c), &[grant(c, vec![user("a")], vec![user("b")], Effect::Deny)]).unwrap();
        let s = &out.statements[0];
        assert_eq!(s.effect, crate::statement::Effect::Deny { privileges: vec![Privilege::Read, Privilege::Write] });
        assert_eq!(s.audience.except[0].name, crate::audience::MemberName::User("b".into()));
        assert_eq!(s.target, Target::Path(Path {
            catalog_id: Some(c), catalog: "sales".into(),
            schema: Level::Named("s".into()), table: Level::Unnamed, column: Level::Unnamed,
        }));
    }

    #[test]
    fn a_grant_outside_the_scope_is_silently_absent() {
        let out = lower_grants(&ep(Uuid::from_u128(9)), &[grant(Uuid::from_u128(8), vec![user("a")], vec![], Effect::Allow)]).unwrap();
        assert!(out.statements.is_empty() && out.lowerings.is_empty());
    }

    #[test]
    fn an_empty_audience_is_a_lowering_with_todays_text() {
        let c = Uuid::from_u128(9);
        let out = lower_grants(&ep(c), &[grant(c, vec![], vec![], Effect::Allow)]).unwrap();
        assert_eq!(out.lowerings[0].code, ReasonCode::EmptyAudience);
        assert!(out.lowerings[0].detail.starts_with("the grant's principals name no engine principal"));
    }
}
```

In `crates/egret-perm-compile/src/ranger/grants.rs`, render tests move with `scope_to_resources`: the existing `grants.rs` `mod tests` (`:752-798`) moves there unchanged.

- [ ] **Step 2: Run them. Expected: FAIL** (`lower_grants` not found).

```bash
cargo test -p egret-perm-ir lower::grants
```

- [ ] **Step 3: Implement.**

`lower_grants` is the loop of `trigger.rs:486-555` with these changes:

- `catalog_names.get` becomes `ep.catalogs.get`.
- The empty-include skip becomes `Lowering { source: Source::Grant{grant_id}, code: EmptyAudience, detail: <the exact string at trigger.rs:491-494> }`, plus its `tracing::warn!`.
- `UnexpandedPrincipalKind` becomes `Lowering { code: UnexpandedPrincipal, detail: <exact format at :518-521> }`.
- Every other error is returned.

`grant_statement`:

```rust
pub fn grant_statement(g: &Grant, catalog: &str, catalog_id: Uuid) -> Result<Statement, CompileError> {
    ensure_scope_levels_are_named(&g.scope)?;
    ensure_a_grant_wildcard_level_has_nothing_named_beneath_it(&g.scope)?;
    for p in g.principals.include.iter().chain(&g.principals.exclude) {
        match p.kind {
            PrincipalKind::User => validate_user_ref(&p.r#ref)?,
            PrincipalKind::Group => {}
            PrincipalKind::Role | PrincipalKind::WorkspaceMembers => {
                return Err(CompileError::UnexpandedPrincipalKind { kind: p.kind })
            }
        }
    }
    let mut privileges: Vec<Privilege> = Vec::new();
    for a in &g.actions {
        let p = match a { Action::Read => Privilege::Read, Action::Write => Privilege::Write };
        if !privileges.contains(&p) { privileges.push(p); }
    }
    let level = |v: &Option<String>| v.clone().map_or(Level::Unnamed, Level::Named);
    Ok(Statement {
        source: Source::Grant { grant_id: g.id },
        audience: crate::audience::audience_from_expanded(&g.principals),
        target: Target::Path(Path {
            catalog_id: Some(catalog_id), catalog: catalog.to_string(),
            schema: level(&g.scope.schema), table: level(&g.scope.table), column: level(&g.scope.column),
        }),
        effect: match g.effect {
            egret_perm::Effect::Allow => Effect::Allow { privileges },
            egret_perm::Effect::Deny => Effect::Deny { privileges },
        },
        derivation: Derivation::AsDeclared,
    })
}
```

**Order trap.** Today validation runs scope checks first and then the user refs, inside `principal_item` in include → exclude order (`grants.rs:166-216`, `:694-720`). The check above keeps that order, so the first error raised is the same one.

**Access order trap.** `engine_accesses` is applied per action in the grant's action order, with dedupe (`grants.rs:690-699`). `render_grant` must expand `privileges` in the same order. `Privilege` dedupe preserves first appearance, so the access list is identical.

`ranger/grants.rs::render_grant` is `compile_grant`'s former body from `let mut accesses` onward:

- accesses come from `privileges` through `egret_perm::engine_accesses` (Read → `Action::Read`, Write → `Action::Write`);
- items come from `principal_item(&expanded refs…)`, rebuilt from `s.audience`: `MemberName::User(n)` → `users`, `Everyone` → `groups: ["public"]`, `UnexpandedGroup(g)` → `groups: [g]`, in member order;
- resources come from `scope_to_resources` on a `Scope` rebuilt from the `Path` (`Level::Named(v)` → `Some(v)`).

`principal_item` gains a sibling `item_from_members(&[Member], Effect, &[String]) -> AccessItemInput` that preserves the users / groups bucket order. Both are kept only while a consumer exists; `principal_item` is still used by `validate_grant_shape`'s `Expansion::NotYet`. Moving that check to the IR's own pre-expansion validator (`Role` allowed, `WorkspaceMembers` refused) removes the last caller, and `principal_item` is deleted.

In `trigger.rs`, replace `:486-555` with:

```rust
    let lowered = egret_perm_ir::lower::grants::lower_grants(&l.ep, &l.grants)?;
    for s in &lowered.statements {
        let r = crate::ranger::grants::render(s, &l.profile_name, engine)?;
        let target = target_service_id(&r.body, &l.writable, &format!("grant {}", r.grant_id.unwrap()))?;
        desired.insert((target, r.name.clone()), r.into_desired());
    }
    skipped.extend(lowered.lowerings.iter().map(crate::ranger::skipped));
```

`ranger::grants::render` wraps `render_grant` + `access_to_ranger_json` + `stamp_service_type` + `Rendered { name: format!("{COMPILER_GRANT_PREFIX}{id}") … }`.

- [ ] **Step 4: Shadow old vs new, then delete the old path.**
  1. Keep the old grant loop as `fn legacy_grants(...)` in `trigger.rs` and the old `compile_grant` as `legacy_compile_grant`.
  2. In `shadow::compare`, compile the second time with an env flag `EGRET_SHADOW_LEGACY=grants` that makes `compile_desired` take the legacy loop. Use `#[cfg(feature = "shadow-compile")] fn legacy_enabled(family: &str) -> bool { std::env::var("EGRET_SHADOW_LEGACY").as_deref() == Ok(family) }`.
  3. Run:

     ```bash
     EGRET_SHADOW_LEGACY=grants EGRET_TEST_SHADOW_LOG=/tmp/shadow5.log \
       cargo test --workspace --features egret-perm-compile/shadow-compile --no-fail-fast -- --test-threads=1
     wc -l /tmp/shadow5.log
     ```

     Expected: no shadow assertion. Record the count.
  4. Delete `legacy_grants`, `legacy_compile_grant` and the `legacy_enabled` branch for grants.

- [ ] **Step 5: Run the gates. Expected: PASS.**

```bash
cargo test -p egret-perm-ir
DATABASE_URL=… cargo test -p egret-perm-compile --test compile_grants --test recompile --test group_grants --test grammar_access --test equivalence
just test-golden
EGRET_TEST_RANGER_E2E=1 cargo test -p egret-ranger-eval --test differential --no-fail-fast
cargo test -p egret-perm-compile --test profile_lock_sites
cargo test -p egret-perm-ir --test boundary
```

The Ranger differential is in the list because it calls `compile_grant`; it needs a JVM and Maven (`just test-ranger-gates`).

- [ ] **Step 6: fmt, clippy, commit.**

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings
git add -A crates && git commit -m "refactor(perm): grants lower to the neutral IR, Ranger renders shape only

Shadow comparisons (legacy vs IR): <N>.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Rules (masks, row filters) through the IR and the matrix

**Files:**
- Create: `crates/egret-perm-ir/src/lower/rules.rs`, `crates/egret-perm-compile/src/ranger/rules.rs`.
- Modify: `crates/egret-perm-compile/src/rules.rs`.
  - The decisions move to `lower/rules.rs`: `validate_rule_shape`, `ensure_row_filter_scope_is_a_table`, the selector/engine match in `compile_rule_counting` (`:453-634`), `ensure_custom_mask_parses`, `invalid_custom_mask`, the `exempt_principals` refusals, `TaggedColumn::new` validation, and the ceiling checks.
  - The rendering moves to `ranger/rules.rs`: `column_resources`, `table_resources`, `table_row_filter_name`, `policy_target`, `build_body`, `build_tag_body`, `stamp_service_type`, `declared_mask_types`, `mask_items`, `rowfilter_items`, `PUBLIC_GROUP`.
  - `group_by_table`, `TableTarget` and `ResolvedNames` stay in `rules.rs` (loader-side and public API). `rules.rs` keeps only these and the public `compile_rule` wrapper.
- Modify: `crates/egret-perm-compile/src/trigger.rs:569-584` and `:1027-1235` (`compile_one_rule` deleted). `crates/egret-perm-compile/src/ir_load.rs` gains `load_rule_inputs`.

**Interfaces:**
- Produces:
  - `pub struct egret_perm_ir::lower::rules::RuleInputs { pub classification_name: Option<String>, pub tagged: Vec<TaggedTable> }`
  - `pub struct TaggedTable { pub catalog: String, pub schema: String, pub table: String, pub table_level: bool, pub columns: Vec<String> }`
  - `pub trait CeilingCount { fn counts(&self, catalog: &str, schema: &str, table: &str, column: Option<&str>) -> bool; }`, implemented for `crate::table_columns::Leaving` in `egret-perm-compile`
  - `pub fn needs_tagged_tables(ep: &EpInput, r: &Rule) -> bool`: the matrix answers M3/M4/M5 ⇒ fetch
  - `pub fn lower_rules(ep: &EpInput, rules: &[Rule], inputs: &BTreeMap<Uuid, RuleInputs>, read_skips: BTreeMap<Uuid, Lowering>, ceiling: usize, counts: &dyn CeilingCount) -> LowerResult` (Task 10 inserts `dir: &Directory` after `rules`)
  - `pub fn lower_rule(r: &Rule, ep_kind: EpKind, classification_channel: bool, catalog: &str, inputs: &RuleInputs, ceiling: usize, counts: &dyn CeilingCount) -> Result<Vec<Statement>, CompileError>`
  - `pub(crate) fn ranger::rules::render(s: &Statement, rule: &Rule, service: &str, tag_service: Option<&str>, e: EngineType) -> Result<Value, CompileError>`
  - `pub(crate) async fn ir_load::load_rule_inputs(conn: &mut PgConnection, ep: &EpInput, rules: &[Rule]) -> Result<(BTreeMap<Uuid, RuleInputs>, Vec<Lowering>), CompileError>`: the dangling-tag-key skip is produced here, since it is a read result; it keeps the exact text from `trigger.rs:1058-1062`.
  - `pub fn compile_rule(...)` (unchanged signature) = `lower_rule` (with `classification_channel = tag_service.is_some()` and `inputs` from `resolved` / `tagged`) followed by `render` for each statement. `MissingTagService` is still an `Err` here: when `lower_rule` meets `Missing(NoClassificationChannel)` it returns `Err(CompileError::MissingTagService{rule})`, and `lower_rules` turns exactly that error into the `Lowering` with the text at `trigger.rs:1174-1177`.

- [ ] **Step 1: Write the failing IR tests** in `lower/rules.rs`. These are pure, with no DB:

```rust
#[test]
fn trino_classification_mask_is_one_native_statement() {
    let r = tag_mask_rule("pii", "MASK_HASH");
    let s = lower_rule(&r, trino(), true, "", &inputs("PII", vec![]), 500, &Always).unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].target, Target::Classification { name: "PII".into() });
    assert_eq!(s[0].derivation, Derivation::AsDeclared);
    assert_eq!(s[0].audience.include[0].name, MemberName::Everyone);
}

#[test]
fn trino_classification_row_filter_adds_column_only_tables_ki_287() {
    let r = tag_row_filter_rule("pii", "region = 'us'");
    let tagged = vec![table("c", "s", "orders", false, &["email"]), table("c", "s", "customers", true, &[])];
    let s = lower_rule(&r, trino(), true, "", &inputs("PII", tagged), 500, &Always).unwrap();
    assert_eq!(s.len(), 2, "the tag statement plus orders (column-only); customers stays the tag's");
    assert_eq!(s[1].derivation, Derivation::ExpandedFromClassification);
    assert!(matches!(&s[1].target, Target::Path(p) if p.table == Level::Named("orders".into()) && p.column == Level::Unnamed));
}

#[test]
fn starrocks_classification_mask_expands_per_column_and_refuses_over_the_ceiling() {
    let r = tag_mask_rule("pii", "MASK_NULL");
    let tagged = vec![table("c", "s", "t", false, &["a", "b", "c"])];
    assert_eq!(lower_rule(&r, starrocks(), false, "", &inputs("PII", tagged.clone()), 3, &Always).unwrap().len(), 3);
    assert!(matches!(
        lower_rule(&r, starrocks(), false, "", &inputs("PII", tagged), 2, &Always),
        Err(CompileError::ExpansionCeiling { count: 3, ceiling: 2, .. })
    ));
}

#[test]
fn a_trino_classification_rule_without_a_channel_is_missing_tag_service() {
    let r = tag_mask_rule("pii", "MASK_NULL");
    assert!(matches!(
        lower_rule(&r, trino(), false, "", &inputs("PII", vec![]), 500, &Always),
        Err(CompileError::MissingTagService { .. })
    ));
}

#[test]
fn a_held_rename_does_not_count_toward_the_ceiling() {
    struct Leaving;
    impl CeilingCount for Leaving {
        fn counts(&self, _: &str, _: &str, t: &str, _: Option<&str>) -> bool { t != "old" }
    }
    let r = tag_mask_rule("pii", "MASK_NULL");
    let tagged = vec![table("c", "s", "old", false, &["a", "b"]), table("c", "s", "new", false, &["a"])];
    assert_eq!(lower_rule(&r, starrocks(), false, "", &inputs("PII", tagged), 1, &Leaving).unwrap().len(), 3);
}
```

Write the helpers `trino()`, `starrocks()`, `tag_mask_rule`, `tag_row_filter_rule`, `table`, `inputs` and `struct Always` (`counts` → `true`) in the same `mod tests`. Also move the pure tests of today's `rules.rs` `mod tests` to `ranger/rules.rs` unchanged. They exercise `compile_rule`, whose signature stays.

- [ ] **Step 2: Run them. Expected: FAIL.**

```bash
cargo test -p egret-perm-ir lower::rules
```

- [ ] **Step 3: Implement.**

`lower_rule` is `compile_rule_counting`'s decision tree. The validation order is kept exactly:

1. the path-scope check;
2. `ensure_mask_type_declared` (through `matrix::support_for_kind(…, &Capability::MaskType(t))`, refusing `UnsupportedMaskType` on anything not `Native`);
3. the custom mask check;
4. then the selector match.

Exemptions become `Audience { include: [Everyone], except: members(rule.exempt) }`, and the `exempt_principals` refusals (`exclude` non-empty, exempt `public`, `Role`, `WorkspaceMembers`) run in `lower_rule` in their current order. The arm is chosen by `matrix::support_for_kind(kind, None, channel, &Capability::ClassificationSelector(effect))`:

- `Native` gives the classification statement. For a row filter, it also adds M3 (`ClassificationRowFilterOnColumnAttachment` → per-table path statements for `!table_level` tables, ceiling-counted).
- `Lowered(ExpandPerColumn | ExpandPerTable)` gives the StarRocks arms.
- `Missing(NoClassificationChannel)` gives `Err(MissingTagService)`.

`MissingTagName` stays checked before the channel, as today (`rules.rs:493-496`).

The `ranger::rules::render` naming:

- `AsDeclared` + `Path` → `perm-rule-{id}`;
- `AsDeclared` + `Classification` → `build_tag_body` (`perm-rule-{id}`, tag service);
- `ExpandedFromClassification` + a path with a column → `perm-rule-{id}-{c}.{s}.{t}.{col}` with `column_resources(…, Mask)`;
- the same without a column → `table_row_filter_name` with `table_resources`.

In `compile_desired`, replace the rule loop (`:569-584`) with:

```rust
    let (inputs, mut read_skips) = crate::ir_load::load_rule_inputs(conn, &l.ep, &l.rules).await?;
    let lowered = egret_perm_ir::lower::rules::lower_rules(&l.ep, &l.rules, &inputs, DEFAULT_EXPANSION_CEILING, &l.leaving)?;
    let by_id: HashMap<Uuid, &Rule> = l.rules.iter().map(|r| (r.id, r)).collect();
    for s in &lowered.statements {
        let egret_perm_ir::statement::Source::Rule { rule_id, rule_name } = &s.source else { unreachable!() };
        let body = crate::ranger::rules::render(s, by_id[rule_id], &l.profile_name, l.tag_service.as_deref(), engine)?;
        let what = format!("rule {rule_name:?}");
        let name = body_name(&body, &what)?;
        let target = target_service_id(&body, &l.writable, &what)?;
        desired.insert((target, name), DesiredPolicy { body, source_kind: SourceKind::Rule, grant_id: None, rule_id: Some(*rule_id) });
    }
```

**Skip order trap.** Today rule skips are interleaved per rule in rule order: the dangling key comes from the read, `MissingTagService` and `UnexpandedPrincipalKind` from the compile. `lower_rules` therefore takes `read_skips` as a `BTreeMap<Uuid, Lowering>` and emits, per rule in input order, either that rule's read skip or its lowering result. It does not append all read skips first. This is why `lower_rules` takes `read_skips` (see Interfaces).

- [ ] **Step 4: Shadow and delete** (as Task 5, Step 4) with `EGRET_SHADOW_LEGACY=rules`. Expected: no assertion. Delete `compile_one_rule` and the legacy branch.

- [ ] **Step 5: Run the gates. Expected: PASS.**

```bash
cargo test -p egret-perm-ir
DATABASE_URL=… cargo test -p egret-perm-compile --test compile_rules --test recompile --test tag_service_routing --test overlap --test grammar_rules --test equivalence --test metadata_tables
DATABASE_URL=… cargo test -p egret-bi --test tag_rename_recompiles
just test-golden
just test-ranger-gates
cargo test -p egret-perm-compile --test profile_lock_sites && cargo test -p egret-perm-ir --test boundary
```

- [ ] **Step 6: fmt, clippy, commit.**

```bash
git commit -am "refactor(perm): masks and row filters lower through the IR and the capability matrix

Shadow comparisons (legacy vs IR): <N>.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Clearance becomes the `withhold` effect in the IR

**Files:**
- Create: `crates/egret-perm-ir/src/lower/withhold.rs`, `crates/egret-perm-compile/src/ranger/withhold.rs`.
- Modify: `crates/egret-perm-compile/src/clearance.rs`.
  - The decisions in `compile_clearance` (`:236-401`) and `compile_starrocks_gates` (`:420-605`) move to `lower/withhold.rs`: the groups-only skip, the empty-gate skip, the no-channel skip, the whole-table / view / not-a-table branches, the ceiling, `largest`, and the MV intersection in `mv_deny_inputs` (`:1005-1039`).
  - The rendering moves to `ranger/withhold.rs`: `clearance_input`, `starrocks_deny`, `WHOLE_TABLE_COLUMN`, the MV resources, `withheld_accesses` (now `privileges` → accesses), `tag_access_to_ranger_json` use.
  - The reads (gates, role holders, tag assignments, current columns, views) move to `ir_load::load_withhold_inputs`.
  - `hidden_column_denies` and its helpers (`:654-970`) stay in `clearance.rs` until Task 10.
- Modify: `crates/egret-perm-compile/src/trigger.rs:600-614`, `crates/egret-perm-compile/src/harness_support.rs` (its `starrocks_*` wrappers call the new render functions).

**Interfaces:**
- Produces:
  - `pub struct egret_perm_ir::lower::withhold::Gate { pub classification_id: Uuid, pub classification_name: String, pub required_roles: Vec<String>, pub required_groups_empty: bool, pub cleared: Vec<String>, pub targets: Vec<GateTarget> }`
  - `pub struct GateTarget { pub table: TaggedTable, pub shape: TableShape }`
  - `pub enum TableShape { Known { schema: String, table: String, columns: Vec<String> }, View, Unknown }`
  - `pub struct WithholdLowered { pub lowered: Lowered, pub whole_tables: Vec<egret_perm::TableCount>, pub view_guard: Vec<PendingGuard> }`
  - `pub struct PendingGuard { pub classification_id: Uuid, pub column: [String; 4], pub cleared: Vec<String>, pub catalog_spelled: bool }`
  - `pub fn lower_withholds(ep: &EpInput, gates: &[Gate], fold: fn(&str) -> String, ceiling: usize, counts: &dyn CeilingCount) -> Result<WithholdLowered, CompileError>`
  - `pub(crate) async fn ir_load::load_withhold_inputs(conn: &mut PgConnection, ep: &EpInput, service_id: i64) -> Result<Vec<Gate>, CompileError>`
  - `pub(crate) fn ranger::withhold::render(s: &Statement, ep: &EpInput, service: &str, tag_service: Option<&str>) -> (String, Value)`
- `fold` is `egret_ranger_eval::resource::fold_identifier`, passed by the caller so the IR does not depend on the evaluator.

- [ ] **Step 1: Write the failing IR tests** in `lower/withhold.rs`:

```rust
#[test]
fn a_groups_only_gate_is_not_compiled_m10() {
    let g = gate("pii", &[], false, vec![]);
    let out = lower_withholds(&trino_ep(true), &[g], fold, 500, &Always).unwrap();
    assert!(out.lowered.statements.is_empty());
    assert_eq!(out.lowered.lowerings[0].code, ReasonCode::WithholdExemptionByGroupOnly);
}

#[test]
fn trino_withholds_read_and_write_on_the_classification() {
    let g = gate("pii", &["analyst"], true, vec!["ann".into()]);
    let s = &lower_withholds(&trino_ep(true), &[g], fold, 500, &Always).unwrap().lowered.statements[0];
    assert_eq!(s.effect, Effect::Withhold { privileges: vec![Privilege::Read, Privilege::Write] });
    assert_eq!(s.target, Target::Classification { name: "pii".into() });
    assert_eq!(s.audience.except[0].name, MemberName::User("ann".into()));
}

#[test]
fn starrocks_withholds_read_per_column_whole_table_and_guards_materialized_views() {
    let known = GateTarget { table: table("c", "s", "t", true, &[]), shape: TableShape::Known { schema: "s".into(), table: "t".into(), columns: vec!["a".into(), "b".into()] } };
    let g = gate_with("pii", &["analyst"], vec!["ann".into()], vec![known]);
    let out = lower_withholds(&starrocks_ep(), &[g], fold, 500, &Always).unwrap();
    let names: Vec<_> = out.lowered.statements.iter().map(|s| (s.derivation.clone(), s.target.clone())).collect();
    assert_eq!(names[0].0, Derivation::WholeTable);
    assert!(out.lowered.statements.iter().any(|s| s.derivation == Derivation::MaterializedViewGuard));
    assert!(out.lowered.statements.iter().all(|s| match &s.effect {
        Effect::Withhold { privileges } => privileges == &vec![Privilege::Read],
        Effect::Deny { privileges } => privileges == &vec![Privilege::CreateMaterializedView],
        _ => false,
    }));
    assert_eq!(out.view_guard.len(), 2, "a NULL mask pending for each known column");
}

#[test]
fn a_view_target_is_not_measured_m14() {
    let v = GateTarget { table: table("c", "s", "v", true, &[]), shape: TableShape::View };
    let out = lower_withholds(&starrocks_ep(), &[gate_with("pii", &["r"], vec![], vec![v])], fold, 500, &Always).unwrap();
    assert_eq!(out.lowered.lowerings[0].code, ReasonCode::ViewNotMeasured);
    assert!(out.lowered.lowerings[0].detail.contains("is a view in the catalog, not a table"));
}
```

`fold` in the tests is `|s: &str| s.to_lowercase()`.

- [ ] **Step 2: Run them. Expected: FAIL.**

```bash
cargo test -p egret-perm-ir lower::withhold
```

- [ ] **Step 3: Implement** by moving code, keeping statement order identical to today's `out.policies` push order:

- Trino gates come in `tag_id` order.
- StarRocks, per gate in order: per target either the whole-table deny then the per-column masks pending, or the per-column denies; then the MV deny last (`clearance.rs:393-398`).

The skip texts move verbatim. Ranger rendering of `Withhold`:

- on `Classification`, `tag_access_to_ranger_json` with the deny on `public` and `denyExceptions` = except;
- on `Path`, `starrocks_deny`;
- for `MaterializedViewGuard`, the 2-level `default_catalog`/`*` map from `mv_deny_inputs`.

`withheld_accesses` is replaced by the M11/M12 matrix filter in the IR: privileges = `[Read, Write]` filtered by `support(WithholdPrivilege(p)) == Native`. It is deleted from `clearance.rs` once `hidden_column_denies` (which also calls it) gets a local `const HIDDEN_ACCESSES: [&str; 1] = ["select"]`, pinned by a test against `engine_accesses(Read, StarRocks)`.

The `role_usernames(…, Mint)` read and mint move to `load_withhold_inputs`. They run **before** any other loader mint, as today: clearance mints after the model expansion (`members.rs` first, then `clearance.rs:268`). Keep that order: `load_ep` then `load_withhold_inputs`.

- [ ] **Step 4: Shadow and delete** with `EGRET_SHADOW_LEGACY=withhold`. Expected: no assertion.

- [ ] **Step 5: Run the gates. Expected: PASS.**

```bash
cargo test -p egret-perm-ir
DATABASE_URL=… cargo test -p egret-perm-compile --test clearance --test clearance_starrocks --test clearance_triggers --test people
DATABASE_URL=… cargo test -p egret-bi
just test-golden && just test-ranger-gates
cargo test -p egret-perm-compile --test profile_lock_sites && cargo test -p egret-perm-ir --test boundary
```

- [ ] **Step 6: fmt, clippy, commit.**

```bash
git commit -am "refactor(perm): clearance lowers to the IR's withhold effect

Shadow comparisons (legacy vs IR): <N>.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: The withhold view guard: version decision in the matrix, fold in the target

**Files:**
- Modify:
  - `crates/egret-perm-compile/src/clearance_mask.rs:405-460`: `FoldVersions` and `fold_plugin_versions` are deleted.
  - `crates/egret-perm-compile/src/trigger.rs:616-629`, `945-1025`: `fold_clearance_masks` takes the IR's decision.
  - `crates/egret-perm-ir/src/lower/withhold.rs`: adds `view_guard_versions`.

**Interfaces:**
- Produces:
  - `pub fn egret_perm_ir::lower::withhold::view_guard_versions(ep: &EpInput) -> Result<Vec<String>, String /*today's Unmodelled text*/>`
  - `pub(crate) async fn trigger::fold_clearance_masks(conn, l: &Loaded, versions: Vec<egret_ranger_eval::eval::PluginVersion>, pending: Vec<PendingGuard>, desired: &mut …, skipped: &mut …) -> Result<(), CompileError>`

- [ ] **Step 1: Write the failing tests** in `lower/withhold.rs`:

```rust
#[test]
fn the_view_guard_folds_at_each_distinct_bound_version() {
    let ep = starrocks_ep_with(vec![bound("sr-a", Ok("2.8.0")), bound("sr-b", Ok("2.9.0")), bound("sr-c", Ok("2.8.0"))]);
    assert_eq!(view_guard_versions(&ep).unwrap(), vec!["2.8.0".to_string(), "2.9.0".to_string()]);
}

#[test]
fn an_unmodelled_engine_names_itself_m13() {
    let ep = starrocks_ep_with(vec![bound("sr-x", Err("image foo:1 is not a known image".into()))]);
    let why = view_guard_versions(&ep).unwrap_err();
    assert!(why.starts_with("view protection (clearance mask) not active for sr: engine sr-x runs an image egret has not modelled"));
}

#[test]
fn a_known_but_unmeasured_version_is_not_measured() {
    let ep = starrocks_ep_with(vec![bound("sr-y", Ok("2.10.0"))]);
    assert!(view_guard_versions(&ep).unwrap_err().contains("sr-y"));
}

#[test]
fn with_no_engine_bound_the_default_image_decides() {
    let mut ep = starrocks_ep_with(vec![]);
    ep.default_version = Ok("2.8.0".into());
    assert_eq!(view_guard_versions(&ep).unwrap(), vec!["2.8.0".to_string()]);
}
```

**Text trap.** Today an engine whose plugin version **parses** to a `PluginVersion` (`BoundEngine::plugin_version`, `routes.rs:3465-3473`) is accepted, and `PluginVersion` has 2.5.0, 2.8.0 and 2.9.0 variants (`egret-ranger-eval/src/eval.rs:705-709`). The matrix's M13 says `Exactly(["2.8.0","2.9.0"])`. `engine_plugin_version` already refuses 2.5.0 for StarRocks: the dropped image is Trino-only and StarRocks never shipped 2.5.0. So the sets agree.

The `a_known_but_unmeasured_version_is_not_measured` test needs an `Ok("2.10.0")`, which `engine_plugin_version` can never produce today. The `Err` string for that case is new text and unreachable in production. That is acceptable only because it is unreachable; state that in the code comment, and keep the golden (G17's unknown image takes the `Err` path whose text is unchanged).

- [ ] **Step 2: Run them. Expected: FAIL.**

```bash
cargo test -p egret-perm-ir lower::withhold::tests::the_view_guard
```

- [ ] **Step 3: Implement.** `view_guard_versions` reproduces `fold_plugin_versions` (`clearance_mask.rs:425-460`):

1. go over `ep.engines` in name order (the loader orders them `ORDER BY name`, as today);
2. the first `Err(why)` returns the formatted `unmodelled(&e.name, why)` text;
3. collect distinct versions in first-appearance order;
4. with none bound, use `ep.default_version` (an error gives the `"(none bound; egret's default image)"` text);
5. finally, check every version against `matrix::support_for_kind(kind, Some(v), _, &Capability::WithholdThroughView) == Lowered(ViewGuardMask)`; otherwise return the not-measured text.

`fold_clearance_masks` converts `Vec<String>` to `PluginVersion` (`"2.8.0"` → `Ranger2_8_0`, …) in the target. That conversion is shape: the evaluator's enum. On `Err(reason)`, it emits one skip per merged column exactly as `FoldVersions::Unmodelled` did (`trigger.rs:994-999`). The fold itself (`fold_at_versions`, `principal_gap`, `fold_clearance_mask`, `merge_by_column`) stays in `clearance_mask.rs`, in the Ranger target, because it composes Ranger masks (including hand-written ones) through the Ranger evaluator.

- [ ] **Step 4: Shadow and delete** with `EGRET_SHADOW_LEGACY=view_guard`. Expected: no assertion. Delete `FoldVersions`, `fold_plugin_versions` and the legacy branch.

- [ ] **Step 5: Run the gates. Expected: PASS.**

```bash
cargo test -p egret-perm-ir && cargo test -p egret-perm-compile --lib clearance_mask
DATABASE_URL=… cargo test -p egret-perm-compile --test clearance_starrocks --test explain_engine_mismatch
just test-golden && just test-ranger-gates
cargo test -p egret-perm-compile --test profile_lock_sites
```

- [ ] **Step 6: fmt, clippy, commit.**

```bash
git commit -am "refactor(perm): the view guard's version check is a capability matrix entry

Shadow comparisons (legacy vs IR): <N>.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Engine privileges through the IR

**Files:**
- Create: `crates/egret-perm-ir/src/lower/engine_privileges.rs`, `crates/egret-perm-compile/src/ranger/engine_privileges.rs`.
- Modify:
  - `crates/egret-perm-compile/src/engine_grants.rs:55-150`: `compile_engine_grant` splits. `check_engine_grant` (the capability, engine, include-only, value count/kind and reserved-target checks) stays here beside the catalogue as the loader-side verdict. The resource and item building moves to `ranger/engine_privileges.rs`.
  - `crates/egret-perm-compile/src/trigger.rs:557-567`, `834-903`: `compile_engine_grants` is deleted.
  - `crates/egret-perm-compile/src/ir_load.rs`: adds `load_engine_privilege_checks`.

**Interfaces:**
- Produces:
  - `pub enum egret_perm_ir::lower::engine_privileges::Check { Ok { capability: String, values: Vec<String> }, Skip(String) }`
  - `pub fn lower_engine_privileges(grants: &[EngineGrant], checks: &BTreeMap<Uuid, Check>) -> Lowered`
  - `pub(crate) async fn ir_load::load_engine_privilege_checks(conn, service_id, engine, grants: &[EngineGrant]) -> Result<BTreeMap<Uuid, Check>, CompileError>`: runs `impersonation_target_ok` and then `check_engine_grant`. The `InvalidEngineGrant` / `UnexpandedPrincipalKind` `Display` text becomes `Skip(e.to_string())`, as `trigger.rs:898-899` does.
  - `pub(crate) fn ranger::engine_privileges::render(s: &Statement, profile: &str, engine: EngineType) -> Result<AccessPolicyInput, CompileError>`
  - `pub fn engine_grants::compile_engine_grant(...)` keeps its signature (used by `routes/engine_grants.rs` previews) as `check` + `lower` + `render`.

- [ ] **Step 1: Write the failing tests** in `lower/engine_privileges.rs`:

```rust
#[test]
fn an_empty_include_is_skipped_with_todays_text() {
    let g = eg(vec![], None);
    let out = lower_engine_privileges(&[g.clone()], &[(g.id, Check::Ok { capability: "trino.session_property".into(), values: vec!["x".into()] })].into());
    assert!(out.statements.is_empty());
    assert!(out.lowerings[0].detail.starts_with("the engine grant's principals name no engine principal"));
}

#[test]
fn a_checked_grant_is_one_engine_privilege_statement() {
    let g = eg(vec![user("ann")], None);
    let out = lower_engine_privileges(&[g.clone()], &[(g.id, Check::Ok { capability: "trino.session_property".into(), values: vec!["query_max_memory".into()] })].into());
    assert_eq!(out.statements[0].effect, Effect::EnginePrivilege { capability: "trino.session_property".into() });
    assert_eq!(out.statements[0].target, Target::EngineObject { capability: "trino.session_property".into(), values: vec!["query_max_memory".into()] });
}

#[test]
fn a_refused_check_is_a_skip() {
    let g = eg(vec![user("ann")], Some(uuid::Uuid::from_u128(3)));
    let out = lower_engine_privileges(&[g.clone()], &[(g.id, Check::Skip("impersonation target gone".into()))].into());
    assert_eq!(out.lowerings[0].code, ReasonCode::InvalidEnginePrivilege);
}
```

Use the capability ids from `crates/egret-perm-compile/src/engine_capabilities.rs`, and read the real ids before writing the test. The ones above are placeholders for the shape and must be replaced by a real id from the catalogue.

**Order trap.** Today the empty-include check runs **before** the impersonation check (`trigger.rs:858-880`). The loader must therefore skip the check for an empty include, and `lower_engine_privileges` must test the include first. Otherwise a grant that is both empty and has a dead target changes its reason.

- [ ] **Step 2: Run them. Expected: FAIL.**

```bash
cargo test -p egret-perm-ir lower::engine_privileges
```

- [ ] **Step 3: Implement** by moving `trigger.rs:844-902` and `engine_grants.rs:57-150` as described. The rendered resources come from the catalogue (`cap.resources`, `cap.access`), looked up by `capability` id in the target.

- [ ] **Step 4: Shadow and delete** with `EGRET_SHADOW_LEGACY=engine_privileges`. Expected: no assertion.

- [ ] **Step 5: Run the gates. Expected: PASS.**

```bash
DATABASE_URL=… cargo test -p egret-perm-compile --test engine_grants_compile --test engine_grants_routes --test engine_grants_explain --test engine_grants_references --test engine_grant_prefix_rename --test engine_grants_editor
just test-golden && cargo test -p egret-perm-compile --test profile_lock_sites && cargo test -p egret-perm-ir --test boundary
```

- [ ] **Step 6: fmt, clippy, commit.**

```bash
git commit -am "refactor(perm): engine privileges lower through the IR

Shadow comparisons (legacy vs IR): <N>.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Served-unit completion passes, the pipeline driver, and removing the bridge

**Files:**
- Create: `crates/egret-perm-compile/src/ranger/completion.rs`.
- Modify:
  - `crates/egret-perm-compile/src/trigger.rs:631-742`: the hidden-column and metadata-table blocks move into `ranger::completion::complete`, gated by M16/M17.
  - `crates/egret-perm-compile/src/clearance.rs`: `hidden_column_denies` and its helpers (`:654-970`) move to `ranger/completion.rs`. `clearance.rs` is deleted if empty.
  - `crates/egret-perm-compile/src/metadata_tables.rs`: `git mv` to `ranger/completion/metadata_tables.rs`. `with_denies_via` (used by explain, `routes.rs:3637`) moves with it.
  - `crates/egret-perm-ir/src/audience.rs`: delete `Audience::to_principal_set` and the `touched` flag.
  - `crates/egret-perm-compile/src/members.rs`: `expand_enumerated_principals` stops rewriting the model and returns only the `Directory`. Each family now builds its audience with `audience(&set, &dir)`: grants `lower_grants(ep, grants, dir)`, rules `lower_rules(…, dir, …)`, engine privileges `lower_engine_privileges(grants, dir, checks)`. These signature changes land in this task.
  - `crates/egret-perm-ir/src/lower/{grants,rules,engine_privileges}.rs`: take `&Directory`.

**Interfaces:**
- Produces:
  - `pub(crate) async fn ranger::completion::complete(conn: &mut PgConnection, l: &Loaded, desired: &mut BTreeMap<PolicyKey, DesiredPolicy>, tag_service_id: Option<i64>) -> Result<(), CompileError>`
  - The final `compile_desired` body:

```rust
pub(crate) async fn compile_desired(conn: &mut sqlx::PgConnection, service_id: i64) -> Result<Option<Compiled>, CompileError> {
    let Some(l) = crate::ir_load::load_ep(conn, service_id).await? else { return Ok(None) };
    let mut desired = BTreeMap::new();
    let mut skipped = Vec::new();
    crate::ranger::emit(&l, egret_perm_ir::lower::grants::lower_grants(&l.ep, &l.grants, &l.directory)?, &mut desired, &mut skipped)?;
    let checks = crate::ir_load::load_engine_privilege_checks(conn, service_id, l.ep.kind.engine, &l.engine_grants).await?;
    crate::ranger::emit(&l, egret_perm_ir::lower::engine_privileges::lower_engine_privileges(&l.engine_grants, &l.directory, &checks), &mut desired, &mut skipped)?;
    let (inputs, read_skips) = crate::ir_load::load_rule_inputs(conn, &l.ep, &l.rules).await?;
    crate::ranger::emit(&l, egret_perm_ir::lower::rules::lower_rules(&l.ep, &l.rules, &l.directory, &inputs, read_skips, DEFAULT_EXPANSION_CEILING, &l.leaving)?, &mut desired, &mut skipped)?;
    let gates = crate::ir_load::load_withhold_inputs(conn, &l.ep, service_id).await?;
    let w = egret_perm_ir::lower::withhold::lower_withholds(&l.ep, &gates, egret_ranger_eval::resource::fold_identifier_owned, DEFAULT_EXPANSION_CEILING, &l.leaving)?;
    crate::ranger::emit(&l, w.lowered, &mut desired, &mut skipped)?;
    if !w.view_guard.is_empty() {
        fold_clearance_masks(conn, &l, egret_perm_ir::lower::withhold::view_guard_versions(&l.ep), w.view_guard, &mut desired, &mut skipped).await?;
    }
    let tag_service_id = l.sweep.iter().copied().find(|&id| id != service_id);
    crate::ranger::completion::complete(conn, &l, &mut desired, tag_service_id).await?;
    refuse_overlaps(conn, &l, &desired, tag_service_id).await?; // trigger.rs:744-797, moved into a fn unchanged
    Ok(Some(Compiled { /* as today: desired, skipped, sweep, profile_name, largest: w.whole_tables, uncounted */ }))
}
```

`fold_identifier_owned` is a `fn(&str) -> String` adapter. If `egret_ranger_eval::resource::fold_identifier` already has that signature, pass it directly and do not add the adapter.

- [ ] **Step 1: Write the failing completion-gate test** in `ranger/completion.rs`:

```rust
#[test]
fn completion_passes_are_chosen_by_the_matrix() {
    use egret_perm_ir::matrix::{support_for_kind, Capability, LoweringKind, Support};
    use egret_perm_ir::ep::{EpKind, Mechanism};
    let k = |engine| EpKind { engine, mechanism: Mechanism::Ranger };
    assert_eq!(passes(k(egret_perm::EngineType::Trino)), vec![Pass::MetadataTables]);
    assert_eq!(passes(k(egret_perm::EngineType::StarRocks)), vec![Pass::HiddenColumns]);
    assert_eq!(
        support_for_kind(k(egret_perm::EngineType::StarRocks), None, false, &Capability::MaskCoversHiddenColumns),
        Support::Lowered(LoweringKind::DenyHiddenColumns)
    );
}
```

`passes(kind)` returns the passes whose capability answers `Lowered(DenyHiddenColumns | DenyMetadataTables)`.

Also add, in `crates/egret-perm-ir/src/audience.rs` tests, a compile-time guard: a doc-test that `Audience::to_principal_set` no longer exists:

```rust
/// ```compile_fail
/// let _ = egret_perm_ir::audience::Audience::default().to_principal_set(&Default::default());
/// ```
pub struct BridgeRemoved;
```

- [ ] **Step 2: Run them. Expected: FAIL.**

```bash
cargo test -p egret-perm-compile --lib ranger::completion && cargo test -p egret-perm-ir --doc
```

- [ ] **Step 3: Implement** by moving `trigger.rs:631-742` into `complete`:

- the StarRocks block runs when `passes` contains `HiddenColumns`;
- the Trino block runs when it contains `MetadataTables`;
- the bodies are unchanged and in the same order.

Move `trigger.rs:744-797` into `fn refuse_overlaps` unchanged. Remove the bridge and switch the lowerings to `&Directory` as listed under Files.

- [ ] **Step 4: Shadow and delete.**
  1. Keep a `legacy_compile_desired` holding the whole Task 1 pipeline. Rebuild it from `git show <task-1-commit>:crates/egret-perm-compile/src/trigger.rs` plus the then-current modules: the full old path, used only during this step.
  2. Run with `EGRET_SHADOW_LEGACY=all`. Expected: no assertion over the whole workspace.
  3. Delete it.

If reconstructing the old path in full is impractical because its modules are already gone, run the shadow with `EGRET_SHADOW_LEGACY=completion` (old completion blocks against the new `complete`) and rely on the golden for the rest. Record which one ran in the commit body.

- [ ] **Step 5: Run the gates. Expected: PASS.**

```bash
cargo test -p egret-perm-ir && cargo test -p egret-perm-compile --lib
DATABASE_URL=… cargo test -p egret-perm-compile --features harness
DATABASE_URL=… cargo test --workspace --no-fail-fast
just test-golden && just test-ranger-gates
cargo test -p egret-perm-compile --test profile_lock_sites && cargo test -p egret-perm-ir --test boundary
EGRET_TEST_SHADOW_LOG=/tmp/shadow10.log cargo test --workspace --features egret-perm-compile/shadow-compile --no-fail-fast -- --test-threads=1
```

The last line is the determinism shadow, now back to same-path mode.

- [ ] **Step 6: Check that no dead code remains.**

```bash
grep -rn "legacy_\|EGRET_SHADOW_LEGACY\|to_principal_set\|FoldVersions\|compile_one_rule\|compile_engine_grants\b" crates/ && echo "LEFTOVER" || echo clean
```

Expected: `clean`.

- [ ] **Step 7: fmt, clippy, commit.**

```bash
git commit -am "refactor(perm): compile is load, lower, render, complete; the old driver is gone

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Performance, locks and documentation check

**Files:**
- Modify:
  - `crates/egret-perm-compile/src/lib.rs`: update the module doc (`:1-41`) to describe load → lower → render → complete and to point at `egret-perm-ir`.
  - `crates/egret-perm-compile/src/trigger.rs`: module doc paragraphs that name moved functions.
  - `crates/egret-perm-ir/src/lib.rs`: crate doc with the D4 boundary.

**Interfaces:** none new.

- [ ] **Step 1: Measure.**

```bash
cargo test -p egret-perm-compile --release --features harness --test golden_compile -- --ignored --nocapture g21_compile_timing
```

Expected: median ≤ 1.10 × the Task 1 baseline. If it is slower, profile the statement building (`cargo flamegraph` on the ignored test) and fix it before going on. Do not raise the bound.

- [ ] **Step 2: Check the lock sites and the read set.**

```bash
cargo test -p egret-perm-compile --test profile_lock_sites
git diff <task-1-commit> -- crates/egret-perm-compile/tests/profile_lock_sites.rs | wc -l
```

Expected: PASS and `0` (the table is untouched).

- [ ] **Step 3: Check that names and API are unchanged.**

```bash
git diff --exit-code <task-1-commit> -- openapi.json frontend/src crates/egret-perm-compile/tests/golden migrations
```

Expected: no output.

- [ ] **Step 4: Run the full pre-merge gate.**

```bash
just check
```

Expected: green, with `RAN database-gated suites`, the two Ranger gates, and `test-golden` listed.

- [ ] **Step 5: Commit.**

```bash
git commit -am "docs(perm): describe the IR pipeline where the compiler lives

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: Demo stack: byte-identical in production

**Files:** none in the repository. The evidence goes into the PR description.

**Interfaces:** consumes `scripts/golden-demo-capture.sh` (Task 1).

- [ ] **Step 1: Capture before the deploy.** On the shared demo stack (port 18080), with the operator token:

```bash
EGRET_URL=http://localhost:18080 EGRET_TOKEN=… scripts/golden-demo-capture.sh /tmp/demo-before
```

Expected: the printed sum is `0` (a converged stack). If it is not `0`, run it again; the second run must print `0`. A stack that does not converge is a finding to report, not to paper over.

- [ ] **Step 2: Build and deploy the branch image** (the egret deploy-to-kind recipe). Before trusting the deploy, confirm that the image carries the new binary:

```bash
docker run --rm --entrypoint sh <image> -c 'strings /usr/local/bin/egret | grep -c egret_perm_ir'
```

Expected: a count of at least 1. The build cache can ship a stale binary.

- [ ] **Step 3: Capture after the deploy and compare.**

```bash
EGRET_URL=http://localhost:18080 EGRET_TOKEN=… scripts/golden-demo-capture.sh /tmp/demo-after
diff /tmp/demo-before/SHA256SUMS /tmp/demo-after/SHA256SUMS && echo BYTE-IDENTICAL
```

Expected: the printed sum is `0` (the recompile on the new code wrote nothing), followed by `BYTE-IDENTICAL`. Also confirm that each profile's `policy_version` is unchanged:

```bash
psql "$DEMO_DB" -c 'SELECT name, policy_version FROM policy.authz_services ORDER BY name'
```

Compare with the same query taken before the deploy.

- [ ] **Step 4: Run a live check.** As an unprivileged user on the stack, run one masked query and one row-filtered query on Trino and on StarRocks. The results must equal the results from before the deploy (for a behaviour-neutral PR the live check is a sanity check; the SHA comparison is the proof). Record both in the PR.

---

## Self-review

**Spec coverage (sub-project 1 acceptance → task):**

| Acceptance item | Where |
|---|---|
| Neutral IR exists (`egret-perm-ir`) and Store → IR makes the decisions | Tasks 2, 3, 5–10 (D4 split) |
| Capability matrix | Task 2 (M1–M19), used in Tasks 6–10 |
| `NotEnforced` generated from the matrix | Task 2 (`rule_enforcement`, `withhold_engine_reason`) |
| Ranger JSON byte-identical | Task 1 (golden + shadow + recipe), checked in every task, Task 11 (corpus untouched), Task 12 (stack SHA-256) |
| In production | Task 12 |
| Spec Verification §1 (IR unit tests: inheritance, group expansion, deny-wins, capability lowering) | Tasks 3, 5, 6, 7, 8, 9 |

Two items are deliberately absent:

- Inheritance and deny-wins *semantics* are not computed by the IR in SP1; they stay Ranger's. The IR carries deny statements as statements, and the evaluator over the IR is explain-on-IR, deferred to SP2 (D7).
- Spec Verification §3 (differential): `just test-ranger-gates` runs in Tasks 5, 6, 7 and 8.

**Placeholder scan:**

- Two places ask the implementer to read before writing: the capability ids in Task 9, Step 1, and the mask-type lists in Task 2, Step 3. Both have a pinning test that fails if the copied values are wrong, so a wrong guess cannot pass.
- The commented matrix rows in the D3/Task 2 code are listed for review and must be written out as `Entry` literals (stated at the site).
- The `<N>`, `<t>` and `<task-1-commit>` markers are measured values filled at execution.

**Type consistency:**

- `lower_rules` has six arguments in Task 6 (with `read_skips`) and seven in Task 10 (plus `&Directory`), as both tasks state.
- `lower_grants` and `lower_engine_privileges` gain `&Directory` in Task 10, as listed in that task's Files.
- `EpInput`, `Audience`, `Statement`, `Lowering`, `Support` and `LoweringKind` are used with the D2/Task 2 definitions throughout.
- `PendingGuard` (Task 7) replaces `clearance_mask::PendingMask` at the fold call. `merge_by_column` takes `PendingGuard` after Task 8, and that conversion is in Task 8's scope.

## Open questions for the owner (recommended default first)

- **Q1.** Every matrix entry except the view guard is `Versions::Any`, because today's compiler is version-blind. **Default:** keep `Any` in SP1 (byte-identical), and tighten to measured versions in SP3, when unknown ⇒ "not measured" starts to change compiled output.
- **Q2.** A bound profile of an unmodelled type counts as enforcing in `notEnforced` today. **Default:** keep it in SP1 with a pinning test, and fix it in SP3 (it is unreachable after `refuse_engine_of_another_type` / `check_profile_bindable`).
- **Q3.** The hidden-column and metadata-table passes parse Ranger bodies, hand-written included. **Default:** keep them as Ranger-target completion passes gated by the matrix, and move them into the IR in SP3 once hand-written policies are gone (D8).
- **Q4.** Explain on the IR. **Default:** out of SP1, into SP2.
- **Q5.** Infra policies. **Default:** stay stored until SP3.
- **Q6.** "In production" means the shared demo stack (the only deployment). **Default:** Task 12's SHA-256 comparison on that stack.
