ALTER TABLE users ADD COLUMN username citext UNIQUE;

-- Backfill existing rows: slug of the email local-part ([a-z0-9_-], lowercased),
-- de-duplicated by appending "-N" until globally unique. A PL/pgSQL loop assigns
-- usernames one at a time, checking each candidate against ALL current usernames
-- (mirrors the Rust ensure_username logic). A set-based CTE would be unsafe here:
-- partitioning per base slug independently can mint the same "base-2" that a user
-- whose local-part literally equals "base-2" also claims, aborting the migration.
DO $$
DECLARE r RECORD; base text; cand text; n int;
BEGIN
  FOR r IN
    SELECT id,
           COALESCE(NULLIF(regexp_replace(lower(split_part(email::text,'@',1)), '[^a-z0-9_-]+', '-', 'g'), ''), 'user') AS base
    FROM users WHERE username IS NULL
    ORDER BY created_at, id
  LOOP
    n := 1; cand := r.base;
    WHILE EXISTS (SELECT 1 FROM users WHERE username = cand) LOOP
      n := n + 1; cand := r.base || '-' || n;
    END LOOP;
    UPDATE users SET username = cand WHERE id = r.id;
  END LOOP;
END $$;
