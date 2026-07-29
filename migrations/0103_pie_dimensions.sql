-- Pie's `dimension` (singular) becomes `dimensions` (an array of up to three), enabling nested-ring
-- pies. Task 1 already changed the renderer to read `pie.dimensions` and stopped reading
-- `pie.dimension`, so every already-saved pie -- which still carries the old singular key --
-- renders blank right now: the backend still returns rows for the old dimension, so the
-- empty-state guard never fires; the chart just draws nothing. This migration is what un-breaks
-- those charts, not a tidy-up.
--
-- The conversion is a straight lift: `pie.dimension` (a string) -> `pie.dimensions: [<that
-- string>]`, old key removed. A pie with no stored dimension has nothing to lift and is left
-- alone.
--
-- Same two structural points as migrations 0101/0102: `-` binds tighter than `->`, so the
-- sub-object (`settings` / `vizOverride`) is extracted into a plain alias inside a LATERAL before
-- `-` is used on it; and a non-object value there (string, array, JSON null) is normalised to
-- `'{}'::jsonb` inside that same LATERAL, not guarded in the WHERE clause with a
-- `jsonb_typeof(...) = 'object' AND ...` check, since Postgres does not promise left-to-right AND
-- evaluation.
--
-- The gate is on the VALUE's type, not on key existence: `jsonb_typeof(s->'pie.dimension') =
-- 'string'`. A bare `s ? 'pie.dimension'` would also match a JSON `null` stored under that key
-- (still "present"), and building `jsonb_build_array(s->>'pie.dimension')` from that would write
-- `pie.dimensions: [null]` -- a fabricated single-null-ring pie that never existed before. Gating
-- on the type instead leaves that row -- and equally a number, array, or object under the old key,
-- shapes the API does not itself reject (chartparams.rs validates that `settings` is an object,
-- not the values of individual keys within it) -- completely untouched, old key included: whatever
-- downstream code eventually needs to do with a malformed `pie.dimension` is a decision this
-- migration declines to make on its behalf.
--
-- Unlike 0101/0102, there is no `::numeric` (or any other) cast here: the only extraction is
-- `s->>'pie.dimension'`, taken solely on the branch already gated to JSON type `string`, so it can
-- never receive a non-string value and cannot throw. The single-malformed-row-aborts-the-whole-
-- migration failure mode those two migrations guarded against with COALESCE/CASE therefore has no
-- surface to guard against here -- checked, and found not to apply.
--
-- `vizOverride` (migration 0100) is a PARTIAL override: `pie.dimensions` is written into it only
-- when the override itself carried a (string) `pie.dimension` -- writing it unconditionally would
-- shadow the base chart's own dimensions for every card whose override never touched pie settings
-- at all.
--
-- No `viz_type = 'pie'` filter: harmless either way, since `pie.*` keys are namespaced and sit
-- dormant on any renderer that isn't reading them. Not filtering also means a chart later switched
-- back to viz_type pie finds its dimension already converted rather than reverting to the broken
-- singular shape.

UPDATE analytics.charts c
SET params = jsonb_set(c.params, '{settings}', migrated.new_settings)
FROM (
  SELECT
    ch.id,
    (s - 'pie.dimension') || jsonb_build_object('pie.dimensions', jsonb_build_array(s->>'pie.dimension')) AS new_settings
  FROM analytics.charts ch, LATERAL (
    -- Normalised here rather than guarded in the WHERE -- see the header comment.
    SELECT CASE WHEN jsonb_typeof(ch.params->'settings') = 'object'
                THEN ch.params->'settings' ELSE '{}'::jsonb END AS s
  ) x
  -- Gate on the value's type, not key existence, so a `pie.dimension: null` (or any non-string
  -- value) row is left fully untouched -- see the header comment.
  WHERE jsonb_typeof(s->'pie.dimension') = 'string'
) AS migrated
WHERE c.id = migrated.id;

UPDATE analytics.dashboard_cards dc
SET options = jsonb_set(dc.options, '{vizOverride}', migrated.new_override)
FROM (
  SELECT
    card.id,
    (o - 'pie.dimension') || jsonb_build_object('pie.dimensions', jsonb_build_array(o->>'pie.dimension')) AS new_override
  FROM analytics.dashboard_cards card, LATERAL (
    -- Normalised here rather than guarded in the WHERE, for the same reason as the charts
    -- statement above.
    SELECT CASE WHEN jsonb_typeof(card.options->'vizOverride') = 'object'
                THEN card.options->'vizOverride' ELSE '{}'::jsonb END AS o
  ) x
  WHERE jsonb_typeof(o->'pie.dimension') = 'string'
) AS migrated
WHERE dc.id = migrated.id;
