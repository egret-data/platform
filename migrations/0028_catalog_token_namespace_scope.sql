ALTER TABLE catalog_tokens
  ADD COLUMN namespace_scope text[] NOT NULL DEFAULT '{}';
