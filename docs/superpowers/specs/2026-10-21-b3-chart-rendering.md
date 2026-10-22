# B3: chart rendering bugs (plan)

Package B3 of the known-issues fix programme. Frontend only; no backend, no migration.
Every row reproduced on the demo stack before the fix (main at `3eee5fcd`).

| Row | What changes |
|---|---|
| KI-043 | `.png` is offered only when the card drew a chart image: `EChartsRenderer` marks its container `data-chart-image`, and the exporter reads the `<svg>` inside it (`chartImageSvg`). There is no DOM-capture path in the exporter, so Object detail (and any card without an ECharts surface) is simply not offered `.png`, as Metabase's `canSavePng` leaves out object. |
| KI-049, KI-072 | The popover note reads `The maximum download size is ${MAX_CHART_ROWS} rows.`, derived from the constant. One component serves the editor and the dashboard card. |
| KI-044 | `ProgressChart` no longer defaults `goal` to 0; an empty goal (or a goal column whose cell is null) is "No goal set". |
| KI-045 | Decision D7: the row chart's default sort is its first metric, descending, with the x-axis as tie-breaker; an authored sort wins. Horizontal category axes are drawn `inverse`, so rows read top to bottom in query order (largest on top). `authoredOrderby` now finds the shortest authored prefix that rebuilds the stored sort, so an authored `dimension ASC` on a row chart is not mistaken for the derived tail. The gallery text already says "sorted by value". |
| KI-046 | Region maps drop rows that match no region before building the series (so the legend range covers only coloured regions) and show a note under the card naming up to three unmatched values, as pin maps do. Vietnam's province list and archipelagos are untouched. |
| KI-047 | Change between descriptors that share a settings prefix (the `cartesian.*` family: bar, line, area, combo, row, scatter) keeps the config and the sort; other switches reset as before. |
| KI-051 | `honoursRowLimit(descriptor, config)` probes the descriptor like `honoursOrderby`; Row limit is hidden for Big number, Progress and Gauge. |
| KI-057 | The Total swatch falls back to `defaultTotalColor()`, the same function the builder draws with, written as hex (`toHexColor`) for the native colour input. |
| KI-163 | The metric-vs-metric scatter's axes get `nameTextStyle` in the axis-label colour, like every other cartesian axis. |
| KI-164 | The funnel's bar mode passes a size-aware option, so the shared x-label fit decides (all labels upright, rotated, or none). |
| KI-165 | The box plot measures the last box's value labels and widens `grid.right` by the overhang (`labelOverhangRight`). |

Decisions taken here: the PNG fix withdraws `.png` rather than rasterising the DOM (no capture path
exists); Change keeps config across the whole cartesian prefix (row and scatter too, since they share
it), not only the four named in the row; saved row charts whose stored sort is the old derived
`dimension ASC` now read as an authored alphabetical sort (visible in the Sort control) rather than
being silently re-sorted.
