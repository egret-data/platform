-- Optional free-text description for a catalog role (documents its intent).
ALTER TABLE catalog_roles ADD COLUMN IF NOT EXISTS description text;
