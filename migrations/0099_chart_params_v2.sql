-- Chart params v2: the query stays at `spec`, and every viz's configuration becomes a
-- flat, namespaced `settings` map — see
-- docs/en/specs/2026-07-28-chart-studio-a2-viz-registry-design.md.
--
-- Rows already at version 2 are skipped, and soft-deleted rows are rewritten too so that
-- restoring one cannot resurrect a v1 shape. The frontend still reads v1 (each descriptor's
-- `fromParams` falls back), so a row this misses degrades to the old read path rather than
-- breaking.
UPDATE analytics.charts SET params = jsonb_build_object(
  'version', 2,
  'spec', COALESCE(params->'spec', '{}'::jsonb),
  'settings',
    COALESCE(
      CASE
        WHEN viz_type IN ('bar','row','line','area','scatter','combo') THEN
          (SELECT jsonb_object_agg('cartesian.' || k, v) FROM jsonb_each(COALESCE(params->'cartesian', '{}'::jsonb)) AS e(k, v))
        WHEN viz_type = 'pivot_table' THEN
          jsonb_strip_nulls(jsonb_build_object(
            'pivot.rows', params->'rows',
            'pivot.cols', params->'cols',
            'pivot.metrics', params->'metrics',
            'pivot.showRowTotals', params->'showRowTotals',
            'pivot.showColTotals', params->'showColTotals',
            'pivot.condense', params->'condense',
            'pivot.collapsedRows', params->'collapsedRows',
            'pivot.sort', params->'pivotSort'))
        WHEN viz_type = 'table' THEN
          -- v1 stored no table config; the columns were re-derived from the spec.
          jsonb_strip_nulls(jsonb_build_object(
            'table.metrics', params->'spec'->'metrics',
            'table.columns',
              COALESCE(params->'spec'->'dimensions', '[]'::jsonb) || COALESCE(params->'spec'->'columns', '[]'::jsonb)))
        WHEN viz_type = 'object' THEN
          jsonb_build_object('object.columns', COALESCE(params->'spec'->'columns', '[]'::jsonb))
        WHEN viz_type = 'map' THEN
          jsonb_strip_nulls(jsonb_build_object(
            'map.key', COALESCE(params->'map'->'mapKey', '"us_states"'::jsonb),
            'map.region', params->'map'->'region',
            'map.metric', params->'map'->'metric'))
        ELSE
          -- The nine simple viz types each nested their config under their own name.
          (SELECT jsonb_object_agg(viz_type || '.' || k, v)
             FROM jsonb_each(COALESCE(params->viz_type, '{}'::jsonb)) AS e(k, v))
      END,
      '{}'::jsonb)
    || jsonb_strip_nulls(jsonb_build_object(
         'column_settings', params->'columnSettings',
         'conditional_formatting', params->'conditionalFormatting'))
)
WHERE params->>'version' IS DISTINCT FROM '2';
