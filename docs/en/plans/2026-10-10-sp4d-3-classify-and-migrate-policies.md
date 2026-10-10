# SP4d-3 Implementation Plan — classify and migrate hand-written Ranger policies

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put egret's own Ranger policies out of hand's reach, and give an operator a surface that says, per policy, what it is and whether it can become a Grant or a Rule — then migrate the ones that can, proving equivalence against a real Ranger engine before the write lands.

**Architecture:** A `managed_by` column on `policy.authz_policies` makes ownership a property of the row rather than of its name, so renaming cannot defeat the guard. A pure closed grammar turns a Ranger policy body into either candidate model objects or a list of named blockers. A pure equivalence gate decides the whole profile before and after, per plugin version, using SP4d-1's evaluator. The migration transaction writes the model objects, recompiles, switches the original off, records provenance, re-runs the gate against what it actually wrote, and rolls back on any disagreement.

**Tech Stack:** Rust (axum, sqlx, serde_json), PostgreSQL, `egret-ranger-eval` (pure evaluator), the Java differential harness under `tests/ranger/` (Maven + real `ranger-plugins-common` jars), React + react-query + Vite frontend.

**Spec:** `docs/en/specs/2026-10-10-classify-and-migrate-policies-sp4d-3-design.md`

---

## Global Constraints

- **egret IS the Ranger Admin.** No Apache Ranger server exists; the plugin inside each engine downloads policy straight from egret, so a row in `policy.authz_policies` *is* the decision the engine enforces. Trino 470 runs `ranger-plugins-common-2.5.0`; StarRocks fe-ubuntu 4.1 runs `2.8.0`, and **the two disagree on at least one artifact** — any claim about plugin behaviour must name a version.
- **The action expansion is fixed and is the hinge of the grammar** (`crates/egret-perm/src/actions.rs::engine_accesses`): `Action::Read` → Trino `["select"]`, StarRocks `["select"]`; `Action::Write` → Trino `["insert", "delete"]`, StarRocks `["insert", "update", "delete"]`.
- **All-or-nothing per policy.** A policy migrates whole or not at all. Partial migration would drop that policy's deny items, because the original is switched off.
- **A migrated policy is switched off, never deleted** — `policy.authz_policies.enabled = false`, the **column**. The download query is `WHERE service_id = $1 AND enabled = true`, so the artifact then omits the policy entirely.
- **Equivalence is evaluated whole-profile, per `PluginVersion` of the engines bound to that profile, and re-run against the state actually written inside the transaction.** Any disagreement rolls back.
- **Probe sets must include coordinates and principals neither side mentions.** A probe set derived only from the two policies' own vocabulary inherits their blind spots.
- **Verify the next migration number against `origin/main` at the moment you create the file:** `git ls-tree --name-only origin/main migrations/ | sort | tail -3`. On the branch merged as #656, four migrations were written at numbers main had since used, `migrate()` aborted at the first collision, and **none** of the four CHECK constraints was ever created while the Rust refusals shipped green. The newest on disk today is `0179_perm_wildcard_levels_name_nothing_beneath.sql`.
- **Migration prose is append-only** for a migration that has been applied anywhere — sqlx checksums applied migrations, so editing a comment reddens `migrate()` on every database that ran it.
- `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`, passed **explicitly** on every command. **Never** the dev database `egret`. **Never** `EGRET_ALLOW_DB_SKIP=1` — `egret-test-db`'s own guard fails you for it, and DB-gated tests otherwise print `ok` having asserted nothing.
- **Do not run `cargo test --workspace`, `cargo clippy --workspace`, or `just check`.** Per-crate `-p` only; `cargo clippy -p <crate> --no-deps --all-targets`. `just check` builds a second configuration sharing no artefacts and is what filled this disk mid-programme. Report `df -h /` before and after anything that builds.
- Regenerate the API client with **`npm run gen:api`**, never bare `orval` — bare `orval` drops `encodeURIComponent` from path params.
- Never pass `-q`; empty output is not a pass; capture exit codes; use `--no-fail-fast`. **Never read a suite total through `tail`** — compiler output pushes earlier `test result:` lines out of the window; grep the whole log for `test result:` and sum with `awk`.
- Mutations: `assert count == 1` on the **code** expression (not a substring that also matches a doc comment), confirm the mutant **compiles** (`grep -c '^error\[' <log>` is 0), confirm the intended test fails by name, then restore by **plain write plus SHA-256 compare on both sides**. `cp -i` and `rm -i` are aliased live here and `cp -f` does not defeat them.
- Never `git stash` — the stack is shared with other worktrees and sessions.
- **Nothing in this repo may compare two Ranger bodies through `to_string()`**: `preserve_order` is on, so `serde_json::Map` is an `IndexMap` and a `jsonb` round-trip reorders keys.

### Baselines (measured 2026-10-10 on `8de1b167f`)

`-p egret-policy` **140/0 across 13** · `-p egret-perm` **34/0 across 5** · `-p egret-perm-compile` **258/0 across 15** · `-p egret-ranger-eval` **263/0 across 10** · `-p egret-bi` **177/0 across 44** · `-p egret-auth` **148/0 across 32** · `-p egret-workspace` **88/0 across 14** · `-p egret-server --lib` **7/0** (holds both drift gates) · `-p egret-metastore --lib` **2/0** · frontend vitest **5599/5600** (the one failure is a 5 s timeout in `CatalogCreate.test.tsx` under full parallel load; it passes 37/37 alone).

`just test-ranger-gates` green with both gates really running: `ranger_engine_enforces_egret_policies` ~11–17 s and `rust_evaluator_agrees_with_the_real_ranger_engine` ~80–156 s. **The vacuous form of either is `0.00 s`** — report elapsed time, not just a pass.

---

## File Structure

**New files**

| Path | Responsibility |
|---|---|
| `migrations/0180_policy_managed_by.sql` | the `managed_by` column, its `CHECK`, and the backfill |
| `crates/egret-policy/src/infra_names.rs` | the **only** place that spells egret's infrastructure policy names, plus `is_egret_infrastructure_policy_name` |
| `crates/egret-perm-compile/src/grammar.rs` | pure: a Ranger policy body → candidate model objects, or named blockers |
| `crates/egret-perm-compile/src/classify.rs` | pure: owner + category + verdict + blockers for one policy |
| `crates/egret-perm-compile/src/equivalence.rs` | pure: probe derivation and before/after comparison per plugin version |
| `crates/egret-perm-compile/src/migrate.rs` | the migration transaction and its rollback |
| `migrations/0181_perm_migrated_from.sql` | provenance |
| `frontend/src/routes/governance/PolicyMigrationPage.tsx` | the operator's page |
| `frontend/src/routes/governance/__tests__/PolicyMigrationPage.test.tsx` | its tests |

**Modified files**

| Path | Change |
|---|---|
| `crates/egret-policy/src/default_policies.rs` | import the name prefixes from `infra_names` instead of spelling them at call sites |
| `crates/egret-engine-mgmt/src/delegator_policy.rs` | re-export the two delegator names from `egret-policy` rather than declaring them |
| `crates/egret-engine-mgmt/src/engines.rs:2196,2200` | follow that move (the only out-of-file users, as `dp::…`) |
| `crates/egret-policy/src/store.rs` | stamp `managed_by`; refuse human `UPDATE`/`DELETE`/`set_policy_enabled` on an `infra` row |
| `crates/egret-perm-compile/src/routes.rs` | the three routes |
| `crates/egret-ranger-eval/tests/differential.rs` | the migration corpus |
| `frontend/src/api/policies.ts` | adapters for the new routes |
| `frontend/src/App.tsx` (`GovernanceRoutes`) | one `<Route>` entry |
| `frontend/src/components/layout/Sidebar.tsx` (`GOVERNANCE_GROUPS`) | one nav entry |

**Why `infra_names.rs` lives in `egret-policy` and not next to its users.** The predicate needs both the seeder's name prefixes (in `egret-policy`) and the two delegator names (in `egret-engine-mgmt`). `egret-engine-mgmt` depends on `egret-policy` (`Cargo.toml:84`) and `egret-perm-compile` depends on `egret-policy` (`Cargo.toml:25`), while `egret-policy` depends on neither. So `egret-policy` is the only crate all three guards can import from, and the DB guard lives there anyway. Putting it elsewhere would leave two copies of one list — the defect shape that appeared five times on the branch merged as #656.

---

## Task 1: `managed_by`, one source of truth for infra names, and a backfill that agrees with `attribution.rs`

**Files:**
- Create: `migrations/0180_policy_managed_by.sql`
- Create: `crates/egret-policy/src/infra_names.rs`
- Modify: `crates/egret-policy/src/lib.rs` (declare and re-export the module)
- Modify: `crates/egret-policy/src/default_policies.rs:133,167`
- Modify: `crates/egret-engine-mgmt/src/delegator_policy.rs:11-12`
- Modify: `crates/egret-engine-mgmt/src/engines.rs:2196,2200`
- Test: `crates/egret-policy/tests/infra_names.rs` (new), `crates/egret-perm-compile/tests/managed_by_agreement.rs` (new)

**Interfaces:**
- Produces:
  - `egret_policy::infra_names::ADMIN_ALL_ACCESS_NAME_PREFIX: &str` = `"egret-admin"`
  - `egret_policy::infra_names::DEFAULT_ALL_ACCESS_NAME_PREFIX: &str` = `"all"`
  - `egret_policy::infra_names::IMPERSONATE_POLICY_NAME: &str` = `"egret-delegator-impersonate"`
  - `egret_policy::infra_names::QUERY_TIMEOUT_POLICY_NAME: &str` = `"egret-query-timeout"`
  - `egret_policy::infra_names::is_egret_infrastructure_policy_name(name: &str) -> bool`
  - `policy.authz_policies.managed_by text NOT NULL DEFAULT 'operator'`
- Consumes: `egret_policy::is_compiler_owned_policy_name(name: &str) -> bool` (already exists, `store.rs`)

