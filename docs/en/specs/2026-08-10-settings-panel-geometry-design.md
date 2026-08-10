# Chart settings panel: geometry parity with Metabase

**Status:** design, approved 2026-08-10
**Programme:** follow-on to the chart-parity and dashboard-parity programmes

## 1. What this is about

The chart programme brought the settings panel to parity on *which controls exist*. It did not
touch *how they are laid out*. A live sweep of all 19 chart types on both sides — egret at
`localhost:30080`, Metabase OSS at `localhost:3033`, the same "MB · …" parity cards on both —
measured every control in every Display and Axes panel.

| | Metabase | egret |
|---|---|---|
| Distinct control widths across 19 Display panels | **37** | **85** |
| Distinct control heights | **34** | **67** |

Metabase's 37 is itself inflated: the `104 / 112 / 137` triples that recur are one segmented
control (`Auto | Compact | Full`) whose three buttons size to their text. Counting components
rather than boxes, Metabase uses **one** content width (350–352 px) and **one** control height
(40 px), plus a right-aligned switch at 40×21. egret has no such convention.

The worst offenders, and the shape of the problem:

| chart | widths | heights | example |
|---|---|---|---|
| line, combo | 8 | 5 | `55` and `253` in the same panel |
| bar, row, area | 7 | 5 | |
| pie | 7 | 4 | |
| scatter, gauge, table | 6 | 5 | |

Every cartesian Axes tab shows the same pair: **`X axis scale` at 112 px sits directly above
`Show lines and tick marks` at 255 px.** Both are selects, both in the X-axis group. In Metabase
both are 352.

### Controls below a usable size

Present on **10 of 19** charts:

- `8×10 px` reorder arrows — bar, row, line, area, combo, scatter, and **table** (22 of them, two
  per column)
- `9×16 px` — pie (8 of them)
- `18×18` remove-range on gauge, `16×16` on boxplot
- `94×16` "More options", `47×16` "Show all", `42×16` "Hide all", `134×16` "Add or remove columns"

Metabase uses 22×22 icon buttons for the equivalent job. An 8×10 target is about one sixth the
area, and these are the controls a user reaches for repeatedly while arranging a chart.

## 2. Root cause — and why it is countable

This is not 19 chart-specific problems. It is four shared defects:

**2.1 Height: 92 call-site overrides.** `components/ui/select.tsx` defaults to
`data-[size=default]:h-9` (36 px) and `components/ui/input.tsx` to `h-9`. Neither default is ever
used in the settings directory. Instead the call sites override it **92 times**: `h-8` ×58 (32 px)
and `h-7` ×34 (28 px). The three measured heights — 28, 32, 36 — are exactly these.

**2.2 Width: 22 fixed-width overrides.** `w-24` ×8 (96 px), `w-16` ×6 (64 px), `w-28` ×5 (112 px),
plus `w-72`, `w-64`, `w-56`, `w-[18px]`. The measured 64/96/112 are exactly these.

**2.3 Unsized glyph buttons.** The reorder controls are bare `▲`/`▼` text at `text-[10px]` with no
box (`SeriesListSection.tsx:134`, and the same pattern in `TableColumnsSection.tsx`). Their size is
whatever the glyph happens to be — hence 8×10.

**2.4 The panel is too narrow to hold Metabase's geometry.** `ChartExplore.tsx:633` sets `w-72`
(288 px) with `p-4`, giving 256 px of content. Metabase's is 400 px with 352 of content. Long
labels like "Show lines and tick marks" do not fit at 255, which is *why* the narrow variants were
introduced in the first place. Normalising the controls without widening the panel would trade one
defect for another.

## 3. Target

Adopt Metabase's geometry exactly, per the decision of 2026-08-10.

| | value | how |
|---|---|---|
| Panel | **400 px** | `w-[400px] p-6` on `ChartExplore.tsx:633` — 400 − 2×24 = **352** content |
| Full-width control | **352×40** | primitives default to it; call sites stop overriding |
| Icon button | **≥24×24** | shared `IconButton`, replacing bare glyphs |
| Switch | **40×21, right-aligned** | already close; keep |

Two geometries in the whole panel, as Metabase has. `352` is a *consequence* of the panel width
and padding, not a number to hard-code at call sites — that is what produced the mess being fixed.

