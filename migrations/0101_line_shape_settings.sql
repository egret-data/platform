-- Line shape, missing-value and marker settings replace four booleans that could not express
-- what they described: `smooth` and `step` could both be true, and a two-state `connectNulls`
-- hid a render-time default that made the panel contradict the chart. See
-- docs/en/specs/2026-07-28-chart-redesign-02-line.md.
--
-- Every value written here preserves how each chart already looks. In particular `markers`
-- becomes 'on' rather than 'auto' whenever it was not explicitly off: the old builder showed
-- markers unless told otherwise, so 'auto' would restyle existing charts. New charts get 'auto'
-- from the builder default instead. `step` wins over `smooth` when a row somehow has both set
-- (nothing ever prevented that, and ECharts renders the combination as stepped).
--
-- Both UPDATEs below extract the settings sub-object into a plain `s` alias in a FROM
-- subquery rather than writing `params->'settings' - 'key'` inline: Postgres's built-in `-`
-- (subtraction) sits at higher precedence than the generic `->` operator, so
-- `params->'settings' - 'key'` actually parses as `params -> ('settings' - 'key')` and fails
-- with "operator is not unique" -- confirmed against the real database while testing this
-- migration. Binding the extracted object to `s` first sidesteps the whole precedence question.

-- `analytics.charts.params->'settings'` is the chart's OWN full settings map (not a partial),
-- so every cartesian-family row gets all three new keys written explicitly, computed against
-- the old builder's read-time defaults -- an absent old key must still resolve to the value the
-- chart renders today, not the new reader's own default. Rows with no `cartesian.` key at all
-- (a non-cartesian viz) are untouched, and rows that already carry a new key are left alone too,
-- in case this ever runs twice.
UPDATE analytics.charts c
SET params = jsonb_set(c.params, '{settings}', migrated.new_settings)
FROM (
  SELECT
    ch.id,
    (s - 'cartesian.smooth' - 'cartesian.step' - 'cartesian.connectNulls' - 'cartesian.showMarkers')
      || jsonb_build_object(
           'cartesian.lineShape',
             CASE WHEN s->>'cartesian.step' = 'true' THEN 'step'
                  WHEN s->>'cartesian.smooth' = 'true' THEN 'smooth'
                  ELSE 'linear' END,
           'cartesian.missing',
             CASE WHEN s->>'cartesian.connectNulls' = 'false' THEN 'none'
                  ELSE 'interpolate' END,
           'cartesian.markers',
             CASE WHEN s->>'cartesian.showMarkers' = 'false' THEN 'off'
                  ELSE 'on' END
         ) AS new_settings
  FROM analytics.charts ch, LATERAL (SELECT ch.params->'settings' AS s) x
  WHERE jsonb_typeof(ch.params->'settings') = 'object'
    AND EXISTS (SELECT 1 FROM jsonb_object_keys(ch.params->'settings') AS k WHERE k LIKE 'cartesian.%')
    AND NOT (s ?| array['cartesian.lineShape', 'cartesian.missing', 'cartesian.markers'])
) AS migrated
WHERE c.id = migrated.id;

-- `analytics.dashboard_cards.options->'vizOverride'` is a PARTIAL override (migration 0100):
-- a key present there wins over the chart's own settings, a key absent there means "inherit
-- from the chart". So unlike the chart table above, a new key is only written when its old key
-- was actually present in the override -- writing all three unconditionally would materialise
-- e.g. a hardcoded `markers: 'on'` into an override that never touched markers, silently
-- shadowing whatever the underlying chart actually draws.
UPDATE analytics.dashboard_cards dc
SET options = jsonb_set(dc.options, '{vizOverride}', migrated.new_override)
FROM (
  SELECT
    card.id,
    (o - 'cartesian.smooth' - 'cartesian.step' - 'cartesian.connectNulls' - 'cartesian.showMarkers')
      || (CASE WHEN o ?| array['cartesian.step', 'cartesian.smooth']
            THEN jsonb_build_object('cartesian.lineShape',
                   CASE WHEN o->>'cartesian.step' = 'true' THEN 'step'
                        WHEN o->>'cartesian.smooth' = 'true' THEN 'smooth'
                        ELSE 'linear' END)
            ELSE '{}'::jsonb END)
      || (CASE WHEN o ? 'cartesian.connectNulls'
            THEN jsonb_build_object('cartesian.missing',
                   CASE WHEN o->>'cartesian.connectNulls' = 'false' THEN 'none'
                        ELSE 'interpolate' END)
            ELSE '{}'::jsonb END)
      || (CASE WHEN o ? 'cartesian.showMarkers'
            THEN jsonb_build_object('cartesian.markers',
                   CASE WHEN o->>'cartesian.showMarkers' = 'false' THEN 'off'
                        ELSE 'on' END)
            ELSE '{}'::jsonb END) AS new_override
  FROM analytics.dashboard_cards card, LATERAL (SELECT card.options->'vizOverride' AS o) x
  WHERE jsonb_typeof(card.options->'vizOverride') = 'object'
    AND o ?| array['cartesian.smooth', 'cartesian.step', 'cartesian.connectNulls', 'cartesian.showMarkers']
) AS migrated
WHERE dc.id = migrated.id;