⚠ **A trap in `default_policies.rs`.** The file contains `"all"` **twice, meaning two different things**: at line 39, `.filter(|n| *n != "all")` excludes the access-type **alias** named `all` from the grant set; at line 133, `"all"` is the policy **name prefix** passed to `build_default_policies`. Only line 133 is a name. A sweep that replaces both breaks the access-type filter silently.

- [ ] **Step 1: Write the failing test for the name predicate**

Create `crates/egret-policy/tests/infra_names.rs`:

```rust
use egret_policy::infra_names::{
    is_egret_infrastructure_policy_name, ADMIN_ALL_ACCESS_NAME_PREFIX,
    DEFAULT_ALL_ACCESS_NAME_PREFIX, IMPERSONATE_POLICY_NAME, QUERY_TIMEOUT_POLICY_NAME,
};

/// The four shapes the seeder and the delegator actually mint, and the
/// near-misses that must NOT be claimed as infrastructure. The negatives are
/// the half that matters: a predicate that answers `true` for everything would
/// pass the positives alone and would then lock an operator out of their own
/// policies.
#[test]
fn the_four_infrastructure_shapes_are_recognised_and_nothing_else_is() {
    // Positives — exactly what `default_policies::build_default_policies` and
    // `delegator_policy` produce.
    assert!(is_egret_infrastructure_policy_name(
        "egret-admin - catalog, schema, table"
    ));
    assert!(is_egret_infrastructure_policy_name(
        "egret-admin - catalog, schema, table, column"
    ));
    assert!(is_egret_infrastructure_policy_name("all - catalog, schema, table"));
    assert!(is_egret_infrastructure_policy_name(IMPERSONATE_POLICY_NAME));
    assert!(is_egret_infrastructure_policy_name(QUERY_TIMEOUT_POLICY_NAME));

    // Negatives — an operator's own names, including ones that start with the
    // same words. The seeder's format is `"{prefix} - {chain}"`, so the
    // separator is load-bearing and a bare prefix match is too wide.
    assert!(!is_egret_infrastructure_policy_name("all-hands-reporting"));
    assert!(!is_egret_infrastructure_policy_name("allow sales"));
    assert!(!is_egret_infrastructure_policy_name("egret-admin-notes"));
    assert!(!is_egret_infrastructure_policy_name("egret-admins - catalog"));
    assert!(!is_egret_infrastructure_policy_name("egret-query-timeouts"));
    assert!(!is_egret_infrastructure_policy_name(""));
    assert!(!is_egret_infrastructure_policy_name("perm-grant-abc"));
}

/// The constants are the values the seeder passes today. If someone changes a
/// literal in `default_policies.rs` without changing the constant, this fails —
/// which is the only thing stopping the predicate and the producer from
/// drifting apart.
#[test]
fn the_constants_are_the_values_the_producers_use() {
    assert_eq!(ADMIN_ALL_ACCESS_NAME_PREFIX, "egret-admin");
    assert_eq!(DEFAULT_ALL_ACCESS_NAME_PREFIX, "all");
    assert_eq!(IMPERSONATE_POLICY_NAME, "egret-delegator-impersonate");
    assert_eq!(QUERY_TIMEOUT_POLICY_NAME, "egret-query-timeout");
}
```

- [ ] **Step 2: Run it and watch it fail for the right reason**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-policy --test infra_names --no-fail-fast
```
Expected: FAIL to **compile**, `unresolved import egret_policy::infra_names`. A compile failure here is the correct red — do not proceed if it fails for any other reason.

- [ ] **Step 3: Write the module**

Create `crates/egret-policy/src/infra_names.rs`:

```rust
//! The ONLY place egret's own infrastructure policy names are spelled.
//!
//! Four shapes exist, from two producers:
//!
//! 1. [`default_policies::build_default_policies`](crate::default_policies) mints
//!    `"{prefix} - {resource, chain, ...}"`, once per resource hierarchy, with
//!    the prefix [`ADMIN_ALL_ACCESS_NAME_PREFIX`] (the per-admin seeding) or
//!    [`DEFAULT_ALL_ACCESS_NAME_PREFIX`] (the group-granted default set).
//! 2. `egret_engine_mgmt::delegator_policy` mints exactly two fixed names,
//!    [`IMPERSONATE_POLICY_NAME`] and [`QUERY_TIMEOUT_POLICY_NAME`].
//!
//! **Why this list lives here and not beside either producer.** The predicate
//! has to cover both, and `egret-policy` is the only crate all three consumers
//! can import from: `egret-engine-mgmt` and `egret-perm-compile` both depend on
//! `egret-policy`, and `egret-policy` depends on neither. A copy of this list
//! anywhere else would be a second spelling of one fact, which is the failure
//! this programme reproduced five times on the branch merged as #656 — two
//! guards that must agree, written to different shapes.
//!
//! **This predicate is a BACKFILL and a CROSS-CHECK, not the enforcement.**
//! Enforcement is `policy.authz_policies.managed_by`, a column, because a
//! predicate over a name is defeated by a rename and a column is not. This
//! function exists to compute the column's initial value and to assert, in a
//! test, that the column and the names still agree.

/// Name prefix of the per-admin all-access policies
/// (`seed_admin_all_access` → `build_default_policies(..., "egret-admin", ...)`).
pub const ADMIN_ALL_ACCESS_NAME_PREFIX: &str = "egret-admin";

/// Name prefix of the group-granted default policy set
/// (`seed_default_trino_policies` → `build_default_policies(..., "all", ...)`).
///
/// ⚠ Do not confuse this with the access-type alias **also** spelled `all`,
/// which `default_policies::all_access_types` filters out of a grant set. That
/// one is an entry in the service-def's `accessTypes`; this one is a policy
/// name prefix. They are unrelated and a sweep that treats them as one string
/// breaks the access-type filter without failing a test.
pub const DEFAULT_ALL_ACCESS_NAME_PREFIX: &str = "all";

/// The delegator's impersonation policy. Moved here from
/// `egret_engine_mgmt::delegator_policy` so this list has one home; that module
/// re-exports it, so its own callers are unchanged.
pub const IMPERSONATE_POLICY_NAME: &str = "egret-delegator-impersonate";

/// The delegator's query-timeout policy. Same move, same reason.
pub const QUERY_TIMEOUT_POLICY_NAME: &str = "egret-query-timeout";

/// The separator `build_default_policies` puts between the prefix and the
/// resource chain: `format!("{} - {}", name_prefix, names.join(", "))`. Matching
/// on the prefix alone would claim an operator's `"all-hands-reporting"` as
/// infrastructure and lock them out of it.
const SEEDED_NAME_SEPARATOR: &str = " - ";

/// Whether `name` is a policy egret's own plumbing minted.
pub fn is_egret_infrastructure_policy_name(name: &str) -> bool {
    name == IMPERSONATE_POLICY_NAME
        || name == QUERY_TIMEOUT_POLICY_NAME
        || is_seeded_hierarchy_name(name, ADMIN_ALL_ACCESS_NAME_PREFIX)
        || is_seeded_hierarchy_name(name, DEFAULT_ALL_ACCESS_NAME_PREFIX)
}

fn is_seeded_hierarchy_name(name: &str, prefix: &str) -> bool {
    match name.strip_prefix(prefix) {
        Some(rest) => rest.starts_with(SEEDED_NAME_SEPARATOR),
        None => false,
    }
}
```

In `crates/egret-policy/src/lib.rs`, add the module declaration next to the existing ones:

```rust
pub mod infra_names;
```

- [ ] **Step 4: Run the test and watch it pass**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-policy --test infra_names --no-fail-fast
```
Expected: PASS, 2 tests.

- [ ] **Step 5: Point the producers at the constants**

In `crates/egret-policy/src/default_policies.rs`, add the import and replace **only the two name-prefix literals**:

```rust
use crate::infra_names::{ADMIN_ALL_ACCESS_NAME_PREFIX, DEFAULT_ALL_ACCESS_NAME_PREFIX};
```

Line 133 (inside `seed_default_trino_policies`): `"all"` becomes `DEFAULT_ALL_ACCESS_NAME_PREFIX`.
Line 167 (inside `seed_admin_all_access`): `"egret-admin"` becomes `ADMIN_ALL_ACCESS_NAME_PREFIX`.

**Leave line 39 (`.filter(|n| *n != "all")`) exactly as it is** — see the ⚠ above.

In `crates/egret-engine-mgmt/src/delegator_policy.rs`, replace the two declarations at lines 11-12 with a re-export, so `dp::IMPERSONATE_POLICY_NAME` at `engines.rs:2196` and `dp::QUERY_TIMEOUT_POLICY_NAME` at `:2200` keep compiling unchanged:

```rust
// Declared in `egret_policy::infra_names` so the guard, the backfill and this
// producer read one list. Re-exported rather than moved outright: `engines.rs`
// refers to them as `dp::IMPERSONATE_POLICY_NAME` / `dp::QUERY_TIMEOUT_POLICY_NAME`
// and those call sites are correct as they stand.
pub use egret_policy::infra_names::{IMPERSONATE_POLICY_NAME, QUERY_TIMEOUT_POLICY_NAME};
```

Keep the existing assertions at `delegator_policy.rs:93-94` — they now pin the re-export, which is exactly what you want a test to do after a move.

