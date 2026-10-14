-- View governance B1 (Task 7): a view's table-level lineage, as Trino reports it.
--
-- egret runs `EXPLAIN (TYPE IO, FORMAT JSON)` of a view's SQL as the principal
-- `egret-lineage` and records the BASE tables the plan lists (nested views
-- resolve through the engine, so no view ever appears here as a base). Every
-- name is fully qualified as catalog / schema / table, with the catalog as the
-- engine names it, which on an engine egret provisions is
-- `registry.catalogs.name` (0147: "the catalog's SQL name on every engine").
--
-- No foreign keys: a view lives in lakekeeper's tables, not egret's, and a base
-- may be dropped while the view stands. The B3 sweep reconciles both.

CREATE TABLE view_lineage (
    view_catalog text NOT NULL,
    view_schema  text NOT NULL,
    view_name    text NOT NULL,
    base_catalog text NOT NULL,
    base_schema  text NOT NULL,
    base_table   text NOT NULL,
    PRIMARY KEY (view_catalog, view_schema, view_name, base_catalog, base_schema, base_table),
    CONSTRAINT view_lineage_names_not_empty CHECK (
        view_catalog <> '' AND view_schema <> '' AND view_name <> ''
        AND base_catalog <> '' AND base_schema <> '' AND base_table <> '')
);

-- The base-table side, for B3: "which views read this table".
CREATE INDEX view_lineage_base_idx ON view_lineage (base_catalog, base_schema, base_table);

-- Whether the lineage was last computed or last failed, and why. A `computed`
-- row with no `view_lineage` row cannot be written through egret: an empty
-- lineage is a refusal (spec R6), recorded as `failed`.
CREATE TABLE view_lineage_state (
    view_catalog text NOT NULL,
    view_schema  text NOT NULL,
    view_name    text NOT NULL,
    state        text NOT NULL CHECK (state IN ('computed', 'failed')),
    detail       text,
    at           timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (view_catalog, view_schema, view_name),
    CONSTRAINT view_lineage_state_failed_says_why CHECK (state <> 'failed' OR detail IS NOT NULL)
);

-- `egret-lineage` reads every governed table (spec R5), and an engine JWT carries
-- `users.username` as `preferred_username`. A human whose username were
-- `egret-lineage` would therefore hold the principal's grants. `ensure_username`
-- skips the name (egret_core::engine_principals::RESERVED_ENGINE_USERNAMES); this
-- makes it unrepresentable. A row already holding it is cleared first, so the
-- next `ensure_username` call derives a different name: that user never had a
-- right to the principal's identity. `username` is citext, so the comparison is
-- case-insensitive.
UPDATE users SET username = NULL WHERE username = 'egret-lineage';
ALTER TABLE users ADD CONSTRAINT users_username_not_reserved
    CHECK (username IS NULL OR username <> 'egret-lineage');
