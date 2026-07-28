-- Dashboard per-card viz overrides follow chart params to v2: a flat, namespaced partial
-- of `settings` instead of the pre-A2 nested display keys. See
-- docs/en/specs/2026-07-28-chart-render-consistency-design.md.
--
-- `viz_type` stays at the top level (it selects the renderer, it is not a setting). Cards
-- with no override are untouched, and the reader still accepts the old shape, so a
-- dashboard opened mid-deploy cannot break.
UPDATE analytics.dashboard_cards c
SET options = jsonb_set(
  c.options,
  '{vizOverride}',
  (
    SELECT COALESCE(jsonb_object_agg(out.k, out.v), '{}'::jsonb)
    FROM (
      SELECT
        CASE
          WHEN o.k = 'columnSettings' THEN 'column_settings'
          WHEN o.k = 'conditionalFormatting' THEN 'conditional_formatting'
          WHEN o.k = 'tableState' THEN 'table.state'
          ELSE o.k
        END AS k,
        o.v
      FROM jsonb_each(c.options->'vizOverride') AS o(k, v)
      WHERE o.k NOT IN ('cartesian','pie','scalar','progress','gauge','funnel','waterfall',
                        'smartscalar','boxplot','sankey','map')

      UNION ALL

      -- Nested per-viz config objects become `<viz>.<field>`; the map's `mapKey` is `key`.
      SELECT viz.k || '.' || CASE WHEN f.k = 'mapKey' THEN 'key' ELSE f.k END AS k, f.v
      FROM jsonb_each(c.options->'vizOverride') AS viz(k, v)
      CROSS JOIN LATERAL jsonb_each(viz.v) AS f(k, v)
      WHERE viz.k IN ('cartesian','pie','scalar','progress','gauge','funnel','waterfall',
                      'smartscalar','boxplot','sankey','map')
        AND jsonb_typeof(viz.v) = 'object'
    ) AS out
  )
)
WHERE c.options ? 'vizOverride'
  AND jsonb_typeof(c.options->'vizOverride') = 'object';
