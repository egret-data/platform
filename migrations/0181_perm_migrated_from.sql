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
--
-- There is no `clearance_ids` column: migrating a clearance is out of scope
-- (the user's ruling of 2026-10-10), so no migration can produce one.

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