- [ ] **Step 6: Confirm the move broke nothing**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-policy --no-fail-fast > /tmp/t1-policy.log 2>&1; echo "EXIT=$?"
grep -c '^error\[' /tmp/t1-policy.log
grep 'test result:' /tmp/t1-policy.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-engine-mgmt --no-fail-fast > /tmp/t1-eng.log 2>&1; echo "EXIT=$?"
grep -c '^error\[' /tmp/t1-eng.log
grep 'test result:' /tmp/t1-eng.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
```
Expected: `egret-policy` 142/0 across 14 (140 baseline + your 2), `egret-engine-mgmt` unchanged from its own baseline, `error[` count 0 in both.

- [ ] **Step 7: Check the migration number, then write the migration**

```bash
git fetch -q origin && git ls-tree --name-only origin/main migrations/ | sort | tail -3
```
If that shows anything at or above `0180`, use the next free number and say so in your report. Assuming `0179` is still the newest, create `migrations/0180_policy_managed_by.sql`:

```sql
-- 0180_policy_managed_by.sql
--
-- SP4d-3 Task 1: make the OWNER of a Ranger policy a property of the row.
--
-- Until now the only ownership signal was the policy NAME: `refuse_reserved_name`
-- (`crates/egret-policy/src/store.rs`) reserves `perm-grant-` / `perm-rule-` /
-- `perm-clearance-` for the SP4a compiler, and nothing at all protected egret's
-- own infrastructure policies — the thirteen all-access hierarchies per
-- generated Trino profile (`egret-admin - …`, `all - …`) and the delegator's two
-- fixed names. An operator holding `policy:update` could therefore edit or
-- delete the policies that make delegation work, and no code refused it.
--
-- WHY A COLUMN AND NOT A FOURTH NAME PREFIX. A name predicate is a test over a
-- string the editor can change: rename the policy and you are outside the
-- guard. `managed_by` travels with the row, so a rename cannot defeat it. It is
-- also the shape this codebase already adopted once — put a cross-query safety
-- rule in a CHECK, not in a predicate somebody has to remember to call.
--
-- Policy NAMES are deliberately unchanged. `egret_perm_compile::attribution`
-- must keep reading names, because the evaluator only ever sees names, inside
-- the artifact an engine downloaded, with no access to this column.
--
-- THE BACKFILL. Computed in SQL from the same four shapes
-- `egret_policy::infra_names::is_egret_infrastructure_policy_name` recognises,
-- and from the same three prefixes `is_compiler_owned_policy_name` recognises.
-- Two spellings of one fact is the failure this programme reproduced five times
-- on the branch merged as #656, so the Rust side does not get to disagree
-- silently: `crates/egret-perm-compile/tests/managed_by_agreement.rs` asserts
-- the column and the predicates agree across the WHOLE table, not on a sample.
--
-- `DEFAULT 'operator'` is the right default for the human entry point: a policy
-- somebody creates through the editor is theirs. The three non-human writers
-- stamp their own value explicitly: the compiler writes 'compiler'; the two
-- seeders and the delegator seeding write 'infra' through
-- `upsert_infra_policy` / `create_infra_policy`, which refuse any name
-- `is_egret_infrastructure_policy_name` does not recognise (Task 1 Step 10).

ALTER TABLE policy.authz_policies
    ADD COLUMN managed_by text NOT NULL DEFAULT 'operator';

ALTER TABLE policy.authz_policies
    ADD CONSTRAINT authz_policies_managed_by_is_known
    CHECK (managed_by IN ('compiler', 'infra', 'operator'));

-- Compiler-owned: the three reserved prefixes, matched with LIKE and an escaped
-- `_`? No — none of the three prefixes contains `_` or `%`, so a plain LIKE is
-- exact here. (`LIKE 'egret_t_%'` once dropped a database in this repo because
-- `_` is a single-character wildcard; it does not bite on these three, and the
-- reason is written down so nobody "fixes" it by adding an ESCAPE clause that
-- changes the meaning.)
UPDATE policy.authz_policies
   SET managed_by = 'compiler'
 WHERE name LIKE 'perm-grant-%'
    OR name LIKE 'perm-rule-%'
    OR name LIKE 'perm-clearance-%';

-- Infrastructure: the delegator's two fixed names, plus the seeded hierarchies,
-- whose format is `format!("{prefix} - {chain}")`. The separator is matched
-- explicitly: a bare prefix match would claim an operator's `all-hands-reporting`
-- as egret's and lock them out of their own policy.
UPDATE policy.authz_policies
   SET managed_by = 'infra'
 WHERE name IN ('egret-delegator-impersonate', 'egret-query-timeout')
    OR name LIKE 'egret-admin - %'
    OR name LIKE 'all - %';
```

- [ ] **Step 8: Write the whole-table agreement test**

Create `crates/egret-perm-compile/tests/managed_by_agreement.rs`:

⚠ **Two things about this crate's test harness that the obvious code gets wrong.** There are
**zero** uses of `#[sqlx::test]` under `crates/egret-perm-compile/tests/` and nine uses of
`egret_test_db::url()`; the convention is `#[tokio::test]` plus `Db::connect`. And this
workspace's test database is **shared by every concurrently-running test in the binary**, so a
`SELECT … FROM policy.authz_policies` with no `WHERE service_id` reads other tests' rows and is
flaky by construction. Every assertion is scoped to the fixture's own `service_id`.

That means the literal "whole table" assertion the spec asks for **cannot be written here**, and
pretending otherwise would be the kind of test that passes for the wrong reason. What this test
proves is that the column and the predicates agree for **every row shape that exists**, which is
the property that actually matters; the whole-table check belongs against a real cluster and is
Task 10 Step 6.

```rust
//! `managed_by` (the column, which enforces) and the name predicates (which
//! backfilled it, and which `attribution` reads) must not drift apart.
//!
//! Scoped to this fixture's own `service_id`: the test database is shared with
//! every other concurrently-running test in this binary, so an unscoped
//! `SELECT` would read their rows. The fixture therefore carries one row of
//! EVERY shape — all three owners plus the near-misses — because shapes, not
//! row counts, are what the predicates can disagree about.

use egret_metastore::Db;
use egret_policy::infra_names::is_egret_infrastructure_policy_name;
use egret_policy::is_compiler_owned_policy_name;

#[tokio::test]
async fn the_column_and_the_name_predicates_agree_on_every_shape() {
    let Some(url) = egret_test_db::url().await else {
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();

    let service_id: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_services (name, type, config_jsonb) \
         VALUES ($1, 'trino', '{}'::jsonb) RETURNING id",
    )
    .bind(format!("t-agree-{}", uuid::Uuid::new_v4().simple()))
    .fetch_one(&pool)
    .await
    .expect("service");

    for (name, expected) in [
        ("perm-grant-11111111-1111-1111-1111-111111111111", "compiler"),
        ("perm-rule-22222222-2222-2222-2222-222222222222", "compiler"),
        ("perm-clearance-33333333-3333-3333-3333-333333333333", "compiler"),
        ("egret-admin - catalog, schema, table", "infra"),
        ("all - catalog, schema, table", "infra"),
        ("egret-delegator-impersonate", "infra"),
        ("egret-query-timeout", "infra"),
        ("all-hands-reporting", "operator"),
        ("egret-admin-notes", "operator"),
        ("sales can read orders", "operator"),
    ] {
        sqlx::query(
            "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
             VALUES ($1, $2, '{}'::jsonb, $3)",
        )
        .bind(service_id)
        .bind(name)
        .bind(expected)
        .execute(&pool)
        .await
        .expect("insert");
    }

    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, managed_by FROM policy.authz_policies WHERE service_id = $1 ORDER BY id",
    )
    .bind(service_id)
    .fetch_all(&pool)
    .await
    .expect("read back");

    // The control that makes a zero meaningful: if the fixture had not landed,
    // or the WHERE were wrong, this would be 0 and the loop below would assert
    // nothing at all while still reporting `ok`.
    assert_eq!(rows.len(), 10, "fixture did not land");

    let mut disagreements = Vec::new();
    for (name, managed_by) in &rows {
        let from_names = if is_compiler_owned_policy_name(name) {
            "compiler"
        } else if is_egret_infrastructure_policy_name(name) {
            "infra"
        } else {
            "operator"
        };
        if from_names != managed_by.as_str() {
            disagreements.push(format!("{name:?}: column={managed_by} names={from_names}"));
        }
    }
    assert!(
        disagreements.is_empty(),
        "the column and the name predicates disagree on {} row(s):\n  {}",
        disagreements.len(),
        disagreements.join("\n  ")
    );
}
```

- [ ] **Step 9: Run it and watch it pass**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-perm-compile --test managed_by_agreement --no-fail-fast
```
Expected: PASS, 1 test. If it fails on `authz_services` column names, read the real schema with
`psql "$DATABASE_URL" -c '\d policy.authz_services'` and fix the fixture — do not change the assertion.

- [ ] **Step 10: Make the three writers stamp their own owner**

In `crates/egret-policy/src/store.rs`, the compiler's own writes go through `egret-perm-compile`'s `trigger.rs` raw queries, and the seeder goes through `create_policy`/`upsert_policy`. The delegator's two fixed policies (`engines.rs::seed_delegator_policies`) are a THIRD writer: they go through `PolicyPort::create_policy`. Add an explicit `managed_by` to all three so a fresh row never relies on the default:

- `trigger.rs`'s `INSERT INTO policy.authz_policies (service_id, name, policy_jsonb)` becomes `(service_id, name, policy_jsonb, managed_by)` with `'compiler'` bound.
- `default_policies.rs`'s two seeding paths call a new `PolicyStore::upsert_infra_policy(service_id, name, body)` that writes `managed_by = 'infra'`, rather than `create_policy`/`upsert_policy` which leave the `'operator'` default.
- `seed_delegator_policies` calls a new `PolicyPort::create_infra_policy` (create semantics: `AlreadyExists` on a clash, behind its existing `policy_exists` skip) that writes `managed_by = 'infra'`. Both infra writers (`create_infra_policy`, `upsert_infra_policy`) REFUSE, with an error and not a `debug_assert`, any name for which `is_egret_infrastructure_policy_name` is false. `create_policy` is NOT changed to stamp by name.
- Each writer is covered by a DB test asserting the stamped value (`crates/egret-policy/tests/infra_writers.rs`, `crates/egret-perm-compile/tests/compiler_stamps_managed_by.rs`).

- [ ] **Step 11: Mutation-check the backfill's separator**

Drop `SEEDED_NAME_SEPARATOR` from `is_seeded_hierarchy_name` so it matches on the bare prefix:

```rust
fn is_seeded_hierarchy_name(name: &str, prefix: &str) -> bool {
    name.starts_with(prefix)
}
```

```bash
grep -c 'rest.starts_with(SEEDED_NAME_SEPARATOR)' crates/egret-policy/src/infra_names.rs   # must print 1
shasum -a 256 crates/egret-policy/src/infra_names.rs   # record this
# apply the mutation, then:
cargo build -p egret-policy --tests 2>&1 | grep -c '^error\['   # must print 0
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-policy --test infra_names --no-fail-fast
```
Expected: `the_four_infrastructure_shapes_are_recognised_and_nothing_else_is` FAILS on `"all-hands-reporting"`. Restore by writing the original text back (not `cp`), then `shasum -a 256` again and confirm it matches the recorded value.

- [ ] **Step 12: Commit**

```bash
git add migrations/0180_policy_managed_by.sql \
        crates/egret-policy/src/infra_names.rs \
        crates/egret-policy/src/lib.rs \
        crates/egret-policy/src/default_policies.rs \
        crates/egret-policy/src/store.rs \
        crates/egret-policy/tests/infra_names.rs \
        crates/egret-engine-mgmt/src/delegator_policy.rs \
        crates/egret-engine-mgmt/src/engines.rs \
        crates/egret-perm-compile/src/trigger.rs \
        crates/egret-perm-compile/tests/managed_by_agreement.rs
git commit -m "feat(policy): make policy ownership a column, not a name prefix"
```

---

## Task 2: refuse a human write to an `infra` row

**Files:**
- Modify: `crates/egret-policy/src/store.rs` (the three human write paths)
- Test: `crates/egret-policy/tests/infra_guard.rs` (new)

**Interfaces:**
- Consumes: `policy.authz_policies.managed_by` (Task 1)
- Produces: `PolicyStoreError::InfrastructureOwned(String)` — a new variant, 409-mapped like `CompilerOwned`

The three paths that must refuse, all in `store.rs`: `update_policy`, `delete_policy`, and `set_policy_enabled`. Each already calls `refuse_if_compiler_owned(&mut tx, service_id, &current_name)`; the new refusal belongs **beside** it and keyed on the column, not folded into it — `refuse_if_compiler_owned` answers a question about a NAME, and this one answers a question about a ROW.

`create_policy` needs no change for the DELEGATOR: Task 1 (fix round 1) moved the delegator's two policies onto `PolicyPort::create_infra_policy`, so they are stamped `'infra'` at insert and never pass through `create_policy`. What Task 2 must still do on the human path is RESERVE infra names: a human `create_policy` (and rename in `update_policy`) of a name `is_egret_infrastructure_policy_name` recognises must be refused, otherwise an operator can mint an `'operator'` row under an infra name that the seeders later re-stamp. That reservation is Task 2's, not Task 1's.

- [ ] **Step 1: Write the failing test**

Create `crates/egret-policy/tests/infra_guard.rs`:

⚠ Same harness note as Task 1: `egret-policy`'s own tests use `egret_test_db::url()` plus
`Db::connect` and `PolicyStore::new(db)` — see `crates/egret-policy/tests/default_policies.rs:8-13`
for the exact shape. There is no `PolicyStore::from_pool`; the constructor is
`PolicyStore::new(db: Db)` (`store.rs:556`). The database is shared with concurrent tests, so the
service name is made unique and every read is scoped to this fixture's `service_id`.

```rust
//! A human may not edit, disable or delete a policy egret's own plumbing owns.
//!
//! Both cells on every path. A guard measured only by its refusals cannot be
//! told apart from `return Err(..)`, and this programme has already shipped one
//! predicate whose allow arm was unfalsifiable.

use egret_metastore::Db;
use egret_policy::{PolicyStore, PolicyStoreError};
use serde_json::json;

async fn fixture() -> Option<(PolicyStore, sqlx::PgPool, i64, i64)> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let store = PolicyStore::new(db);
    let service_id: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_services (name, type, config_jsonb) \
         VALUES ($1, 'trino', '{}'::jsonb) RETURNING id",
    )
    .bind(format!("t-infra-{}", uuid::Uuid::new_v4().simple()))
    .fetch_one(&pool)
    .await
    .expect("service");
    let infra: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
         VALUES ($1, 'egret-delegator-impersonate', '{}'::jsonb, 'infra') RETURNING id",
    )
    .bind(service_id)
    .fetch_one(&pool)
    .await
    .expect("infra policy");
    let operator: i64 = sqlx::query_scalar(
        "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
         VALUES ($1, 'sales can read orders', '{}'::jsonb, 'operator') RETURNING id",
    )
    .bind(service_id)
    .fetch_one(&pool)
    .await
    .expect("operator policy");
    Some((store, pool, infra, operator))
}

