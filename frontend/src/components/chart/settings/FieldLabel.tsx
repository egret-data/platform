/**
 * The class every FIELD label in the chart studio's config sidebar wears — one level below
 * `SectionLabel`, which is the heading over a GROUP of them.
 *
 * WHY IT IS NOT UPPERCASE ANY MORE. The Data tab had exactly one typographic treatment:
 * `text-xs font-medium uppercase tracking-wide text-muted-foreground`, on DATASET, METRICS,
 * COLUMNS, X-AXIS, SERIES, MAX CATEGORIES, METRIC, REGION, ROW LIMIT — and on `SectionLabel`,
 * which differs from it only by a bottom border. Everything shouted, so nothing was emphasised
 * relative to anything else and the panel read flat: a reader could not tell a group heading from
 * the label of one select, because they were the same size, the same weight, the same colour and
 * the same case.
 *
 * Metabase's own setting label is `<Text component="label" fw="bold" htmlFor={id}>` —
 * sentence case, foreground colour, no letter-spacing (`ChartSettingsWidget.tsx`) — and its
 * sidebar draws no uppercase text at all; its groups are separated by a rule
 * (`ChartSettingsWidgetList.styled.tsx`), not by a shouting heading. This is the same shape:
 * sentence case for the field, `SectionLabel` kept uppercase for the group above it, so the two
 * levels are told apart by CASE and COLOUR rather than by a 1px border.
 *
 * `text-sm font-normal` and not Metabase's bold, because the Display tab's labels — Legend,
 * Data labels, Line shape, Missing values, Stacking, Show row totals — have always been
 * `text-sm font-normal`, and there is no reason a field on Data should look different from a field
 * on Display. Unifying on the treatment that already existed is what makes the two tabs read as
 * one panel; introducing a third weight would not have.
 *
 * FIVE COPIES BECAME ONE. `LABEL_CLASS` was declared byte-for-byte identically in `viz/fields.tsx`,
 * `viz/cartesian.tsx`, `viz/map.tsx`, `viz/table.tsx` and `SortRows.tsx`. That is why this change
 * had to be made in five places to be made at all, and why the next one would have been made in
 * four of them.
 */
export const FIELD_LABEL = "text-sm font-normal text-foreground";

/**
 * The heading over a small group of fields NESTED INSIDE a section — a pivot's Rows/Columns/Values
 * zone, the Pagination block inside the table's column settings, the Columns picker inside a
 * conditional-formatting rule.
 *
 * A third level exists because a second was not enough: `SectionLabel` draws a rule across the
 * panel, which is right at the top of a tab and wrong three levels down inside a popover, and
 * `FIELD_LABEL` is the label of one control and must not also mean "here comes a group". So the
 * panel now reads at three distinct sizes without any of them shouting:
 *
 *   SECTION   uppercase, bordered, muted, xs   — `SectionLabel`
 *   GROUP     sentence case, muted, xs, medium — this
 *   FIELD     sentence case, foreground, sm    — `FIELD_LABEL`
 *
 * The distinction that carries the hierarchy is COLOUR and SIZE (muted xs versus foreground sm),
 * which survives at any zoom, rather than letter-spacing, which does not.
 */
export const GROUP_LABEL = "text-xs font-medium text-muted-foreground";