### Also in scope (approved)

1. **Icon buttons ≥24 px** — §2.3.
2. **`Group order_month by`** — the label repeats a column name shown directly above it, and the
   control is the odd 112 px. Fold the bucket into the column control itself, as Metabase does
   (`Created At: Month`), so there is one control and no second row.
3. **ALL-CAPS section labels → sentence case.** (The "1 px ragged edge" this item originally also
   carried turned out not to exist — see §6.)
4. **Remove `More options (2)`** — show the settings, as Metabase does. A disclosure with a count
   advertises that something is hidden without saying what.

## 4. Non-goals

- **Not** re-opening which settings exist. Control inventory is already at parity (19 = 19 chart
  types; Metabase's 20th, `list`, is `hidden: true` and not user-selectable).
- **Not** touching the dashboard card settings panel. Different surface, different sweep.
- **Not** the table/pivot column-settings UI beyond the shared primitives. The sweep could not
  measure Metabase's equivalent (§6), so there is no parity target to hit yet.

## 5. Testing strategy — and one thing that cannot be tested the usual way

**jsdom cannot verify any of this.** It does no layout: every `getBoundingClientRect()` returns
zeros. A jsdom test asserting "the select is 352 wide" passes against every implementation,
including one that never changed. Writing one would produce exactly the kind of green-but-empty
assertion this codebase has been bitten by six times.

The guard therefore has to sit where the defect actually lives — **in the source**:

```ts
// A unit test over the settings directory, not over a rendered tree.
it("no call site overrides the primitive control geometry", () => {
  const offenders = filesIn("components/chart/settings")
    .flatMap(f => matches(f, /\b(h-7|h-8|w-16|w-24|w-28)\b/g));
  expect(offenders).toEqual([]);
});
```

This is mutation-checkable in the ordinary way: re-add one `h-8` and it goes red. It pins the
thing that regresses — a future contributor reaching for `h-8` to make something fit.

Geometry itself is confirmed once, live, by re-running the sweep in §1 and requiring egret's
distinct-width count to fall from 85 to ≤ 25 and heights from 67 to ≤ 25. That measurement is
evidence for the PR, not a CI gate; it needs two running apps.

## 6. Findings that did not survive checking

Recorded so they are not "rediscovered" later:

**The 830 px control on pie and the 609 px one on funnel do not exist.** The first sweep bounded
the panel at `x < 760`, which swept in the chart canvas; those were pie legend entries and funnel
steps, which are legitimately clickable. Re-running with the bound taken from the tablist's own box
changed 3 of 19 charts (pie, funnel, object) and left the other 16 identical. All numbers in this
document are from the corrected pass.

**The 1px ragged edge is not a defect.** The FORMATTING rows measure `253 @ x=265` against `255 @
x=264` everywhere else, which read as a misalignment. Measuring the wrapper instead of the button
settles it: the bordered card at `FormatSection.tsx:80` is itself **`255 @ x=264`**, flush with
every sibling, and the 253 is the button sitting inside the card's own 1px border — correct
rendering. Nothing to fix. Recorded because the raw numbers look like a bug and will again.

**Metabase's `table` and `pivot` counts of 0 do not mean Metabase has no settings there.** The
selector did not reach their column-settings UI. Those two cells are unusable for comparison, which
is why §4 excludes that surface from the target.

**`map`, `scalar`, `gauge`, `pivot`, `object` have no separate Display tab in Metabase** — their
settings are one pane. Their measured numbers are of whatever pane was open, so "egret has more
distinct widths than Metabase" is not a supported claim for those five.

## 7. Where egret is already better

Not to be lost while copying Metabase:

- **Sort, Row limit, Max categories, Tooltip metrics live in the panel.** In Metabase, sorting and
  row limits require editing the query in the notebook editor. Keep them.
- **Temporal bucketing works on a SQL result.** Metabase cannot re-bucket a SQL question at all —
  you must edit the SQL. Item 3.2 changes how the control is *presented*, never that it exists.
- **Tabs are URL-addressable** (`?tab=display`). Metabase's sidebar state is not.
- **One series row carries colour, rename, display type and visibility together.**
