-- Add per-token permission scope.
-- Existing tokens (e.g. for test/dev engines) default to full read+write access
-- so they continue to work without any data migration.
ALTER TABLE catalog_tokens
  ADD COLUMN permissions text[] NOT NULL DEFAULT '{catalog:read,catalog:write}';
