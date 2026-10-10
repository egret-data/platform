-- Data Apps P2 (docs/en/specs/2026-09-30-marimo-data-apps-design.md §5): one revisioned
-- draft per app, immutable published versions, and the FKs P1 deferred.

CREATE TABLE analytics.app_drafts (
    app_id     uuid PRIMARY KEY REFERENCES analytics.apps(id) ON DELETE CASCADE,
    source     text NOT NULL CHECK (octet_length(source) <= 1048576),
    revision   bigint NOT NULL DEFAULT 1 CHECK (revision >= 1),
    updated_by uuid NULL REFERENCES users(id) ON DELETE SET NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE analytics.app_versions (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    app_id        uuid NOT NULL REFERENCES analytics.apps(id) ON DELETE CASCADE,
    version_no    integer NOT NULL CHECK (version_no >= 1),
    source        text NOT NULL CHECK (octet_length(source) <= 1048576),
    -- the draft revision this version was published from; NULL for a restore
    from_revision bigint NULL,
    -- for a restore: the version whose source was republished
    restored_from uuid NULL REFERENCES analytics.app_versions(id),
    published_by  uuid NULL REFERENCES users(id) ON DELETE SET NULL,
    published_at  timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT app_versions_app_no_key UNIQUE (app_id, version_no),
    -- target of the composite FK from app_sessions: a pinned version belongs to that app
    CONSTRAINT app_versions_id_app_key UNIQUE (id, app_id)
);
CREATE INDEX app_versions_app_no_desc_idx ON analytics.app_versions (app_id, version_no DESC);

-- Append-only (spec D5). UPDATE is refused except for one shape: the
-- `published_by ... ON DELETE SET NULL` action, which Postgres runs as an UPDATE when
-- the publisher's user row is deleted. Without that exception no user who ever
-- published a version could be deleted. The allowance is exact: `published_by` goes
-- from non-NULL to NULL and every other column is unchanged.
-- DELETE is refused unless the parent app row is already gone, i.e. the delete is the
-- ON DELETE CASCADE from analytics.apps (itself reached from a workspace delete);
-- without that exception a workspace with published apps could never be deleted.
CREATE FUNCTION analytics.app_versions_append_only() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'UPDATE' THEN
        IF OLD.published_by IS NOT NULL AND NEW.published_by IS NULL
           AND (to_jsonb(NEW) - 'published_by') = (to_jsonb(OLD) - 'published_by') THEN
            RETURN NEW;
        END IF;
        RAISE EXCEPTION 'analytics.app_versions is append-only (version %)', OLD.id
            USING ERRCODE = 'restrict_violation';
    END IF;
    IF EXISTS (SELECT 1 FROM analytics.apps WHERE id = OLD.app_id) THEN
        RAISE EXCEPTION 'analytics.app_versions is append-only (version %)', OLD.id
            USING ERRCODE = 'restrict_violation';
    END IF;
    RETURN OLD;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER app_versions_append_only_trg
    BEFORE UPDATE OR DELETE ON analytics.app_versions
    FOR EACH ROW EXECUTE FUNCTION analytics.app_versions_append_only();

-- P1 deferred: folders (cascade-safe — a deleted folder leaves the app Uncategorized).
ALTER TABLE analytics.apps
    ADD CONSTRAINT apps_folder_fk FOREIGN KEY (folder_id)
        REFERENCES analytics.folders(id) ON DELETE SET NULL;

-- P1 deferred: a run session pins a version OF ITS OWN APP. Edit sessions have
-- version_id NULL, which MATCH SIMPLE leaves unchecked — as intended.
-- No run token can be valid before analytics.app_versions exists (P1 had no path that
-- minted one), so deleting any version-pinned session loses nothing and guarantees the
-- FK below cannot fail on deploy against a stray row.
DELETE FROM analytics.app_sessions WHERE version_id IS NOT NULL;
ALTER TABLE analytics.app_sessions
    ADD CONSTRAINT app_sessions_version_app_fk FOREIGN KEY (version_id, app_id)
        REFERENCES analytics.app_versions(id, app_id) ON DELETE CASCADE;
