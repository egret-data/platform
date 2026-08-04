# Chart Redesign 05 — Pie

**Status:** approved design, pending implementation plan
**Date:** 2026-07-29
**Program spec:** `docs/en/specs/2026-07-28-chart-redesign-program-design.md`
**Previous units:** bar `1fe27573`, line `1ee0819e`, table `2638db51`, gauge `9a8e810b`
**Vietnamese reference:** `docs/vi/specs/2026-07-29-chart-redesign-05-pie.md`

## The pie row was wrong too — the third in a row

The comparison said pie was the row egret was **ahead** on, with Metabase adding only
`pie.decimal_places`. Chart 05 was scheduled fifth *because* of that: a UX unit with no gaps to fill.

Reading both sides on 2026-07-29:

| | egret | Metabase |
|---|---|---|
| Dimensions | one — `dimension?: string` | **`pie.dimension: string[]`, up to three nested rings** (`DimensionsWidget`, `dimensions.length < 3`) |
| Rename a slice | — | `SliceNameWidget` |
| Hide / reorder slices | — | `PieRowsPicker`, via `pie.rows` |
| Percent decimals | — | `pie.decimal_places` |
| Donut, centre total, slice threshold, per-slice colour, percent visibility, sort, legend, labels | yes | yes |

Three rows of this comparison have now been checked — table, gauge, pie — and **all three were wrong
in the same direction**: they understated Metabase and overstated egret. The audit already flagged
that the other fifteen rows are unverified; this is the third piece of evidence that "unverified"
should be read as "probably also wrong".

## What a pie is for

Showing how a whole divides. One number per slice, and the slices sum to something meaningful.
A second ring answers the follow-up — *and how does each of those divide?* — without making the
reader open another chart.

## The gap: one level of division

egret's pie can answer "revenue by region". It cannot answer "revenue by region, and within each
region by category" — the question a reader asks immediately afterwards. Metabase can, to three
levels.

## Settings

```
pie.dimensions   : string[]                       up to 3; replaces `dimension`
pie.sliceNames   : Record<string, string>         rename a slice
pie.hiddenSlices : string[]                       slices left out
pie.sliceOrder   : string[]                       display order
pie.decimals     : number                         decimal places on percentages
```

`pie.dimension` is **removed**, not kept alongside — the same reasoning as line's `smooth`/`step` and
gauge's `min`/`max`. Two sources of truth for one thing disagree the moment either is edited.

**Names, order and hiding apply to the first dimension's slices**, matching Metabase's `pie.rows`.
Extending them to inner rings would need a compound key and is not worth the complexity for a chart
whose second and third rings exist to be scanned, not curated. Say so in the panel.

**Temporal units come for free.** `dimensionUnit` is already derived at render from
`params.spec.temporal_units[dimension]` — a shared map keyed by column name — so each ring looks up
its own without any new setting.

## Ring colours

A three-ring pie is unreadable if every slice gets an unrelated palette colour. Metabase solves it
by giving each ring a **lightness variant of the same family colour** — inner dark, outer light
(`getRingColorAlias`).

egret's palette is OKLCH, which is the right space for exactly this: adjust L, keep C and H, and the
family stays recognisable while depth reads at a glance. So an outer slice inherits its parent's
colour, lightened per ring. A per-slice colour override still wins where the user has set one.

## `fieldDescriptor` gains a `dimensions` field kind

Pie is declared through `fieldDescriptor`, whose `fields` support four scalar kinds. An ordered
multi-select of up to N columns is not one of them, and unlike gauge's segments it **cannot** be a
`display` control — dimensions are the query, so they belong in the Data section, which only `fields`
can reach.

So `fields` gains a `dimensions` kind: an ordered list of column pickers with a maximum. That is a
genuine extension, but a narrow and homogeneous one — a list of column names, not a structured
record — and it is the same widget any future viz wanting ordered dimensions will need.

## Migration 0103

`pie.dimension` becomes `pie.dimensions: [<that value>]`, and the old key is removed. Both storage
locations, as always: `charts.params.settings` and `dashboard_cards.options.vizOverride`, with the
override written only where it carried the old key.

A pie with no stored dimension gets nothing — there is no value to lift.

**Migration 0102 taught two things this one must not relearn:** gate on
`jsonb_typeof(...) = 'string'` rather than on key existence, because a key present with a JSON
`null` is not a value; and do not let one malformed row abort the statement for the whole table.

## Deliberately not done

- **No fourth ring.** Metabase stops at three and so does this; past three a pie is a treemap
  wearing the wrong shape.
- **No per-ring slice curation** — see above.
- **No sunburst-style drill on click.** Clicking a slice already drills through in egret; changing
  what that means on a multi-ring pie is a separate decision.
- **Donut, slice threshold, sort and percent visibility keep their settings.** They work, and the
  audit was right that egret is ahead there. The centre total keeps its setting too, but not its
  typography — see below.

## A one-ring pie no longer looks like it did on `main`

This spec originally promised that a one-dimension pie would render exactly as before. **That
promise has been withdrawn**, deliberately, in `935e74d0`: the single ring was given Metabase's
geometry rather than being fenced off from it. Fencing it off would have left egret with two
different pies — a nested one that reads like Metabase's and a plain one that doesn't — for no
reason beyond the promise itself.

Before this branch, a one-ring pie drew ECharts' defaults: labels **outside** the circle, each
joined to its slice by a leader line, at the default size and regular weight; slices meeting edge
to edge with no gap; and, with `Donut` and `Show total` on, a bold value over a plain 13px "Total"
caption.

Now:

- **Labels sit inside the slices**, at 14px and weight 700, in whichever of the theme's two ink
  colours actually reads against that slice's own fill. No leader lines. A label its slice cannot
  hold is truncated with an ellipsis, or dropped outright if even the truncation would leave under
  three real characters.
