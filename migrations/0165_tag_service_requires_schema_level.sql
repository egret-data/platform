-- 0165_tag_service_requires_schema_level.sql
-- SP4d debt Task 2. A policy profile may carry a `tagService` only when its
-- service-def declares a `schema` resource — which, of the three seeded defs, is
-- `trino` alone.
--
-- THIS MIGRATION REMOVES CONFIGURATION. Any `policy.authz_services` row whose
-- `type` is not `trino` and whose `config_jsonb` carries a `tagService` has that
-- ONE key deleted (nothing else in the config is touched, and no row is
-- deleted). Egret has never written such a row — `bind_trino_profile` is the
-- only producer of a `tagService` value and it writes it only onto a `trino`
-- profile — so this clears hand-authored configuration only.
--
-- WHY REMOVING IT IS SAFE, and it is the same measurement the constraint rests
-- on: a non-Trino profile's `tagService` CANNOT ENFORCE ANYTHING.
-- `ServiceTags::from_assignments` (egret-policy-api) hard-codes the
-- `resourceElements` keys `catalog`/`schema`/`table`/`column` and `download_tags`
-- calls it with no engine argument at all, while `starrocks.json`'s middle level
-- is `database`, not `schema`. Ranger's own
-- `RangerTagEnricher.processServiceTags` therefore DROPS every attachment such a
-- profile downloads (`createRangerServiceResourceMatcher` returns null when no
-- hierarchy in the resource def contains every key), so every tag policy in its
-- `tagPolicies` block is already SILENTLY inert: no masking, no row filtering,
-- no error, nothing in a log. The value is dead on arrival, and clearing the key
-- is exactly the remedy an operator would be told to apply.
--
-- WHY VALID RATHER THAN `NOT VALID`. `NOT VALID` would leave a pre-existing bad
-- row unscanned and dormant — but dormant is not harmless, because the row is
-- reachable by a BULK statement that does not filter on engine type.
-- `egret_catalog_registry::tag_version::bump_bound_tag_versions` resolves every
-- profile bound to any engine on any catalog in a warehouse (its CTE filters on
-- `e.authz_service_id IS NOT NULL` only) and bumps them in ONE
-- `UPDATE … WHERE id = ANY($1)`. Postgres enforces a `NOT VALID` CHECK on every
-- UPDATE, so a warehouse holding a healthy Trino engine beside a StarRocks
-- engine whose profile was such a row means: writing a catalog tag makes that
-- single statement violate the constraint, the whole tag write fails, AND the
-- healthy Trino profile's `tag_version` is not bumped either — all-or-nothing.
-- `egret_perm_compile::trigger`'s own bulk bump has the same shape. A dormant
-- dead key becoming an outage on an unrelated healthy profile's write is
-- strictly worse than deleting the key, so the key goes and the constraint is
-- validated.
--
-- THE PREDICATE. `config_jsonb ->> 'tagService' IS NULL` is true both when the
-- key is absent and when its value is JSON `null`, and `->>` on a non-object
-- `config_jsonb` yields NULL rather than erroring. `carries_tag_service` in
-- `egret-policy/src/store.rs` is written to this exact shape, deliberately: two
-- guards that must agree, written to different shapes, is the defect this
-- programme has produced four times. A Rust refusal keyed on `as_str()` beside a
-- CHECK keyed on `->>` would let `{"tagService": 7}` pass the route and surface
-- the constraint as a 500. The UPDATE below is keyed on the same `->>`
-- expression as the CHECK, so it cannot leave behind a row the CHECK then
-- rejects.
--
-- `PolicyStore::create_service` refuses the INSERT direction too, and it is the
-- only `INSERT INTO policy.authz_services` in any `src/` — but it is not
-- sufficient on its own: NO `UPDATE` of `type` or of `config_jsonb` exists
-- anywhere in `src/`, so "flip a tagService-carrying profile to starrocks" has
-- no Rust writer to refuse at (measured, not assumed; see the enumeration on
-- `refuse_tag_service_on_non_trino_profile`). This CHECK is that direction's only
-- bound, and it holds for every INSERT and every UPDATE, from Rust, from a route
-- added later, from `psql`, and from a test's raw `sqlx::query`.
--
-- WHEN THE MAPPING LANDS. Teaching everything that emits Ranger resource keys to
-- take the engine (or its service-def) as an input is separate work: block now,
-- map later. That change replaces this constraint and the Rust refusal together.
UPDATE policy.authz_services
   SET config_jsonb = config_jsonb - 'tagService'
 WHERE type <> 'trino'
   AND config_jsonb ->> 'tagService' IS NOT NULL;

ALTER TABLE policy.authz_services
    ADD CONSTRAINT authz_services_tag_service_needs_schema_level
    CHECK (config_jsonb ->> 'tagService' IS NULL OR type = 'trino');
