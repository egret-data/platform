-- Username takeover (spec docs/en/specs/2026-10-23-username-takeover-design.md §4.1, decision D1):
-- a username is never held by two people over time.
--
-- egret names users by USERNAME in places that outlive the user: `user` refs in
-- the three perm.* principal tables, hand-written `users[]` in Ranger policies,
-- compiled bodies, and the `egret-admin - …` infra policies. A user delete frees
-- `users.username`, and `ensure_username` mints the lowest free `base`, `base-2`,
-- … for the next person whose email slugs to the same base: that person would
-- inherit every allow, deny exemption and mask exemption still naming the name.
--
-- `username_ledger` holds every name ever minted, backfilled or reserved, and is
-- never shrunk. A name enters it when a `users` row first takes it, and leaves
-- its holder (user_id NULL, retired_at set) when the row is deleted or renamed;
-- the ledger row itself stays, so the name can never be minted again.

CREATE TABLE username_ledger (
    username   citext PRIMARY KEY,
    -- The current holder; NULL once retired.
    user_id    uuid NULL REFERENCES users(id) ON DELETE SET NULL,
    -- The user it was minted for. No FK: it survives the delete, so the ledger
    -- names who held the name after audit retention has pruned every event.
    minted_for uuid NULL,
    -- NULL = held before 0209, when the mint time was not recorded.
    minted_at  timestamptz NULL,
    retired_at timestamptz NULL,
    source     text NOT NULL CHECK (source IN
        ('mint', 'backfill_user', 'backfill_ref', 'backfill_policy', 'backfill_audit', 'reserved'))
);
CREATE INDEX username_ledger_user_id_idx ON username_ledger (user_id) WHERE user_id IS NOT NULL;

