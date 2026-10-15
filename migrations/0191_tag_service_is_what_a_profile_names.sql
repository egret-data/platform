-- 0191_tag_service_is_what_a_profile_names.sql
--
-- A tag service is whatever some profile's `config_jsonb ->> 'tagService'`
-- names — not only a row of `type = 'tag'`. Live finding I5.
--
-- THE DEFECT. 0182's trigger cascades a tag service's version bump to the
-- profiles that download its policies as `tagPolicies`, but it fires only
-- `WHEN NEW.type = 'tag'`. The admin API creates profiles of type `trino` or
-- `starrocks` only, so a tag service made through it is a `trino` profile and
-- the trigger never fired for it: inserting, updating or deleting one of its
-- policies bumped the tag service alone, every linking profile kept its
-- version, its plugin was told 304 Not Modified, and it kept enforcing the old
-- block — measured live as a DELETED tag mask still being served.
--
-- THE RULE NOW. Any bump of any profile's `policy_version` carries to the
-- other profiles that name it as `tagService`. A `tag`-typed service behaves
-- exactly as under 0182; a `trino`-typed one now behaves the same. A profile
-- nobody names finds no linker and the UPDATE is a no-op, so dropping the
-- `type` predicate changes nothing for it. (A subquery cannot sit in a trigger's
-- WHEN clause, which is why the "is it named" test is the UPDATE's own WHERE.)
--
-- ONE HOP, AS THE DOWNLOAD EMBEDS ONE LEVEL. Every row this updates is a
-- `trino` profile (0176's CHECK), and without the `type` guard its own bump
-- fires this trigger again; were it in turn named by a third profile the
-- cascade would walk on, and a cycle (A names B, B names A) would never stop.
-- A download embeds only the tag service its profile names directly, so only
-- the direct linkers need the bump.
--
-- THE GUARD IS "INSIDE MY OWN CASCADE", NOT TRIGGER DEPTH. The transaction-
-- local setting `egret.tag_cascade` is set to 'on' around the cascade UPDATE,
-- and a firing that sees it 'on' returns at once: that is exactly the re-entry
-- the cascade causes, and nothing else. A depth test (`pg_trigger_depth() > 1`,
-- this migration's first draft) would also drop every cascade whose ORIGINATING
-- bump runs inside some other trigger — and such triggers exist:
-- `feat/view-governance`'s 0187 (`engines_binding_bumps_profiles`) bumps
-- profiles from a trigger. A dropped cascade fails silently: the linker's
-- plugin gets 304 and keeps the old tag block. `set_config(..., true)` is
-- local to the transaction, so an error mid-cascade aborts the transaction and
-- takes the flag with it; on success it is reset to 'off' before returning,
-- so a later, unrelated bump in the same transaction cascades normally.
--
-- 0182 is not edited (a migration's text is append-only); this replaces its
-- function and its trigger.

CREATE OR REPLACE FUNCTION policy.bump_tag_service_linkers() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('egret.tag_cascade', true) = 'on' THEN
        RETURN NULL;
    END IF;
    PERFORM set_config('egret.tag_cascade', 'on', true);
    UPDATE policy.authz_services
       SET policy_version = policy_version + 1,
           updated_at     = now()
     WHERE config_jsonb ->> 'tagService' = NEW.name
       AND id <> NEW.id;
    PERFORM set_config('egret.tag_cascade', 'off', true);
    RETURN NULL;
END
$$;

CREATE OR REPLACE TRIGGER authz_services_tag_bump_reaches_linkers
    AFTER UPDATE OF policy_version ON policy.authz_services
    FOR EACH ROW
    WHEN (NEW.policy_version > OLD.policy_version)
    EXECUTE FUNCTION policy.bump_tag_service_linkers();
