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
-- silently. `crates/egret-perm-compile/tests/managed_by_agreement.rs` runs THIS
-- FILE'S OWN backfill statements (it `include_str!`s them; there is no second
-- copy of the SQL) over a fixture carrying every name shape — all three owners
-- plus near-misses such as `all-hands-reporting` — and asserts the resulting
-- column equals both the expected owner and what the Rust predicates say. That
-- is an agreement check over every SHAPE, scoped to the fixture's own
-- service_id because the test database is shared. It is NOT a whole-table
-- check: that one is run against a real cluster (plan Task 10 Step 6).
--
-- `DEFAULT 'operator'` is the right default for the human entry point: a policy
-- somebody creates through the editor is theirs. The compiler and the seeder
-- both stamp their own value explicitly (Task 1 Step 10).

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
