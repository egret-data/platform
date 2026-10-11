-- 0180_policy_managed_by.sql
--
-- SP4d-3 Task 1: make the OWNER of a Ranger policy a property of the row.
--
-- Until now the only ownership signal was the policy NAME: `refuse_reserved_name`
-- (`crates/egret-policy/src/store.rs`) reserves `perm-grant-` / `perm-rule-` /
-- `perm-clearance-` for the SP4a compiler, and nothing at all protected egret's
-- own infrastructure policies — the per-admin all-access hierarchies
-- (`egret-admin - …`, on Trino and StarRocks profiles), the thirteen-policy
-- default set (`all - …`, Trino profiles only) and the delegator's two fixed
-- names. An operator holding `policy:update` could therefore edit or
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
-- `egret_policy::infra_names::is_egret_infrastructure_policy` recognises
-- (including its one type condition: `all - …` is egret's only on a `trino`
-- profile),
-- and from the same three prefixes `is_compiler_owned_policy_name` recognises.
-- Two spellings of one fact is the failure this programme reproduced five times
-- on the branch merged as #656, so the Rust side does not get to disagree
-- silently. `crates/egret-perm-compile/tests/managed_by_agreement.rs` runs THIS
-- FILE'S OWN backfill statements (it `include_str!`s them; there is no second
-- copy of the SQL) over a fixture carrying every name shape — all three owners
-- plus near-misses such as `all-hands-reporting` — and asserts the resulting
-- column equals both the expected owner and what the Rust predicates say. That
-- is an agreement check over every SHAPE on a Trino and a StarRocks profile,
-- scoped to the fixture's own service ids because the test database is shared. It is NOT a whole-table
-- check: that one is run against a real cluster (plan Task 10 Step 6).
--
-- `DEFAULT 'operator'` is the right default for the human entry point: a policy
-- somebody creates through the editor is theirs. The three non-human writers
-- stamp their own value explicitly: the compiler (`egret-perm-compile`'s
-- `trigger.rs`) writes 'compiler'; the two seeders in `default_policies.rs` and
-- the delegator seeding (`engines.rs::seed_delegator_policies`) write 'infra'
-- through `PolicyStore::upsert_infra_policy` / `create_infra_policy`, which
-- refuse any name `is_egret_infrastructure_policy` does not recognise on the
-- profile's type. `seed_default_trino_policies` refuses a non-Trino profile
-- outright. Before this migration it did not, so a StarRocks profile may hold a
-- Trino-shaped `all - …` set its plugin rejects on every refresh; the backfill
-- below leaves those rows 'operator', so an operator can delete them.
--
-- FORMAT NOTE for whoever edits the two UPDATEs below:
-- `managed_by_agreement.rs`'s extractor strips only WHOLE-LINE `--` comments,
-- so a trailing `-- …` on a SQL line would stay in the extracted predicate and
-- comment out the scoping the test appends after it; keep every comment on its
-- own line.

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
-- as egret's and lock them out of their own policy. The `all - ` arm joins the
-- profile's type: that set is built from the Trino service-def only, so on any
-- other type it was never egret's (see above).
UPDATE policy.authz_policies
   SET managed_by = 'infra'
 WHERE name IN ('egret-delegator-impersonate', 'egret-query-timeout')
    OR name LIKE 'egret-admin - %'
    OR (name LIKE 'all - %'
        AND service_id IN (SELECT id FROM policy.authz_services WHERE type = 'trino'));