-- Why triggers on `users`, not Rust code: the trigger runs inside the statement
-- that writes `users.username`, so it covers every writer (ensure_username_via,
-- migrations, tests that INSERT a username, an operator's psql), and it has no
-- check-then-insert race. A "SELECT from a retired table, then UPDATE" design
-- races under READ COMMITTED with a concurrent delete; here the ledger row for
-- `bob` was committed when `bob` was minted, a delete only clears its user_id,
-- and a mint of `bob` blocks on the `users` unique index until the delete
-- commits, then fails on `username_ledger_pkey` with 23505.
-- `ensure_username_via` already treats any 23505 inside its per-candidate
-- SAVEPOINT as "taken" and tries the next suffix, so it needs no change.
CREATE FUNCTION username_ledger_record() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    -- A rename or a NULLing (0187's shape) retires the old name like a delete.
    IF TG_OP = 'UPDATE' THEN
        IF OLD.username IS NOT NULL THEN
            UPDATE username_ledger
               SET user_id = NULL, retired_at = now()
             WHERE username = OLD.username AND user_id = OLD.id;
        END IF;
    END IF;
    IF NEW.username IS NULL THEN
        RETURN NULL;
    END IF;
    -- The holder re-setting their own name (a case-only rewrite reaches here
    -- only through `::text` comparisons; citext treats it as unchanged).
    IF EXISTS (SELECT 1 FROM username_ledger
                WHERE username = NEW.username AND user_id = NEW.id) THEN
        RETURN NULL;
    END IF;
    -- Raises 23505 on username_ledger_pkey when the name was ever held,
    -- backfilled or reserved.
    INSERT INTO username_ledger (username, user_id, minted_for, minted_at, source)
    VALUES (NEW.username, NEW.id, NEW.id, now(), 'mint');
    RETURN NULL;
END;
$$;

CREATE TRIGGER users_username_ledger_insert
    AFTER INSERT ON users
    FOR EACH ROW WHEN (NEW.username IS NOT NULL)
    EXECUTE FUNCTION username_ledger_record();

CREATE TRIGGER users_username_ledger_update
    AFTER UPDATE OF username ON users
    FOR EACH ROW WHEN (NEW.username IS DISTINCT FROM OLD.username)
    EXECUTE FUNCTION username_ledger_record();

-- Retirement on delete. The FK's ON DELETE SET NULL then clears user_id;
-- minted_for keeps the record of who held the name.
CREATE FUNCTION username_ledger_retire() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE username_ledger SET retired_at = now()
     WHERE user_id = OLD.id AND retired_at IS NULL;
    RETURN OLD;
END;
$$;

CREATE TRIGGER users_username_ledger_retire
    BEFORE DELETE ON users
    FOR EACH ROW EXECUTE FUNCTION username_ledger_retire();

-- Append-only. No API frees a name; an operator who must free one drops this
-- trigger in a manual migration, on purpose. Only the two retirement moves are
-- allowed: user_id to NULL (the FK action, or a rename) and retired_at from
-- NULL to a time. Setting user_id to another user would hand them the name (the
-- record trigger would then take it as their own), so it is refused as well.
CREATE FUNCTION username_ledger_append_only() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'DELETE' OR TG_OP = 'TRUNCATE' THEN
        RAISE EXCEPTION 'username_ledger is append-only: a retired username is never freed'
            USING ERRCODE = 'P0001';
    END IF;
    IF NEW.username::text IS DISTINCT FROM OLD.username::text
       OR NEW.minted_for IS DISTINCT FROM OLD.minted_for
       OR NEW.minted_at IS DISTINCT FROM OLD.minted_at
       OR NEW.source IS DISTINCT FROM OLD.source
       OR (NEW.user_id IS NOT NULL AND NEW.user_id IS DISTINCT FROM OLD.user_id)
       OR (OLD.retired_at IS NOT NULL AND NEW.retired_at IS DISTINCT FROM OLD.retired_at) THEN
        RAISE EXCEPTION 'username_ledger is append-only: only retirement may change a row'
            USING ERRCODE = 'P0001';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER username_ledger_append_only
    BEFORE UPDATE OR DELETE ON username_ledger
    FOR EACH ROW EXECUTE FUNCTION username_ledger_append_only();

-- A TRUNCATE (users … CASCADE included) fires no row trigger.
CREATE TRIGGER username_ledger_no_truncate
    BEFORE TRUNCATE ON username_ledger
    FOR EACH STATEMENT EXECUTE FUNCTION username_ledger_append_only();

-- The base slug `egret_auth::users::slugify_localpart` derives from an email:
-- lowercase the local part, replace each run outside [a-z0-9_] with one `-`,
-- trim `-`, and use `user` when nothing is left. A test compares the two over a
-- corpus. `lower` follows the database collation, which may not lowercase every
-- non-ASCII letter the way Rust does; every non-ASCII character becomes `-`
-- either way, and the one caller (service-account emails, built from an ASCII
-- slug) never has one.
CREATE FUNCTION username_slug(email text) RETURNS text
LANGUAGE sql IMMUTABLE AS $$
    SELECT COALESCE(
        NULLIF(btrim(regexp_replace(lower(split_part(email, '@', 1)), '[^a-z0-9_]+', '-', 'g'), '-'), ''),
        'user')
$$;

-- Every name a Ranger policy body lists in `users[]`, in any item list, except
-- `{USER}`-style macros.
CREATE FUNCTION policy_body_usernames(body jsonb) RETURNS SETOF text
LANGUAGE sql IMMUTABLE AS $$
    SELECT DISTINCT u.name
      FROM unnest(ARRAY['policyItems', 'denyPolicyItems', 'allowExceptions', 'denyExceptions',
                        'dataMaskPolicyItems', 'rowFilterPolicyItems']) AS k(list),
           LATERAL jsonb_array_elements(CASE WHEN jsonb_typeof(body -> k.list) = 'array'
                                             THEN body -> k.list ELSE '[]'::jsonb END) AS item,
           LATERAL jsonb_array_elements_text(CASE WHEN jsonb_typeof(item -> 'users') = 'array'
                                                  THEN item -> 'users' ELSE '[]'::jsonb END) AS u(name)
     WHERE u.name <> '' AND u.name !~ '^\{.*\}$'
$$;

-- The seed: every name that is knowable, so no mint after 0209 hands out a name
-- a stale ref, body or event still names. Each step is ON CONFLICT DO NOTHING,
-- so the order decides the source of a name found twice (a held name stays
-- `backfill_user`). Kept callable, so a test can insert pre-0209 shapes and run
-- it again.
CREATE FUNCTION username_ledger_backfill() RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    -- 1. Every held username. minted_at NULL: the mint time was never recorded.
    INSERT INTO username_ledger (username, user_id, minted_for, minted_at, source)
    SELECT username, id, id, NULL, 'backfill_user' FROM users WHERE username IS NOT NULL
    ON CONFLICT DO NOTHING;

    -- 2. Every `user` ref in the three perm.* principal tables. A held one
    --    conflicted in step 1; the rest name a deleted user (or a name written
    --    before anyone held it) and are retired.
    INSERT INTO username_ledger (username, source, retired_at)
    SELECT ref, 'backfill_ref', now() FROM (
        SELECT ref FROM perm.grant_principals WHERE kind = 'user'
        UNION SELECT ref FROM perm.rule_exemptions WHERE kind = 'user'
        UNION SELECT ref FROM perm.engine_grant_principals WHERE kind = 'user'
    ) r
    WHERE ref <> ''
    ON CONFLICT DO NOTHING;

    -- 3. Every `users[]` name of ANY policy body: hand-written (`operator`), but
    --    also `compiler` and `infra`. A stale compiled body or a stale
    --    `egret-admin` body names a freed name exactly as dangerously as a
    --    hand-written one, and `egret-admin` (engine ALL access) is the worst
    --    case. It also retires `egret_delegator` wherever an impersonate engine
    --    exists (step 5 does it everywhere).
    INSERT INTO username_ledger (username, source, retired_at)
    SELECT DISTINCT n, 'backfill_policy', now()
      FROM policy.authz_policies p, LATERAL policy_body_usernames(p.policy_jsonb) AS n
    ON CONFLICT DO NOTHING;

    -- 4a. Deleted service accounts. Their `service_account:delete` event keeps
    --     the email; its base slug is retired (the suffix the account had is
    --     unknowable, so only the base).
    INSERT INTO username_ledger (username, source, retired_at)
    SELECT DISTINCT username_slug(detail ->> 'email'), 'backfill_audit', now()
      FROM audit.audit_log
     WHERE action = 'service_account:delete' AND detail ? 'email'
    ON CONFLICT DO NOTHING;

    -- 4b. Every user named by a governance event of a grant, rule or engine
    --     grant that is still in retention: `"user:<name>"` strings (grant and
    --     rule summaries) and `{"kind": "user", "ref": <name>}` objects (engine
    --     grant summaries).
    INSERT INTO username_ledger (username, source, retired_at)
    SELECT DISTINCT n, 'backfill_audit', now() FROM (
        SELECT substr(s #>> '{}', 6) AS n
          FROM audit.audit_log a,
               LATERAL jsonb_path_query(a.detail,
                   'strict $.** ? (@.type() == "string" && @ starts with "user:")') AS s
         WHERE a.kind = 'mutation' AND a.resource_type IN ('perm_grant', 'perm_rule', 'engine_grant')
        UNION
        SELECT s #>> '{}'
          FROM audit.audit_log a,
               LATERAL jsonb_path_query(a.detail,
                   'strict $.** ? (@.type() == "object" && @.kind == "user").ref') AS s
         WHERE a.kind = 'mutation' AND a.resource_type IN ('perm_grant', 'perm_rule', 'engine_grant')
    ) e
    WHERE n <> ''
    ON CONFLICT DO NOTHING;

    -- 5. The principals egret runs as on an engine. `egret-lineage` is also
    --    refused by 0187's CHECK; `egret_delegator` was mintable until now
    --    (engine-grants spec E21).
    INSERT INTO username_ledger (username, source, retired_at)
    VALUES ('egret-lineage', 'reserved', now()), ('egret_delegator', 'reserved', now())
    ON CONFLICT DO NOTHING;
END;
$$;

SELECT username_ledger_backfill();

-- The evidence the username-reuse report (spec §4.5) needs and that the next
-- boot would destroy: the `egret-admin` sync and every recompile rewrite exactly
-- the bodies that show a past takeover. Snapshotted once, here, before any of
-- that runs. Only rows that are evidence are kept: a name with no holder, a body
-- last written before its holder existed, or an `egret-admin` body naming
-- someone who is not an active platform admin. After 0209 every rewrite is
-- correct by construction, so no later snapshot is taken. No FKs: the evidence
-- outlives the service, policy and user.
CREATE TABLE username_reuse_baseline (
    service_id             bigint NOT NULL,
    policy_id              bigint NOT NULL,
    policy_name            text NOT NULL,
    managed_by             text NOT NULL,
    policy_updated_at      timestamptz NOT NULL,
    username               citext NOT NULL,
    holder_id              uuid NULL,
    holder_created_at      timestamptz NULL,
    holder_is_active_admin boolean NOT NULL,
    taken_at               timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (policy_id, username)
);

-- The active platform admins: `RoleRepo::platform_admin_user_ids` (direct and
-- group arms) plus `status = 'active'`, as `engine_admin_user_ids_via` reads it.
INSERT INTO username_reuse_baseline
    (service_id, policy_id, policy_name, managed_by, policy_updated_at, username,
     holder_id, holder_created_at, holder_is_active_admin)
SELECT * FROM (
    SELECT p.service_id, p.id, p.name, p.managed_by, p.updated_at, n.name::citext,
           u.id, u.created_at,
           COALESCE(u.status = 'active' AND u.id IN (
               SELECT g.principal_id FROM access_grants g JOIN roles r ON r.id = g.role_id
                WHERE r.name = 'platform_admin' AND g.scope_type = 'platform'
                  AND g.principal_type = 'user'
               UNION
               SELECT ug.user_id FROM access_grants g JOIN roles r ON r.id = g.role_id
                 JOIN user_groups ug ON ug.group_id = g.principal_id
                WHERE r.name = 'platform_admin' AND g.scope_type = 'platform'
                  AND g.principal_type = 'group'), false)
      FROM policy.authz_policies p
           CROSS JOIN LATERAL policy_body_usernames(p.policy_jsonb) AS n(name)
           LEFT JOIN users u ON u.username = n.name::citext
) b(service_id, policy_id, policy_name, managed_by, policy_updated_at, username,
    holder_id, holder_created_at, holder_is_active_admin)
WHERE holder_id IS NULL
   OR policy_updated_at < holder_created_at
   OR (managed_by = 'infra' AND starts_with(policy_name, 'egret-admin - ')
       AND NOT holder_is_active_admin);
