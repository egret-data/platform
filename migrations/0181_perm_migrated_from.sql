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
-- rather than take "it was proved" on faith. The row is inserted before the
-- equivalence gate runs and the count is written after it passes, in the same
-- transaction, so a committed row never carries a count the gate did not reach.
-- It is the total over every profile the gate evaluated, so every policy
-- migrated by one request records the same number.
--
-- There is no `clearance_ids` column: migrating a clearance is out of scope
-- (the user's ruling of 2026-10-10), so no migration can produce one.
--
-- KEYED BY `policy_id`, not by name (Task 7 review, I-3). An operator may
-- rename the switched-off original; keyed by name, the rename orphaned this
-- row, the page then advised "switch it on" (enforcing the policy twice), and
-- undo could not find it. `policy_name` is the name AT MIGRATION, for display.
--
-- `policy_id` has the default ON DELETE NO ACTION, deliberately. egret never
-- deletes a migrated original; an operator deleting it would make the undo
-- impossible and leave this row describing a policy that no longer exists, so
-- the delete is refused (`PolicyStore::delete_policy` turns the violation into
-- a 409 that says to undo the migration first). NO ACTION rather than
-- RESTRICT because NO ACTION is checked at the end of the statement: deleting
-- the whole PROFILE cascades to both this row (`service_id`) and the policy in
-- one statement, and must still succeed.
--
-- The hashes guard undo (Task 7 review, I-2; the user's ruling of 2026-10-11).
-- `grant_hashes[i]` / `rule_hashes[i]` are SHA-256 hex digests of the canonical
-- form of `grant_ids[i]` / `rule_ids[i]` as stored when the migration
-- committed, and `original_hash` of the original's body (without its `name`,
-- so a rename stays undoable) and its `enabled` column. Undo recomputes all of
-- them in its own transaction and refuses with 409, writing nothing, if any
-- differs: an object edited since the migration is not the migration's to
-- delete. The canonical form sorts object keys (and the order-free principal
-- lists), so it does not depend on jsonb's key order.

CREATE TABLE perm.migrated_from (
    id            bigserial PRIMARY KEY,
    service_id    bigint      NOT NULL REFERENCES policy.authz_services(id) ON DELETE CASCADE,
    policy_id     bigint      NOT NULL REFERENCES policy.authz_policies(id),
    policy_name   text        NOT NULL,
    grant_ids     uuid[]      NOT NULL DEFAULT '{}',
    rule_ids      uuid[]      NOT NULL DEFAULT '{}',
    grant_hashes  text[]      NOT NULL DEFAULT '{}',
    rule_hashes   text[]      NOT NULL DEFAULT '{}',
    original_hash text        NOT NULL,
    probes_proved integer     NOT NULL,
    migrated_by   text        NOT NULL,
    migrated_at   timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT migrated_from_names_something CHECK (
        cardinality(grant_ids) + cardinality(rule_ids) > 0
    ),
    CONSTRAINT migrated_from_hashes_every_object CHECK (
        cardinality(grant_hashes) = cardinality(grant_ids)
        AND cardinality(rule_hashes) = cardinality(rule_ids)
    ),
    CONSTRAINT migrated_from_policy_name_is_named CHECK (policy_name <> ''),
    CONSTRAINT migrated_from_is_once_per_policy UNIQUE (policy_id)
);
