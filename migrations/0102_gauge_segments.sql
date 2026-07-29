-- Gauge `min`/`max` become coloured `segments`: the old renderer drew three zones at fixed
-- thirds of [min, max] with a fixed direction (low red, high green) -- wrong for any metric
-- where less is better (p99 latency, error rate, churn...). Task 1 replaced the render-time
-- read with `gauge.segments`, an array of `{min, max, color, label?}`; this migration converts
-- what is already stored so no saved chart changes how it looks. See
-- docs/en/specs/2026-07-29-chart-redesign-04-gauge.md.
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
-- So a min-only row gets NO segments -- only the stale `gauge.min` is removed.
--
-- Same two points as migration 0101: `-` binds tighter than `->`, so the sub-object is extracted
-- into a plain alias (`s` / `o`) before `-` is used on it; and a non-object `settings` /
-- `vizOverride` (string, array, JSON null) is normalised to `{}` inside that same LATERAL rather
-- than guarded with a `jsonb_typeof(...) = 'object' AND EXISTS(...)` check, since Postgres does
-- not promise left-to-right AND evaluation.
--
-- `vizOverride` (migration 0100) is a PARTIAL override: `gauge.segments` is written into it only
-- when the override itself carried `gauge.min` or `gauge.max` -- writing it unconditionally would
-- shadow the base chart's own segments for every card whose override never touched gauge
-- settings at all.

UPDATE analytics.charts c
SET params = jsonb_set(c.params, '{settings}', migrated.new_settings)
FROM (
  SELECT
    ch.id,
    (s - 'gauge.min' - 'gauge.max')
      || (CASE WHEN s ? 'gauge.max' THEN
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
    SELECT COALESCE((s->>'gauge.min')::numeric, 0) AS mn,
           (s->>'gauge.max')::numeric AS mx
  ) y
  WHERE s ?| array['gauge.min', 'gauge.max']
) AS migrated
WHERE c.id = migrated.id;

UPDATE analytics.dashboard_cards dc
SET options = jsonb_set(dc.options, '{vizOverride}', migrated.new_override)
FROM (
  SELECT
    card.id,
    (o - 'gauge.min' - 'gauge.max')
      || (CASE WHEN o ? 'gauge.max' THEN
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
    SELECT COALESCE((o->>'gauge.min')::numeric, 0) AS mn,
           (o->>'gauge.max')::numeric AS mx
  ) y
  WHERE o ?| array['gauge.min', 'gauge.max']
) AS migrated
WHERE dc.id = migrated.id;
