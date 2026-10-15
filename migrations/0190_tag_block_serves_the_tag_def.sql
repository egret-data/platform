-- 0190_tag_block_serves_the_tag_def.sql
--
-- Make every engine re-download a policy set whose `tagPolicies` block now
-- carries a different service-def.
--
-- THE DEFECT. `ranger/api/download.rs` built the `tagPolicies` block with the
-- service-def of the TAG SERVICE's own `type`. The admin API creates profiles
-- of type `trino` or `starrocks` only, so a tag service made through it is a
-- `trino` profile, and the block went out carrying `trino.json`. That def has
-- no `tag` resource, so the Ranger plugin builds no `tag` trie, its candidate
-- filter stops narrowing, and EVERY tag policy is applied to EVERY tagged
-- column (measured live on Trino 483 / plugin 2.5.0, and against the jar in
-- `tests/ranger/src/test/java/EgretTagBlockServiceDefTest.java`). A deny on
-- one tag denied columns carrying only another; a mask on one tag masked
-- them. The block now always carries the `tag` def, as real Ranger Admin does.
--
-- WHY A BUMP. The plugin caches its download and asks with
-- `lastKnownVersion`; the route answers 304 while that equals the profile's
-- `policy_version`. The fix changes the bytes served without changing any
-- version, so without this an engine keeps enforcing the cached, wrong block
-- until something unrelated bumps its profile.
--
-- WHAT IT TOUCHES. Every profile that names a `tagService` — the only
-- downloads that carry the block. Those whose tag service is already of type
-- `tag` get an unchanged body under a new version, which costs one
-- re-download and is otherwise harmless (the plugin compares for equality and
-- the version only grows). Migration 0176's CHECK allows `tagService` only on
-- `trino` profiles, so 0182's trigger (`WHEN NEW.type = 'tag'`) does not fire
-- for any row updated here.

UPDATE policy.authz_services
   SET policy_version = policy_version + 1,
       updated_at     = now()
 WHERE config_jsonb ? 'tagService'
   AND config_jsonb -> 'tagService' <> 'null'::jsonb;