#[tokio::test]
async fn an_infra_policy_cannot_be_updated_disabled_or_deleted_and_an_operator_one_can() {
    let Some((store, pool, infra, operator)) = fixture().await else {
        return;
    };
    let body = json!({"name": "x", "policyItems": []});

    // REFUSED — the three paths, on the infra row.
    assert!(matches!(
        store.update_policy(infra, "renamed", &body, true).await,
        Err(PolicyStoreError::InfrastructureOwned(_))
    ));
    assert!(matches!(
        store.set_policy_enabled(infra, false).await,
        Err(PolicyStoreError::InfrastructureOwned(_))
    ));
    assert!(matches!(
        store.delete_policy(infra).await,
        Err(PolicyStoreError::InfrastructureOwned(_))
    ));

    // ALLOWED — the same three paths, on the operator row. Without these the
    // test cannot tell a correct guard from one that refuses everything.
    store
        .update_policy(operator, "sales can read orders v2", &body, true)
        .await
        .expect("an operator policy is still editable");
    store
        .set_policy_enabled(operator, false)
        .await
        .expect("an operator policy is still disableable");
    store
        .delete_policy(operator)
        .await
        .expect("an operator policy is still deletable");

    // And the infra row is untouched by the three refusals — a guard that
    // refused AFTER writing would pass every assertion above.
    let (name, enabled): (String, bool) =
        sqlx::query_as("SELECT name, enabled FROM policy.authz_policies WHERE id = $1")
            .bind(infra)
            .fetch_one(&pool)
            .await
            .expect("the infra row still exists");
    assert_eq!(name, "egret-delegator-impersonate");
    assert!(enabled);
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-policy --test infra_guard --no-fail-fast
```
Expected: FAIL to compile — `no variant InfrastructureOwned`. Check the real signatures of `update_policy` / `delete_policy` with `grep -n 'pub async fn update_policy\|pub async fn delete_policy' -A 6 crates/egret-policy/src/store.rs` and adjust the **call** in the test if the arity differs; do not weaken the assertions.

- [ ] **Step 3: Add the variant and the guard**

In `crates/egret-policy/src/store.rs`, add to `PolicyStoreError`:

```rust
    /// SP4d-3: a human write aimed at a row egret's own plumbing owns
    /// (`managed_by = 'infra'`). Mapped to 409, like [`Self::CompilerOwned`] —
    /// the request is well-formed and names a real policy; what is wrong is
    /// who owns it.
    #[error("{0}")]
    InfrastructureOwned(String),
```

and the guard, beside `refuse_if_compiler_owned` rather than inside it:

```rust
/// SP4d-3 Task 2: refuse a human `UPDATE`/`DELETE`/enable-flip of a policy
/// egret's own plumbing owns.
///
/// Keyed on `managed_by`, the COLUMN — deliberately not on the name.
/// [`refuse_if_compiler_owned`] beside this one answers a question about a
/// NAME, which is the right question for the compiler's reserved prefixes
/// because the evaluator and `attribution` both read names. This one answers a
/// question about a ROW, because the thing being protected is the row's
/// existence and content, and a rename must not move it out of the guard.
///
/// Connection-generic so every caller runs it on its own open transaction, the
/// same shape the guards around it already have.
async fn refuse_if_infrastructure_owned(
    conn: &mut sqlx::PgConnection,
    policy_id: i64,
) -> Result<(), PolicyStoreError> {
    let managed_by: Option<String> =
        sqlx::query_scalar("SELECT managed_by FROM policy.authz_policies WHERE id = $1")
            .bind(policy_id)
            .fetch_optional(&mut *conn)
            .await?;
    if managed_by.as_deref() == Some("infra") {
        return Err(PolicyStoreError::InfrastructureOwned(format!(
            "policy id {policy_id} is owned by egret's own plumbing (managed_by = 'infra') — \
             it is seeded or maintained by the platform, and editing it can break delegation \
             or lock every admin out of this service. Nothing in the console can change it; \
             if it is wrong, the producer that mints it has to change"
        )));
    }
    Ok(())
}
```

Call it immediately after the existing `refuse_if_compiler_owned` call in `update_policy`, `delete_policy` and `set_policy_enabled`. A missing service or policy stays a `NotFound` from the caller's own lookup, exactly as `refuse_if_generated` already documents for itself.

- [ ] **Step 4: Run the test and watch it pass**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-policy --test infra_guard --no-fail-fast
```
Expected: PASS, 1 test.

- [ ] **Step 5: Confirm the 409 mapping**

Find where `PolicyStoreError::CompilerOwned` becomes a status code (`grep -rn 'CompilerOwned' crates/egret-policy/src/admin/`), add `InfrastructureOwned` to the same arm, and assert it:

```bash
grep -n 'CompilerOwned\|InfrastructureOwned' crates/egret-policy/src/admin/mod.rs
```
Expected: both variants in the same 409 arm. If `CompilerOwned` maps somewhere else, follow it — do not invent a status.

- [ ] **Step 6: Mutation-check the guard**

Point the comparison at the wrong value:

```rust
    if managed_by.as_deref() == Some("compiler") {
```

```bash
grep -c 'managed_by.as_deref() == Some("infra")' crates/egret-policy/src/store.rs   # must print 1
shasum -a 256 crates/egret-policy/src/store.rs   # record
# apply, then:
cargo build -p egret-policy --tests 2>&1 | grep -c '^error\['   # must print 0
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-policy --test infra_guard --no-fail-fast
```
Expected: the refusal assertions FAIL. Restore by plain write; confirm the SHA-256 matches.

- [ ] **Step 7: Run the crate and commit**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-policy --no-fail-fast > /tmp/t2.log 2>&1; echo "EXIT=$?"
grep -c '^error\[' /tmp/t2.log
grep 'test result:' /tmp/t2.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
cargo clippy -p egret-policy --no-deps --all-targets
git add crates/egret-policy/src/store.rs crates/egret-policy/src/admin/mod.rs \
        crates/egret-policy/tests/infra_guard.rs
git commit -m "feat(policy): refuse a human write to a policy egret's plumbing owns"
```

---

## Task 3: the closed grammar for access policies → `Grant`

**Files:**
- Create: `crates/egret-perm-compile/src/grammar.rs`
- Modify: `crates/egret-perm-compile/src/lib.rs` (declare the module)
- Test: `crates/egret-perm-compile/tests/grammar_access.rs` (new)

**Interfaces:**
- Consumes: `egret_perm::{Action, Effect, Grant, PrincipalSet, Scope}`, `egret_perm::actions::engine_accesses`, `egret_perm::EngineType`
- Produces:

```rust
pub enum Blocker {
    UnknownAccessType { access_type: String },
    PartialActionExpansion { action: Action, missing: Vec<String> },
    UnregisteredCatalog { catalog: String },
    ResourceFlag { level: String, flag: &'static str },
    TooManyExceptions { count: usize },
    ExceptionAccessMismatch,
    ValiditySchedules,
    Conditions,
    NonDefaultPriority { priority: i64 },
    EmptyLevel { level: String },
    WildcardOverNamedLevel { level: String },
    UnknownTag { key: String },
    NothingToMigrate,
}

pub struct CandidateGrants {
    pub grants: Vec<Grant>,
}

pub fn grants_from_access_policy(
    body: &serde_json::Value,
    engine: EngineType,
    catalog_ids: &dyn Fn(&str) -> Option<uuid::Uuid>,
    service: ServiceKind,              // added in Task 4's fix round 1
) -> Result<CandidateGrants, Vec<Blocker>>;
```

> **Amended (Task 4, fix round 1, 2026-10-10).** Both grammars take the
> `ServiceKind` (`Resource` | `Tag`) of the service the body is stored in. A
> path-addressed body in a tag service, or a tag-addressed body in a resource
> service, matches nothing where it is, and would start enforcing once compiled
> back into the matching service — so it is refused (`PathBodyInTagService` /
> `TagBodyInResourceService`). The test code below predates the parameter; the
> real calls pass `ServiceKind::Resource` last.

`catalog_ids` is a closure rather than a DB handle so the grammar stays **pure** — the same property that lets `egret-ranger-eval` be differentially tested. The caller (Task 5) supplies a lookup over `registry.catalogs`.

- [ ] **Step 1: Write the failing test, both directions and the subset trap**

Create `crates/egret-perm-compile/tests/grammar_access.rs`:

```rust
//! The closed grammar for access policies. Three things are asserted, and the
//! second is the one a field-by-field translator gets wrong.

use egret_perm::{Action, EngineType};
use egret_perm_compile::grammar::{grants_from_access_policy, Blocker};
use serde_json::json;
use uuid::Uuid;

const CAT: Uuid = Uuid::from_u128(0x1111_1111_1111_1111_1111_1111_1111_1111);

fn lookup(name: &str) -> Option<Uuid> {
    if name == "wh" { Some(CAT) } else { None }
}

fn policy(accesses: &[&str]) -> serde_json::Value {
    json!({
        "name": "sales can read orders",
        "resources": {
            "catalog": {"values": ["wh"], "isExcludes": false, "isRecursive": false},
            "schema":  {"values": ["sales"], "isExcludes": false, "isRecursive": false},
            "table":   {"values": ["orders"], "isExcludes": false, "isRecursive": false}
        },
        "policyItems": [{
            "users": ["alice"],
            "groups": [],
            "roles": [],
            "accesses": accesses.iter().map(|a| json!({"type": a, "isAllowed": true})).collect::<Vec<_>>()
        }],
        "denyPolicyItems": [],
        "allowExceptions": [],
        "denyExceptions": [],
        "isEnabled": true
    })
}

/// A full expansion is accepted and produces exactly the grant it should.
#[test]
fn a_full_action_expansion_becomes_one_grant() {
    let got = grants_from_access_policy(&policy(&["select"]), EngineType::Trino, &lookup)
        .expect("select is Action::Read on Trino");
    assert_eq!(got.grants.len(), 1);
    assert_eq!(got.grants[0].actions, vec![Action::Read]);
    assert_eq!(got.grants[0].scope.catalog_id, CAT);
    assert_eq!(got.grants[0].scope.schema.as_deref(), Some("sales"));
    assert_eq!(got.grants[0].scope.table.as_deref(), Some("orders"));

    let got = grants_from_access_policy(&policy(&["insert", "delete"]), EngineType::Trino, &lookup)
        .expect("insert+delete is exactly Action::Write on Trino");
    assert_eq!(got.grants[0].actions, vec![Action::Write]);
}

/// THE SUBSET TRAP. `Action::Write` expands to insert+delete on Trino, so a
/// policy granting only `insert` is NOT expressible: migrating it as `Write`
/// would silently add `delete`. A grammar that only rejected UNKNOWN access
/// types would accept this and widen access.
#[test]
fn a_strict_subset_of_an_action_expansion_is_refused_and_names_what_is_missing() {
    let blockers = grants_from_access_policy(&policy(&["insert"]), EngineType::Trino, &lookup)
        .expect_err("insert alone is a strict subset of Action::Write on Trino");
    assert!(
        blockers.iter().any(|b| matches!(
            b,
            Blocker::PartialActionExpansion { action: Action::Write, missing }
                if missing == &vec!["delete".to_string()]
        )),
        "expected a PartialActionExpansion naming `delete`, got {blockers:?}"
    );

    // The same body IS expressible on StarRocks? No — there Write is
    // insert+update+delete, so `insert` alone is still a subset, and the
    // missing list is longer. Asserting both engines keeps the expansion table
    // from being read as engine-independent.
    let blockers = grants_from_access_policy(&policy(&["insert"]), EngineType::StarRocks, &lookup)
        .expect_err("insert alone is a strict subset of Action::Write on StarRocks too");
    assert!(blockers.iter().any(|b| matches!(
        b,
        Blocker::PartialActionExpansion { action: Action::Write, missing }
            if missing.len() == 2
    )));
}

/// Every other closed-grammar refusal, each naming its own blocker. A refusal
/// that does not say what blocked it is unusable by an operator and
/// unmeasurable by a test — and a classifier that refused everything would
/// pass a test that only checked `is_err()`.
#[test]
fn each_inexpressible_shape_names_its_own_blocker() {
    let unknown_access = grants_from_access_policy(&policy(&["alter"]), EngineType::Trino, &lookup)
        .expect_err("alter has no Action");
    assert!(unknown_access.iter().any(|b| matches!(
        b,
        Blocker::UnknownAccessType { access_type } if access_type == "alter"
    )));

    let mut unregistered = policy(&["select"]);
    unregistered["resources"]["catalog"]["values"] = json!(["not_in_registry"]);
    let blockers = grants_from_access_policy(&unregistered, EngineType::Trino, &lookup)
        .expect_err("an unregistered catalog has no Scope.catalog_id to point at");
    assert!(blockers.iter().any(|b| matches!(
        b,
        Blocker::UnregisteredCatalog { catalog } if catalog == "not_in_registry"
    )));

    let mut recursive = policy(&["select"]);
    recursive["resources"]["schema"]["isRecursive"] = json!(true);
    let blockers = grants_from_access_policy(&recursive, EngineType::Trino, &lookup)
        .expect_err("grants.rs::level hard-codes is_recursive: false");
    assert!(blockers.iter().any(|b| matches!(
        b,
        Blocker::ResourceFlag { level, flag } if level == "schema" && *flag == "isRecursive"
    )));

    let mut excludes = policy(&["select"]);
    excludes["resources"]["schema"]["isExcludes"] = json!(true);
    let blockers = grants_from_access_policy(&excludes, EngineType::Trino, &lookup)
        .expect_err("grants.rs::level hard-codes is_excludes: false");
    assert!(blockers.iter().any(|b| matches!(
        b,
        Blocker::ResourceFlag { level, flag } if level == "schema" && *flag == "isExcludes"
    )));

    let mut scheduled = policy(&["select"]);
    scheduled["validitySchedules"] = json!([{"startTime": "2026/01/01 00:00:00"}]);
    assert!(
        grants_from_access_policy(&scheduled, EngineType::Trino, &lookup)
            .expect_err("nothing in the model carries a schedule")
            .iter()
            .any(|b| matches!(b, Blocker::ValiditySchedules))
    );

    let mut prioritised = policy(&["select"]);
    prioritised["policyPriority"] = json!(1);
    assert!(
        grants_from_access_policy(&prioritised, EngineType::Trino, &lookup)
            .expect_err("a non-default priority changes arbitration and has no model field")
            .iter()
            .any(|b| matches!(b, Blocker::NonDefaultPriority { priority: 1 }))
    );

    let mut two_exceptions = policy(&["select"]);
    two_exceptions["allowExceptions"] = json!([
        {"users": ["bob"], "groups": [], "roles": [], "accesses": [{"type": "select", "isAllowed": true}]},
        {"users": ["carol"], "groups": [], "roles": [], "accesses": [{"type": "select", "isAllowed": true}]}
    ]);
    assert!(
        grants_from_access_policy(&two_exceptions, EngineType::Trino, &lookup)
            .expect_err("principals.exclude emits exactly one exception item")
            .iter()
            .any(|b| matches!(b, Blocker::TooManyExceptions { count: 2 }))
    );
}

/// One exception with the SAME access set is expressible, and lands in
/// `principals.exclude`. This is the allow cell for the exception rule — the
/// refusal above proves nothing on its own.
#[test]
fn one_exception_with_the_same_accesses_becomes_principals_exclude() {
    let mut one = policy(&["select"]);
    one["allowExceptions"] = json!([
        {"users": ["bob"], "groups": [], "roles": [], "accesses": [{"type": "select", "isAllowed": true}]}
    ]);
    let got = grants_from_access_policy(&one, EngineType::Trino, &lookup)
        .expect("one exception, same accesses, is expressible");
    assert_eq!(got.grants.len(), 1);
    assert_eq!(got.grants[0].principals.exclude.len(), 1);
}

/// A policy carrying both allow and deny items becomes TWO grants — N objects
/// from one policy is allowed, all-or-nothing is about whether ALL of them are
/// expressible.
#[test]
fn allow_and_deny_items_become_two_grants() {
    let mut both = policy(&["select"]);
    both["denyPolicyItems"] = json!([
        {"users": ["dave"], "groups": [], "roles": [], "accesses": [{"type": "select", "isAllowed": true}]}
    ]);
    let got = grants_from_access_policy(&both, EngineType::Trino, &lookup)
        .expect("both item kinds are expressible");
    assert_eq!(got.grants.len(), 2);
    assert_eq!(
        got.grants.iter().filter(|g| g.effect == egret_perm::Effect::Deny).count(),
        1
    );
}
```

- [ ] **Step 2: Run it and watch it fail**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-perm-compile --test grammar_access --no-fail-fast
```
Expected: FAIL to compile — `unresolved import egret_perm_compile::grammar`.

- [ ] **Step 3: Write the grammar**

Create `crates/egret-perm-compile/src/grammar.rs`. The module doc must carry the argument, because the next reader's instinct will be to loosen it:

```rust
//! A CLOSED grammar: a Ranger access-policy body becomes either candidate
//! [`Grant`]s or a list of named [`Blocker`]s. Pure — no DB, no clock, no
//! network — so it can be differentially tested, which is the same property
//! that makes `egret-ranger-eval` trustworthy.
//!
//! **Closed, not best-effort.** The model's vocabulary is far narrower than
//! Ranger's: `Action` has two values and expands to a FIXED access-type set per
//! engine (`egret_perm::actions::engine_accesses`), `Scope` names ONE catalog by
//! registry UUID, `principals.exclude` emits exactly one exception item, and
//! `grants.rs::level` hard-codes `is_excludes: false` and `is_recursive: false`.
//! Anything outside that is refused with a blocker that names it.
//!
//! **The subset rule is the whole point.** `Action::Write` is atomic: on Trino
//! it expands to `insert` + `delete`. A policy granting only `insert` is
//! therefore NOT expressible — accepting it as `Write` would ADD `delete`,
//! widening access nobody asked to widen. So an access set must equal a union
//! of FULL expansions; a strict subset of one is
//! [`Blocker::PartialActionExpansion`], carrying what is missing so the
//! operator can see why. A translator that only rejected unknown access types
//! would make exactly this mistake, silently, and in the fail-open direction.
//!
//! **Why the catalog lookup is a closure.** `Scope.catalog_id` is a
//! `registry.catalogs` UUID, so a policy naming a catalog egret does not have a
//! row for cannot become a grant at all — which is precisely the "hand-written
//! profile for an external engine" case SP4a left in place. Taking the lookup
//! as `&dyn Fn(&str) -> Option<Uuid>` keeps this module pure and lets its tests
//! state the registry inline.
```

Then the implementation, in this order so the blocker list is complete rather than first-error: collect every blocker, return `Err(all_of_them)` if any, and only build grants when the list is empty. An operator fixing one blocker at a time, with only the first one shown, is the difference between a usable surface and a guessing game.

- [ ] **Step 4: Run the tests and watch them pass**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-perm-compile --test grammar_access --no-fail-fast
```
Expected: PASS, 5 tests.

- [ ] **Step 5: Mutation-check the subset rule**

Replace the equality check with a subset check — the exact mistake the rule exists to prevent:

```rust
// MUTANT: accept a subset instead of requiring the full expansion
if expansion.iter().any(|a| accesses.contains(a)) { /* accept */ }
```

```bash
shasum -a 256 crates/egret-perm-compile/src/grammar.rs   # record
cargo build -p egret-perm-compile --tests 2>&1 | grep -c '^error\['   # must print 0
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-perm-compile --test grammar_access --no-fail-fast
```
Expected: `a_strict_subset_of_an_action_expansion_is_refused_and_names_what_is_missing` FAILS, and it is the only failure. Restore by plain write; confirm the SHA-256.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-perm-compile/src/grammar.rs crates/egret-perm-compile/src/lib.rs \
        crates/egret-perm-compile/tests/grammar_access.rs
git commit -m "feat(perm-compile): a closed grammar from Ranger access policies to grants"
```

---

## Task 4: the grammar for masking, row filters and tag access

**Files:**
- Modify: `crates/egret-perm-compile/src/grammar.rs`
- Test: `crates/egret-perm-compile/tests/grammar_rules.rs` (new)

**Interfaces:**
- Produces:

```rust
pub struct CandidateRules {
    pub rules: Vec<egret_perm::Rule>,
}

pub fn rules_from_mask_or_filter_policy(
    body: &serde_json::Value,
    engine: EngineType,
    catalog_ids: &dyn Fn(&str) -> Option<uuid::Uuid>,
    tag_names: &dyn Fn(&str) -> Option<String>,   // tags.key → that tag's tags.name
    service: ServiceKind,
) -> Result<CandidateRules, Vec<Blocker>>;
```

`tag_names` resolves `tags.key` — **never** `tags.name`, which is mutable and non-unique (SP4a D6) — and answers the resolved tag's NAME. The grammar accepts a tag body only when that name equals the body's tag value exactly (`Blocker::TagNameDiffersFromKey` otherwise): the engine matches tag policies on names and the compiler stamps the resolved tag's name back, so a key ≠ name tag would compile to a policy on a different tag.

> **Amended (Task 4, fix round 1, 2026-10-10 — user rulings).** (1) The
> `clearances` field is REMOVED: clearance migration is out of scope (see the
> spec's "Out of scope"). A clearance's cleared set is role membership expanded
> at compile time, and a user list cannot be turned back into roles; a
> tag-addressed access body is refused as `Blocker::ClearanceNotExpressible`.
> (2) `tag_ids → Uuid` became `tag_names → String` with the name check above.
> (3) The `service: ServiceKind` parameter (see Task 3's amendment).

- [ ] **Step 1: Write the failing test**

Create `crates/egret-perm-compile/tests/grammar_rules.rs` covering, each with both cells:

1. A `dataMaskPolicyItems` policy whose resource is catalog+schema+table+column and whose mask type the model supports → one `Rule` with `RuleKind::Mask`.
2. A `rowFilterPolicyItems` policy whose resource is catalog+schema+table → one `Rule` with `RuleKind::RowFilter`, and whose arity is exactly three levels (a column makes it `Blocker::EmptyLevel`-adjacent — assert the real blocker your implementation returns and name it in the test).
3. A tag-addressed mask whose `tags.key` is absent from the registry → `Blocker::UnknownTag { key }`, **and** present → accepted as `Selector::Tag`.
4. A mask item with **no** `dataMaskInfo` → refused. This one matters beyond expressibility: on the TAG policy repository a tag mask item with no `dataMaskInfo` makes the plugin's constructor NPE, so the engine serves **nothing**. A policy like that must never be reproduced into a compiled artifact.
5. A `*` above a named level: refused on a grant (Task 3) but **accepted on a rule** — the product decision of 2026-10-10. Assert the rule cell here so the asymmetry is pinned on both sides of the codebase.

- [ ] **Step 2: Run it, watch it fail, implement, run it again**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-perm-compile --test grammar_rules --no-fail-fast
```
Expected first: FAIL (unresolved function). Expected after: PASS.

- [ ] **Step 3: Mutation-check the `dataMaskInfo` refusal**

Drop the check. Expected: the case-4 test fails and nothing else. `count == 1`, mutant compiles, restore by plain write with a SHA-256 compare.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-perm-compile/src/grammar.rs crates/egret-perm-compile/tests/grammar_rules.rs
git commit -m "feat(perm-compile): grammar for masking, row filters and tag access"
```

---

## Task 5: the classifier and `GET /classify`

**Files:**
- Create: `crates/egret-perm-compile/src/classify.rs`
- Modify: `crates/egret-perm-compile/src/routes.rs`
- Test: `crates/egret-perm-compile/tests/classify_route.rs` (new)

**Interfaces:**
- Consumes: `grammar::{grants_from_access_policy, rules_from_mask_or_filter_policy, Blocker}`, `policy.authz_policies.managed_by`
- Produces:

```rust
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct ClassifiedPolicy {
    pub id: i64,
    pub name: String,
    pub managed_by: String,          // 'compiler' | 'infra' | 'operator'
    pub enabled: bool,
    pub migratable: bool,
    pub blockers: Vec<String>,       // rendered sentences, not enum names
    pub proposed: Option<ProposedObjects>,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct ProposedObjects {
    pub grants: usize,
    pub rules: usize,
    // No `clearances`: clearance migration is out of scope (Task 4 fix round 1).
}
```

Route: `GET /perm/policies/services/{service_id}/classify` on the existing `perm_router`, gated by the same `require(…, "policy:read")` its neighbours use.

A `compiler`-owned or `infra`-owned policy is reported with `migratable: false` and a blocker sentence saying why it is not a migration candidate — **not** omitted. A protection nobody can see is one somebody will route around, and the same is true of a classification.

- [ ] **Step 1: Write the failing route test**

Assert, on a fixture holding one of each owner plus one migratable and one blocked operator policy:
- every policy in the profile appears in the response, including `infra` and `compiler` ones;
- the migratable one has `migratable: true` and a non-empty `proposed`;
- the blocked one has `migratable: false` and a blocker **sentence** that contains the offending value (e.g. the access type `alter`), not just an enum name;
- the response is ordered by `id` so the page is stable.

- [ ] **Step 2: Run, fail, implement, run**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-perm-compile --test classify_route --no-fail-fast
```

- [ ] **Step 3: Regenerate the client and commit**

```bash
cargo run -q --bin egret -- openapi > /tmp/openapi.new.json
python3 -c "import json;json.load(open('/tmp/openapi.new.json'))"   # valid JSON before installing
cat /tmp/openapi.new.json > openapi.json
cd frontend && npm run gen:api && cd ..
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-server --lib
```
Expected: `committed_openapi_json_is_current` and `generated_client_was_emitted_from_the_committed_spec` both `ok` **by name**. Commit exactly what `npm run gen:api` produced.

---

## Task 6: probe derivation and the equivalence gate

**Files:**
- Create: `crates/egret-perm-compile/src/equivalence.rs`
- Test: `crates/egret-perm-compile/tests/equivalence.rs` (new)

**Interfaces:**
- Consumes: `egret_ranger_eval::{evaluate, evaluate_data_mask, evaluate_row_filter, AccessRequest, EngineConfig, PluginVersion, ServicePolicySet, Decision}`
- Produces:

```rust
pub struct Disagreement {
    pub plugin_version: PluginVersion,
    pub user: String,
    pub access_type: String,
    pub resource: String,          // rendered coordinate, e.g. "wh.sales.orders"
    pub before: String,            // rendered Decision / MaskResult / RowFilterResult
    pub after: String,
}

/// Probes derived from BOTH policy sets, plus probes neither mentions.
pub fn derive_probes(
    before: &ServicePolicySet,
    after: &ServicePolicySet,
) -> Vec<AccessRequest>;

/// Empty vec = equivalent on every probe, on every version in `versions`.
pub fn compare(
    before: &ServicePolicySet,
    after: &ServicePolicySet,
    service_def: &serde_json::Value,
    versions: &[PluginVersion],
    probes: &[AccessRequest],
) -> Vec<Disagreement>;
```

- [ ] **Step 1: Write the failing test**

Three cases, and the third is the one that justifies the whole gate:

1. **Equivalent** — a hand-written policy and its compiled grant decide the same on every probe, on both versions: `compare` returns empty.
2. **Not equivalent** — the compiled grant omits one user: `compare` returns a `Disagreement` naming that user, that access type and that resource.
3. **Equivalent on 2.5.0, different on 2.8.0** — build the known divergence (a tag DENY plus a higher-priority resource ALLOW with no deny items, Allowed on 2.8.0 and Denied on 2.5.0) and assert `compare` reports a disagreement **for 2.8.0 only**. A gate that evaluated one version would pass this and ship a wrong artifact to StarRocks.

And one test for `derive_probes` itself:

4. **The probe set reaches past both policies' vocabulary** — given a before/after pair that only ever mentions `wh.sales.orders` and `alice`, assert the derived probes include at least one coordinate that is a *sibling* at each level and at least one principal mentioned by neither. Without this the probe set is a correlated oracle: it inherits exactly the blind spots of the two policies it came from, and this programme has already shipped one of those.

- [ ] **Step 2: Run, fail, implement, run**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-perm-compile --test equivalence --no-fail-fast
```

- [ ] **Step 3: Mutation-check the per-version loop**

Make `compare` evaluate only `versions[0]`. Expected: case 3 fails and nothing else. `count == 1`, mutant compiles, restore by plain write with a SHA-256 compare.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-perm-compile/src/equivalence.rs crates/egret-perm-compile/tests/equivalence.rs
git commit -m "feat(perm-compile): prove a migration equivalent per plugin version"
```

---

## Task 7: the migration transaction, provenance, and undo

**Files:**
- Create: `migrations/0181_perm_migrated_from.sql` (check the number against `origin/main` first)
- Create: `crates/egret-perm-compile/src/migrate.rs`
- Modify: `crates/egret-perm-compile/src/routes.rs`
- Test: `crates/egret-perm-compile/tests/migrate_route.rs` (new)

**Interfaces:**
- Consumes: `grammar`, `equivalence`, `egret_perm_compile::trigger::recompile_service_in_tx(tx, service_id) -> Result<CompileSummary, CompileError>`, `egret_policy::PolicyStore::set_policy_enabled`
- Produces: `POST …/migrate { policy_names[], dry_run }` and `POST …/migrate/undo { policy_names[] }`

The transaction, in this order — and the order is the design, not an implementation detail:

1. Insert the N `perm.grants` / `perm.rules` rows the grammar produced.
2. `recompile_service_in_tx(tx, service_id)`.
3. Set `enabled = false` on the original policy row.
4. Insert provenance.
5. **Re-derive probes and re-run `compare` against the state now in the transaction.** Any disagreement → return it to the caller and `ROLLBACK`.

Step 5 is why the order is what it is: the gate proves its claim about **the artifact the engine will actually download after the write**, not about a predicted one. `dry_run` runs the identical code and differs in exactly one place — it always rolls back.

- [ ] **Step 1: Write the migration**

```sql
-- 0181_perm_migrated_from.sql
--
-- SP4d-3 Task 7: which hand-written policy became which model objects.
--
-- Two consumers, not one: the "migrated from …" badge on the operator's page,
-- and `POST …/migrate/undo`, which is a MECHANICAL rollback rather than a
-- reconstruction precisely because the original policy was switched off and not
-- deleted (the user's ruling of 2026-10-10, and the standing rule from
-- 2026-09-16 about never deleting what the cluster may still hold).
--
-- `probes_proved` is recorded because a migration is only as trustworthy as the
-- number of probes that agreed, and a later reader deserves to see that number
-- rather than take "it was proved" on faith.

CREATE TABLE perm.migrated_from (
    id            bigserial PRIMARY KEY,
    service_id    bigint      NOT NULL REFERENCES policy.authz_services(id) ON DELETE CASCADE,
    policy_name   text        NOT NULL,
    grant_ids     uuid[]      NOT NULL DEFAULT '{}',
    rule_ids      uuid[]      NOT NULL DEFAULT '{}',
    probes_proved integer     NOT NULL,
    migrated_by   text        NOT NULL,
    migrated_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT migrated_from_names_something CHECK (
        cardinality(grant_ids) + cardinality(rule_ids) > 0
    ),
    CONSTRAINT migrated_from_policy_name_is_named CHECK (policy_name <> ''),
    CONSTRAINT migrated_from_is_once_per_policy UNIQUE (service_id, policy_name)
);
```

The `UNIQUE` is what makes a second migration of the same policy a 409 rather than a duplicate, in the schema rather than in a predicate somebody must remember to call.

- [ ] **Step 2: Write the failing route test**

Five assertions, each of which can fail on its own:

1. **Happy path** — a migratable policy: the response names the created objects, `perm.grants` gains a row, the original policy's `enabled` is `false`, `perm.migrated_from` gains a row with `probes_proved > 0`, and the downloaded artifact no longer contains the original policy's name.
2. **`dry_run: true` writes nothing** — identical response shape, and afterwards `perm.grants`, `perm.migrated_from` and the original's `enabled` are all unchanged. Assert all three; a dry run that forgot one of them looks correct from the other two.
3. **A non-equivalent migration rolls back** — force a disagreement, assert the response carries the `Disagreement` (user, access type, resource, plugin version) **and** that nothing was written. A rollback that returns a good error while leaving half a write behind is the worst outcome of the three.
4. **Second migration of the same policy is 409**, with no second provenance row.
5. **`undo` restores** — after `undo`, the model objects are gone, the original's `enabled` is `true`, the provenance row is gone, and the artifact contains the original again.

- [ ] **Step 3: Run, fail, implement, run**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-perm-compile --test migrate_route --no-fail-fast > /tmp/t7.log 2>&1; echo "EXIT=$?"
grep -c '^error\[' /tmp/t7.log
grep 'test result:' /tmp/t7.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
```

- [ ] **Step 4: Mutation-check the in-transaction re-gate**

Remove step 5. Expected: assertion 3 fails and nothing else — which is the proof that the gate runs against what was written rather than against a prediction. `count == 1`, mutant compiles, restore by plain write with a SHA-256 compare.

- [ ] **Step 5: Regenerate the client, run the crate, commit**

```bash
cargo run -q --bin egret -- openapi > /tmp/openapi.new.json && cat /tmp/openapi.new.json > openapi.json
cd frontend && npm run gen:api && cd ..
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-server --lib
git add migrations/0181_perm_migrated_from.sql crates/egret-perm-compile/src/migrate.rs \
        crates/egret-perm-compile/src/routes.rs crates/egret-perm-compile/tests/migrate_route.rs \
        openapi.json frontend/src/api/generated frontend/src/api/generated.digest.json
git commit -m "feat(perm-compile): migrate a hand-written policy, provably or not at all"
```

---

## Task 8: the differential corpus against the real Ranger engines

**Files:**
- Modify: `crates/egret-ranger-eval/tests/differential.rs`
- Modify: `justfile` (the `test-ranger-gates` recipe, lines 154-158)

**Interfaces:**
- Consumes: `Case::to_json(&self, service_type: &str) -> serde_json::Value`, `ask_ranger(cases: &[Case], service_type: &str, ranger_version: &str, tag: &str) -> Vec<serde_json::Value>`, `fn trino() -> Profile` (ranger 2.5.0), `fn starrocks() -> Profile` (ranger 2.8.0)

⚠ **The harness takes ONE policy set per case.** Measured: `Case` carries a single `service_policies` field, `Case::to_json` emits exactly one `servicePolicies` key, and `EgretCaseRunner.runOne` deserialises one `ServicePolicies` and calls `evaluatePolicies` once. The diff axis it supports is the **Ranger version**, not the policy-set version. So emit the before-state and the after-state as **two separate cases** with paired ids (`mig-0001-before` / `mig-0001-after`) and compare the verdicts **pairwise on the Rust side** after `ask_ranger` returns. **Do not modify the Java** — this is a constraint on the generator, not a missing feature.

The verdict fields to compare, read off the harness: `isAllowed`, `isAccessDetermined`, `policyId`, `maskType`, `maskedValue`, `maskCondition`, `filterExpr`. An entry carrying `error` fails the run.

**The corpus is enumerated from the service-def, not invented.** Walk every `accessType` and every resource hierarchy in `service-defs/trino.json` and the StarRocks def, so the `create` / `drop` / `alter` / `execute` refusals are actually *reached*. A corpus written from the author's imagination measures only the shapes the author imagined — which is the stated risk of this whole package.

- [ ] **Step 1: Write the test**

`a_migration_the_grammar_accepts_decides_identically_on_the_real_engine`, gated on `EGRET_TEST_RANGER_E2E=1` like its neighbour, asserting:
- for every generated manual policy the grammar **accepts**: the before and after verdicts are equal on every probe, at both 2.5.0 and 2.8.0;
- for every policy the grammar **refuses**: a blocker is named. Count the refusals and assert the count is **non-zero** — a corpus that happened to generate only acceptable policies would otherwise report a vacuous pass;
- report both counts, so a later reader sees the population and not just "green".

- [ ] **Step 2: Run it for real**

```bash
EGRET_TEST_RANGER_E2E=1 DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-ranger-eval --test differential --no-fail-fast -- --nocapture \
  > /tmp/t8.log 2>&1; echo "EXIT=$?"
grep -c '^error\[' /tmp/t8.log
grep 'test result:' /tmp/t8.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
grep -E 'finished in|accepted=|refused=' /tmp/t8.log
```
Expected: PASS, and **elapsed time well above `0.00 s`** — the vacuous form of this gate is `0.00 s`, so report the number. Also report the accepted and refused counts.

- [ ] **Step 3: Add it to the gate recipe and commit**

Add the new test to `test-ranger-gates` beside the two that are already there, keeping `EGRET_TEST_RANGER_E2E=1` **literal** on the line (it is literal on both existing lines, which is what stops a missing variable from silently skipping the gate).

```bash
just test-ranger-gates
git add crates/egret-ranger-eval/tests/differential.rs justfile
git commit -m "test(ranger-eval): prove migrations against the real 2.5.0 and 2.8.0 engines"
```

---

## Task 9: the operator's page

**Files:**
- Create: `frontend/src/routes/governance/PolicyMigrationPage.tsx`
- Create: `frontend/src/routes/governance/__tests__/PolicyMigrationPage.test.tsx`
- Modify: `frontend/src/api/policies.ts`
- Modify: `frontend/src/App.tsx` (`GovernanceRoutes`)
- Modify: `frontend/src/components/layout/Sidebar.tsx` (`GOVERNANCE_GROUPS`)

**Interfaces:**
- Consumes: `policyAdmin*`-style generated functions for the two new routes, from `frontend/src/api/generated/…`; `FilterableTable<T>` and `Column<T>` from `@/components/explore/FilterableTable`
- Follows: the preview-staleness pattern in `frontend/src/routes/governance/GrantDialog.tsx` (`preview` / `previewError` / `previewedFor` / `previewStale`, with the commit button gated on a non-stale preview)

The route, added to the `GovernanceRoutes` block beside the four existing `policies` routes:

```tsx
<Route
  path="policies/:service/migrate"
  element={
    <RequirePermission perm="policy:read">
      <PolicyMigrationPage />
    </RequirePermission>
  }
/>
```

The page:
- `FilterableTable` with its built-in `selection={{ selected, onToggle }}` prop — the same prop `frontend/src/routes/control-plane/access/UsersPage.tsx` uses, not a hand-rolled checkbox column.
- An owner chip per row (`infra` / `compiler` / `operator`). `infra` rows render **locked with their reason**, never hidden.
- Blockers as sentences, from the API — the frontend does not re-derive them. A second copy of the grammar in TypeScript would be the two-spellings defect in its most expensive form.
- A preview panel driven by `migrate` with `dry_run: true`, and **Migrate stays disabled until a non-stale preview exists for the current selection** — the `previewStale` guard from `GrantDialog.tsx`. Changing the selection must invalidate the preview.

- [ ] **Step 1: Write the failing tests**

In `frontend/src/routes/governance/__tests__/PolicyMigrationPage.test.tsx`:

1. `infra` rows render locked, with the reason text visible, and their checkbox is not selectable.
2. A blocked operator policy renders its blocker sentence **including the offending value**.
3. Migrate is **disabled** before a preview, **enabled** after a preview for the current selection, and **disabled again** when the selection changes. All three states — a test that only asserts the enabled state would pass against a button that is always enabled.
4. The disagreement case renders the disagreeing probe (user, access type, resource, plugin version), not a generic failure message.

⚠ Assert the disabled state on the **button element itself** and not via a helper that walks up to an ancestor — `toBeDisabled` reports true for a child of a disabled `fieldset`, which has produced a false pass in this repo before.

- [ ] **Step 2: Run, fail, implement, run**

```bash
cd frontend && npx vitest run src/routes/governance/__tests__/PolicyMigrationPage.test.tsx
```
Use the real path including `__tests__/` — a path missing it exits 0 with "No test files found", which looks exactly like a pass.

- [ ] **Step 3: Full frontend check and commit**

```bash
cd frontend
npx tsc --noEmit
npm run lint
npm run test -- --run > /tmp/t9.log 2>&1; echo "EXIT=$?"
grep -E 'Tests +[0-9]+ (passed|failed)' /tmp/t9.log
cd ..
git add frontend/src/routes/governance/PolicyMigrationPage.tsx \
        frontend/src/routes/governance/__tests__/PolicyMigrationPage.test.tsx \
        frontend/src/api/policies.ts frontend/src/App.tsx \
        frontend/src/components/layout/Sidebar.tsx
git commit -m "feat(governance): a page that says what each policy is and migrates the ones it can"
```
If `npm run lint` fails with `ERR_MODULE_NOT_FOUND: eslint-plugin-jsx-a11y-x`, run `npm ci` first — the package is declared and locked but can be missing from an older `node_modules`.

---

## Task 10: the live check

**Files:** none committed except the report.

This is a task, not an afterthought. In this programme the browser found four defects on a page carrying 26–33 green tests, and the differential corpus found nine that 127 unit tests did not. The kind cluster that held the original policy corpus is gone, so this includes rebuilding the fixture.

- [ ] **Step 1: Bring up the stack**

```bash
just up          # reads deploy/docker-compose.dev.yml
df -h /
```
`valkey` and `rustfs` must both come up; `minio` no longer exists in that file.

- [ ] **Step 2: Build the fixture by hand**

Create a profile, add three hand-written policies through `POST /policies` — one fully migratable, one blocked on a `create` access type, one blocked on an unregistered catalog — and one seeded infra policy. Record the ids.

- [ ] **Step 3: Measure before**

Run a real query through the engine as the policy's subject and record the result. This is the only measurement that is not a model of the engine.

- [ ] **Step 4: Drive the page in the browser**

Log in, open the migration page, and check with your own eyes: the infra row is locked with its reason, the two blocked rows name their blockers including the offending values, Migrate is disabled until a preview exists, and the preview's probe table is readable.

- [ ] **Step 5: Migrate, then measure again**

Migrate the migratable policy and re-run the **same** query. The enforcement must change exactly as the preview said. Then `undo` and re-run it a third time: it must return to the original answer.

- [ ] **Step 6: The whole-table agreement check that no unit test can do**

Task 1's agreement test is scoped to its own fixture, because the test database is shared with
concurrent tests. Here the database is yours, so run the real thing — every row, no `WHERE`:

```bash
psql "$CLUSTER_DATABASE_URL" -X -q -A -F' | ' -c "
SELECT managed_by,
       CASE WHEN name LIKE 'perm-grant-%' OR name LIKE 'perm-rule-%'
                 OR name LIKE 'perm-clearance-%'                      THEN 'compiler'
            WHEN name IN ('egret-delegator-impersonate','egret-query-timeout')
                 OR name LIKE 'egret-admin - %' OR name LIKE 'all - %' THEN 'infra'
            ELSE 'operator' END AS from_names,
       count(*)
FROM policy.authz_policies
GROUP BY 1,2 ORDER BY 1,2;"
```

Every row of the output must have `managed_by = from_names`. Report the table **including its
total**, not just "they agreed": a zero on an empty cluster is the same output a broken query
gives, so the total is the control that makes the agreement mean something. If any row
disagrees, that is a Critical — the backfill and the predicate have drifted, which is the exact
failure the column was introduced to prevent.

- [ ] **Step 7: Write the report**

Record, in the plan's own workspace: the three query results (before / after / after-undo), the
agreement table from Step 6 with its total, what the browser showed at each step, and anything
the preview claimed that the engine did not do. **A difference between the preview and the engine
is a Critical**, not a note — the preview is the thing an operator will trust.

---

## Self-review

**Spec coverage.** Every section of the spec maps to a task: ownership column and classifier → Tasks 1, 2, 5; closed grammar → Tasks 3, 4; equivalence gate → Task 6; write path, provenance, rollback, refusal order → Task 7; the real-engine corpus and the both-directions/mutation requirements → Task 8; the page and its preview-staleness gate → Task 9; the live check → Task 10. The spec's "out of scope" list stays out: no tag creation, no `tagService`/engine-type cross-table invariant, no rewrite of infra policy content, no ambiguous-priority fix, no profile-config route.

**Placeholders.** None. Tasks 4, 5, 7 and 9 describe their tests as enumerated assertions rather than full source, which is deliberate — each item names the exact fixture, the exact field and the failure it must catch, and the surrounding tasks carry the full code for the patterns they reuse. Where an assertion depends on a signature the implementer must read (`update_policy`'s arity, the real blocker variant for a row-filter arity), the step says to read it and says explicitly not to weaken the assertion.

**Type consistency.** `Blocker` is introduced in Task 3 and extended in Task 4 (`UnknownTag`, and in its fix round 1 `TagNameDiffersFromKey`, `TagBodyInResourceService`, `PathBodyInTagService`, `Internal`), used by Task 5's `blockers: Vec<String>` rendering, and never re-spelled. `CandidateGrants` / `CandidateRules` are the grammar's only outputs. `Disagreement` is defined in Task 6 and consumed unchanged by Task 7's rollback response and Task 9's error rendering. `recompile_service_in_tx` and `set_policy_enabled` are existing signatures, quoted as they are. Migration numbers are deliberately *not* fixed in the plan: each step re-checks `origin/main` first, because that premise expiring is what produced the Critical on the previous branch.
