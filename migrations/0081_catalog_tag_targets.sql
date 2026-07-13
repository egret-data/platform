-- Catalog-resource tag targets (Access P3.2) — the metadata bridge that lets a
-- classification `tag` (from 0080) be attached to a CATALOG table/column, which
-- (unlike a BI object) is NOT uuid-keyed: it is identified by
-- warehouse + schema + table + column. Column '' means a table-level tag.
--
-- METADATA ONLY: no authorization decision / Ranger download reads this table in
-- this phase (tag-based masking policies + the download_tags builder are Task 2).

CREATE TABLE catalog_tag_assignments (
    tag_id       uuid NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    warehouse_id uuid NOT NULL,
    schema_name  text NOT NULL,
    table_name   text NOT NULL,
    column_name  text NOT NULL DEFAULT '',   -- '' = table-level tag
    assigned_by  uuid REFERENCES users(id),
    assigned_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tag_id, warehouse_id, schema_name, table_name, column_name)
);

CREATE INDEX catalog_tag_assignments_res_idx
    ON catalog_tag_assignments (warehouse_id, schema_name, table_name);

-- Monotonic version counter the Task-2 Ranger `download_tags` endpoint will bump
-- on every catalog-tag change so the plugin knows to re-pull. Added now (with the
-- target model) so Task 2 only wires the increment.
ALTER TABLE policy.authz_services ADD COLUMN tag_version bigint NOT NULL DEFAULT 1;
