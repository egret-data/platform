-- Table-level token scope + rotation timestamp.
ALTER TABLE catalog_tokens
    ADD COLUMN table_scope text[] NOT NULL DEFAULT '{}',
    ADD COLUMN rotated_at  timestamptz;
