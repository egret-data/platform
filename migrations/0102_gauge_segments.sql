-- Gauge `min`/`max` become coloured `segments`: the old renderer drew three zones at fixed
-- thirds of [min, max] with a fixed direction (low red, high green) -- wrong for any metric
-- where less is better (p99 latency, error rate, churn...). Task 1 introduced the new shape,
-- `gauge.segments` (an array of `{min, max, color, label?}`), but the renderer still draws a
-- hardcoded auto-range and does not read `segments` yet -- that lands in task 3. This migration
-- only converts what is already stored, ahead of that render-time switch, so that once it lands
-- no saved chart changes how it looks. See docs/en/specs/2026-07-29-chart-redesign-04-gauge.md.
--
-- Colours are the literal hex values behind SERIES_PALETTE[3] / [4] / [1] -- the exact indices
-- the renderer already used for its three zones -- read from
-- frontend/src/components/chart/echarts/palette.ts on 2026-07-29:
--   #EF8C8C  SERIES_PALETTE[3]  red    (low zone)
--   #F9D45C  SERIES_PALETTE[4]  yellow (mid zone)
--   #88BF4D  SERIES_PALETTE[1]  green  (high zone)
-- A migration cannot import the palette module, and pinning these as literals means a later
-- palette retune cannot silently restyle a chart this migration already touched.
--
-- Only rows with an explicit `gauge.min` or `gauge.max` have anything to convert; a row with
-- neither stored already draws today's three zones over an auto-range that follows the data,
-- and writing segments for it would freeze that range instead. With mn = gauge.min (default 0
-- -- the old renderer's own default) and mx = gauge.max, the three segments are exactly the
-- thirds already being drawn:
--   [mn,                 mn + (mx-mn)/3  ] red
--   [mn + (mx-mn)/3,     mn + 2*(mx-mn)/3] yellow
--   [mn + 2*(mx-mn)/3,   mx              ] green
--
-- `gauge.min` set with no `gauge.max` is the one case this cannot reproduce: the old renderer's
-- max there was never read from settings, it was computed from the query result
-- (`value > min ? value * 2 : min + 100`) -- a number this migration cannot know and which
-- changes every time the chart re-runs. Inventing a max would not recover that number, it would
-- fabricate a different, static one and freeze it, changing the chart on its very next render --
-- a worse failure than the one being fixed. Falling through to the render-time default at least
-- keeps the max following the data the way it always has; the cost taken is that the stored
-- `min` is discarded and the range starts at 0 instead of the chart's chosen minimum. That is
-- judged the smaller, more honest failure than a segment array that looks precise and is not.
-- So a min-only row gets NO segments -- only the stale `gauge.min` is removed. (Checked against
-- the live database before writing this migration: there are currently zero gauge charts on it,
-- so today this trade-off costs nothing observed there -- a fact about that one database, not a
-- guarantee about what anyone else's data holds.)
--
-- Both bound checks below test `jsonb_typeof(...) = 'number'`, not `?` (key exists) and not a
-- bare cast: `s ? 'gauge.max'` is true even when the value is a JSON `null`, and `->>` of a JSON
-- null is SQL NULL rather than an error, so a `?`-gated branch would happily build a segments
-- array with `null` bounds. And `(s->>'gauge.min')::numeric` throws on any non-numeric string,
-- array or boolean under that key -- fatal here because the whole table is rewritten inside one
-- transaction, so one malformed row would abort the migration for every chart, not just its own
-- (migration 0101 avoided casts for exactly this reason). A malformed bound is therefore treated
-- as absent, same as a bound that was never set: a malformed `gauge.min` behaves like the old
-- renderer's `cfg.min ?? 0`, and a malformed `gauge.max` takes the no-segments branch like an
-- absent one. The API does not validate individual setting values (`chartparams.rs`), so this is
-- a real shape a caller or older row can carry, not just a defensive nicety.
--
-- Same two points as migration 0101: `-` binds tighter than `->`, so the sub-object is extracted
-- into a plain alias (`s` / `o`) before `-` is used on it; and a non-object `settings` /
-- `vizOverride` (string, array, JSON null) is normalised to `{}` inside that same LATERAL rather
-- than guarded with a `jsonb_typeof(...) = 'object' AND EXISTS(...)` check, since Postgres does
-- not promise left-to-right AND evaluation.
--
-- `vizOverride` (migration 0100) is a PARTIAL override: `gauge.segments` is written into it only
-- when the override itself carried a numeric `gauge.max` -- writing it unconditionally would
-- shadow the base chart's own segments for every card whose override never touched gauge
-- settings at all. The min-only exception is the same rule as the charts statement above: an
-- override with `gauge.min` but no (numeric) `gauge.max` gets its stale `gauge.min` stripped and
-- no segments, for the identical reason -- the old max there was data-derived, not stored.
--
-- No `viz_type = 'gauge'` filter: harmless either way, since these keys are namespaced and a
-- `gauge.*` key sitting dormant on a non-gauge chart is never read by that chart's renderer. Not
-- filtering also means a chart later switched back to viz_type gauge finds its ranges already
-- converted rather than reverting to the old fixed thirds.

UPDATE analytics.charts c
SET params = jsonb_set(c.params, '{settings}', migrated.new_settings)
FROM (
  SELECT
    ch.id,
    (s - 'gauge.min' - 'gauge.max')
      || (CASE WHEN jsonb_typeof(s->'gauge.max') = 'number' THEN
            jsonb_build_object('gauge.segments', jsonb_build_array(
              jsonb_build_object('min', mn,                     'max', mn + (mx - mn) / 3,     'color', '#EF8C8C'),
              jsonb_build_object('min', mn + (mx - mn) / 3,      'max', mn + 2 * (mx - mn) / 3, 'color', '#F9D45C'),
              jsonb_build_object('min', mn + 2 * (mx - mn) / 3,  'max', mx,                     'color', '#88BF4D')
            ))
          ELSE '{}'::jsonb END) AS new_settings
  FROM analytics.charts ch, LATERAL (
    -- Normalised here rather than guarded in the WHERE: `jsonb_object_keys`/`?` on a non-object
    -- either errors or misbehaves, and Postgres does not promise to evaluate an AND left to
    -- right, so a type check sitting beside the operator is a planner convention, not a
    -- guarantee.
    SELECT CASE WHEN jsonb_typeof(ch.params->'settings') = 'object'
                THEN ch.params->'settings' ELSE '{}'::jsonb END AS s
  ) x, LATERAL (
    -- A malformed bound (wrong JSON type, or a JSON null) is treated as absent rather than cast
    -- and allowed to abort the whole statement -- see the header comment.
    SELECT COALESCE(CASE WHEN jsonb_typeof(s->'gauge.min') = 'number' THEN (s->>'gauge.min')::numeric END, 0) AS mn,
           CASE WHEN jsonb_typeof(s->'gauge.max') = 'number' THEN (s->>'gauge.max')::numeric END AS mx
  ) y
  -- A raw `?|` key-existence test would still match (and strip) a row whose only signal is a
  -- `gauge.max: null` -- harmless in effect (nothing gets converted either way, see the CASE
  -- above) but not actually untouched. Testing the same `jsonb_typeof = 'number'` check here
  -- keeps a row with no *numeric* min or max out of the UPDATE entirely.
  WHERE jsonb_typeof(s->'gauge.min') = 'number' OR jsonb_typeof(s->'gauge.max') = 'number'
) AS migrated
WHERE c.id = migrated.id;

UPDATE analytics.dashboard_cards dc
SET options = jsonb_set(dc.options, '{vizOverride}', migrated.new_override)
FROM (
  SELECT
    card.id,
    (o - 'gauge.min' - 'gauge.max')
      || (CASE WHEN jsonb_typeof(o->'gauge.max') = 'number' THEN
            jsonb_build_object('gauge.segments', jsonb_build_array(
              jsonb_build_object('min', mn,                     'max', mn + (mx - mn) / 3,     'color', '#EF8C8C'),
              jsonb_build_object('min', mn + (mx - mn) / 3,      'max', mn + 2 * (mx - mn) / 3, 'color', '#F9D45C'),
              jsonb_build_object('min', mn + 2 * (mx - mn) / 3,  'max', mx,                     'color', '#88BF4D')
            ))
          ELSE '{}'::jsonb END) AS new_override
  FROM analytics.dashboard_cards card, LATERAL (
    -- Normalised here rather than guarded in the WHERE, for the same reason as the charts
    -- statement above.
    SELECT CASE WHEN jsonb_typeof(card.options->'vizOverride') = 'object'
                THEN card.options->'vizOverride' ELSE '{}'::jsonb END AS o
  ) x, LATERAL (
    -- A malformed bound is treated as absent, not cast and allowed to abort -- see the header.
    SELECT COALESCE(CASE WHEN jsonb_typeof(o->'gauge.min') = 'number' THEN (o->>'gauge.min')::numeric END, 0) AS mn,
           CASE WHEN jsonb_typeof(o->'gauge.max') = 'number' THEN (o->>'gauge.max')::numeric END AS mx
  ) y
  -- Same reason as the charts statement above: keeps an override whose only signal is a
  -- `gauge.max: null` fully untouched instead of merely un-converted.
  WHERE jsonb_typeof(o->'gauge.min') = 'number' OR jsonb_typeof(o->'gauge.max') = 'number'
) AS migrated
WHERE dc.id = migrated.id;
