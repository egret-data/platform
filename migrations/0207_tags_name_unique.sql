-- Classification names are unique, ignoring case (classification UX spec
-- §12.2, user decision 2026-10-23: a unique index, and existing duplicates
-- renamed).
--
-- WHY. Rules, clearances and the unreadable-table guard select columns by
-- `tags.key`, but the compiled Trino tag policies and the tag map the Ranger
-- plugin downloads carry `tags.name`, and the engine matches on the name. The
-- two agree only while key and name are one-to-one. Two classifications with
-- one name share every rule written on either: a mask on one masks the other's
-- columns too. The `tag_name_taken` handler check (C10) stops new duplicates
-- but races, and duplicates stored before it remain. With the index every
-- site that selects by key selects what the engine matches.
--
-- EXISTING DUPLICATES. In each group whose `lower(name)` collides, the oldest
-- classification (by `created_at`, then `id`) keeps its name. Each other one
-- is renamed to `<name> (<key>)`, with ` 2`, ` 3`, … appended if a
-- classification already holds that name. This changes which data the rules
-- reach: a rule or hand-written tag policy naming the shared name now applies
-- only to the columns of the classification that kept it. That is the point,
-- and it is why each rename is logged.
--
-- AUDIT. Each rename is one `kind = 'mutation'` row in `audit.audit_log`
-- (`tag:rename` on `tag`, no actor: the system made it), with the before and
-- after names. It is not a governance-feed event: the feed lists only the
-- pairs in `egret_audit::governance::FAMILIES`, and a classification rename
-- made through the API is not one of them either.
--
-- RECOMPILE. A compiled tag policy carries the name as it stood at compile
-- time, so after a rename it is stale. SQL cannot recompile, so each renamed
-- classification is queued in `tag_rename_recompiles`, and the server's boot
-- pass (`egret_bi::TagRepo::recompile_queued_renames`) runs the API rename's
-- own recompile for them: bump the `tag_version` of every service serving a
-- warehouse that carries the tag, recompile every profile the name reaches,
-- then delete the queue rows, in one transaction. A failed pass keeps the rows
-- and runs again at the next boot.
--
-- The `tag_version` bump is deliberately NOT made here. Until the boot pass
-- commits, plugins keep answering 304 and serve the old tag map, which names
-- the same old names as the stored policies, so the two still agree. Bumping
-- here would make every plugin fetch the new names while the policies still
-- name the old ones: the mask stops applying until the recompile lands.

CREATE TABLE tag_rename_recompiles (
    tag_id    uuid PRIMARY KEY REFERENCES tags(id) ON DELETE CASCADE,
    old_name  text NOT NULL,
    queued_at timestamptz NOT NULL DEFAULT now()
);

DO $$
DECLARE
    dup       record;
    candidate text;
    n         int;
BEGIN
    FOR dup IN
        SELECT id, key, name
          FROM (SELECT id, key, name, created_at,
                       row_number() OVER (PARTITION BY lower(name)
                                          ORDER BY created_at, id) AS rank
                  FROM tags) ranked
         WHERE rank > 1
         ORDER BY created_at, id
    LOOP
        candidate := dup.name || ' (' || dup.key || ')';
        n := 1;
        WHILE EXISTS (SELECT 1 FROM tags WHERE lower(name) = lower(candidate)) LOOP
            n := n + 1;
            candidate := dup.name || ' (' || dup.key || ') ' || n;
        END LOOP;

        UPDATE tags SET name = candidate WHERE id = dup.id;
        INSERT INTO tag_rename_recompiles (tag_id, old_name) VALUES (dup.id, dup.name);
        INSERT INTO audit.audit_log (kind, action, resource_type, resource_id, detail)
        VALUES ('mutation', 'tag:rename', 'tag', dup.id::text,
                jsonb_build_object(
                    'before', jsonb_build_object('key', dup.key, 'name', dup.name),
                    'after', jsonb_build_object('key', dup.key, 'name', candidate),
                    'reason', 'migration 0207: another classification has this name'));
    END LOOP;
END
$$;

CREATE UNIQUE INDEX tags_name_lower_key ON tags (lower(name));
