-- 0182_tag_service_bump_reaches_its_linkers.sql
--
-- A change to a tag service's policies must reach every engine that enforces
-- them.
--
-- THE DEFECT. A resource profile's download (`ranger/api/download.rs`) embeds
-- the policies of the tag service its `config_jsonb ->> 'tagService'` names, as
-- the `tagPolicies` block, and answers 304 Not Modified when the plugin's
-- `lastKnownVersion` equals the PROFILE's `policy_version`. Every writer bumps
-- only the service the changed row lives in: the six
-- `SET policy_version = policy_version + 1` statements in
-- `crates/egret-policy/src/store.rs`, and `egret-perm-compile`'s `trigger.rs`
-- (which additionally bumps the one profile it is recompiling). So a write to a
-- tag service's policies bumped the tag service and nothing that downloads it:
-- every other profile linking that tag service kept its version, its engine
-- was told "not modified", and it went on enforcing the OLD tag masks and row
-- filters until something unrelated happened to bump the profile. Measured
-- before this migration: creating a tag policy moved the tag service from
-- version 1 to 2 and left both linking profiles at 1
-- (`crates/egret-policy/tests/tag_service_linkers_bump.rs`).
--
-- WHY A TRIGGER AND NOT ONE MORE `UPDATE` AT EACH WRITER. There are seven
-- writers in two crates, and the rule "a tag-service bump also bumps its
-- linkers" would have to be remembered at every one of them, and at every
-- writer added later. Put in the database, it cannot be forgotten: any bump of
-- a `tag` service's version, from any writer, carries to the profiles that
-- download it.
--
-- WHAT IT TOUCHES. Only rows whose `config_jsonb ->> 'tagService'` equals the
-- tag service's `name` (names are UNIQUE, 0007). Migration 0176's CHECK allows
-- a `tagService` only on a `trino` profile, so every row bumped here is a
-- `trino` profile, the trigger's own `WHEN NEW.type = 'tag'` is false for it,
-- and the cascade cannot recurse. A profile bumped twice in one transaction
-- (the compiler already bumps the profile it recompiles) is harmless: the
-- plugin compares versions for equality, and the version only grows.
--
-- LOCKS. The cascade row-locks the linking profiles inside the writer's
-- transaction, after the writer has locked the tag service's row. Writers that
-- already lock profiles first (the permission compiler's `lock_services_in_tx`
-- expands to tag-service sharers) take the same rows; a writer that locks in the
-- other order can deadlock, which Postgres resolves by aborting one transaction
-- — a retryable error, never a stale engine.

CREATE FUNCTION policy.bump_tag_service_linkers() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE policy.authz_services
       SET policy_version = policy_version + 1,
           updated_at     = now()
     WHERE config_jsonb ->> 'tagService' = NEW.name
       AND id <> NEW.id;
    RETURN NULL;
END
$$;

CREATE TRIGGER authz_services_tag_bump_reaches_linkers
    AFTER UPDATE OF policy_version ON policy.authz_services
    FOR EACH ROW
    WHEN (NEW.type = 'tag' AND NEW.policy_version > OLD.policy_version)
    EXECUTE FUNCTION policy.bump_tag_service_linkers();