- **A 1° gap separates adjacent slices**, so the divisions read without needing a border.
- **The centre caption is bigger and bolder**: the value stays at 22px, and "Total" goes from a
  plain 13px to 14px at weight 700.
- **Legend and slice labels default to on.** Both are Auto unless set, and Auto means shown — a
  pie's colours say nothing without a key. An explicitly saved `false` still turns either off.

  > **The slice-labels half of this was later reversed** (`resolveShowLabels`, options/pie.ts).
  > The reason given here — "a pie's colours say nothing without a key" — argues for the LEGEND,
  > which is the key and which still defaults on; it says nothing about repeating the key on the
  > arcs. Metabase's `getDefaultShowLabels` returns `false` at one dimension, and since the
  > percentage default moved to "legend" at every ring count, a one-ring pie's legend already reads
  > `US  75%` — so arc names were the same string twice. **Two rings and up keep the default**,
  > because there the argument does hold: the legend lists ring 0 only, so the outer rings have no
  > key anywhere else.
- **The radii are the one thing that did not move.** `Donut` off is still a full circle; `Donut` on
  still opens the same hole. Metabase's own one-ring ratio was deliberately not adopted, because a
  plain pie's proportions were never the thing that read badly.

**Every existing saved single-ring pie therefore changes appearance on upgrade.** Nothing is stored
differently and nothing needs migrating for it: this is entirely a render-time change, so it reaches
every pie already saved, and undoing it for one chart is not possible — it would be a code change.

## Decision record

Every commit on this branch has an empty body, so `git log` and the PR show only the pre-reversal
text above. Two decisions written down earlier in this document were deliberately reversed
mid-branch; a third finding explains why both were harder to judge than they look. Recorded here so
none of them is quietly re-introduced by someone reading only the original text.

### An outer slice's percentage is a share of its PARENT, not of its ring — reversed in `bd14992f`

The first implementation computed every slice's percentage as its share of its own ring's total,
reasoning that a pie shows parts of a whole and that this kept all rings comparable with the
innermost one. On a real three-ring chart that does not survive contact. It holds for ring 0, whose
parent *is* the whole; it breaks on every ring outside it. A channel worth 500 inside one
region-and-product parent of 1000 read as **4.17% of everything** (500/12000) — arithmetically true,
and useless to whoever is looking at that one wedge, who wants **50% of its parent**. That is what
Metabase computes (`normalizedPercentage`), and what rings 1+ now show.

Ring 0 is untouched: its share of its ring already is its share of the chart. ECharts computes
`percent` once per series with no per-datum override, so each outer slice carries a precomputed
`parentPercent` on its own datum, which both the label formatter and the shared tooltip prefer
whenever it is present.

### Metabase's 3× white-text bias was ported, then removed — `f8b8483b`, reversed in `208a1a73`

Metabase's `getTextColorForBackground` multiplies white's contrast by a flat 3 before comparing
("we intentionally want to return white text color more frequently", per its own design notes). It
was ported faithfully first, as planned. On egret's own palette it measurably made contrast
**worse**: for `#509EE3` and `#A989C5`, dark text clears WCAG AA comfortably — 6.20 and 5.97 against
egret's real dark-text token — while the bias picked white, whose contrast against those fills is
capped by the fills' own luminance at roughly 2.86 and 2.97, nowhere near 4.5. It did not nudge a
borderline call in a debatable direction; it turned a passing label into a failing one.

The bias is a look preference Metabase made deliberately, not a legibility law, and 4.5:1 is
egret's higher-priority rule, so where the two conflict the bias loses. What shipped instead: pick
whichever candidate has the better raw contrast, and apply the white preference **only** as a
tie-break between two candidates that both already clear 4.5:1 — at which point neither reading is
at risk and Metabase's white-leaning look is free to win.

### The label colour picker never worked in the live app until `ea793344`

Recorded because it is why both decisions above were harder to judge than they look. `parseColor`
could not read `oklch()`, and egret's design tokens (Tailwind v4) are written that way — a browser
hands a resolved `var(--foreground)` back as `oklch(...)`, not `rgb(...)`, because
`getComputedStyle` only serialises legacy sRGB colours to `rgb()`. So in the real app every
contrast comparison fell through to the unparseable-input fallback and returned the default ink
regardless of the fill. Light mode still looked correct, purely because that fallback happens to
suit a light palette; dark mode is where it showed.

The lesson to keep: anything that reads a design token to make a contrast decision must be tested
against a real `oklch()` string directly, not only through the function that consumes it. Testing
it only through the chosen-candidate output is exactly what let this stay broken.

## Live check

On the Demo Sales dataset:

1. A one-dimension pie draws **one** ring over the same slices as `main`, with the same radii — but
   with inside labels, 1° gaps and the larger centre caption described above. Still one ring over
   the same slices is what the migration is actually being tested on; identical pixels are not.
2. Adding `category` as a second dimension draws a second ring, whose slices nest inside their
   region.
3. The outer ring's colours are recognisably lighter members of the inner slice's family.
4. A third dimension works; removing back to one works.
5. Renaming a slice changes it in the chart and the legend.
6. Hiding a slice removes it and the remaining percentages re-base.
7. Percent decimals change what the labels show.
8. The same pie as a small dashboard card, with two rings.

## Verification

The program's gate: lint, typecheck, tests, build, deploy to kind, live check, before/after
screenshots at both sizes. There is a migration, so the backend gate is in scope and the backend is
rolled first.

**As with gauge, the migration needs a fixture that does not exist yet** — check whether the kind
database has a saved pie with `pie.dimension`, and if not, create one on `main` before deploying the
branch. A migration verified only against an empty table is not verified.
