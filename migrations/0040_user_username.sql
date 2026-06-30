ALTER TABLE users ADD COLUMN username citext UNIQUE;

-- Backfill existing rows: slug of the email local-part ([a-z0-9_-], lowercased),
-- de-duplicated by appending "-N" to collisions (deterministic by created_at).
WITH base AS (
  SELECT id,
         NULLIF(regexp_replace(lower(split_part(email::text,'@',1)), '[^a-z0-9_-]+', '-', 'g'), '') AS slug,
         created_at
  FROM users
), ranked AS (
  SELECT id, COALESCE(slug, 'user') AS slug,
         row_number() OVER (PARTITION BY COALESCE(slug,'user') ORDER BY created_at, id) AS rn
  FROM base
)
UPDATE users u
SET username = CASE WHEN r.rn = 1 THEN r.slug ELSE r.slug || '-' || r.rn END
FROM ranked r WHERE u.id = r.id;
