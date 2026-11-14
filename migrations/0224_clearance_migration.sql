-- SP3c-2: provenance of each clearance -> withhold-rule migration run.
-- One row per run; holds enough to undo exactly.
CREATE TABLE perm.clearance_migration (
    id bigserial PRIMARY KEY,
    tags text[] NOT NULL,
    rules jsonb NOT NULL,
    rule_ids uuid[] NOT NULL,
    exemption_hashes text[] NOT NULL,
    original jsonb NOT NULL,
    migrated_by text NOT NULL,
    migrated_at timestamptz NOT NULL DEFAULT now()
);
