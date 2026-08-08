# "Hide this card if there are no results"

The last of Metabase's three dashcard-modal settings egret did not have. Metabase's modal carries
**Title**, **Description** and **Hide this card if there are no results** on every chart; egret's card
settings panel had Title, On click, Auto height, Move to tab.

This unit **built** hide-if-empty and **did not** build Description. Both decisions are argued below.

## What Metabase's flag actually does — measured, not read

Driven on a scratch dashboard built for the purpose (never `/dashboard/2`, which is the parity
baseline), one 24-column row of three `8×6` cards, the middle one a card whose SQL returns no rows:

| | |
|---|---|
| flag OFF | the tile renders a **"No results!"** empty state, sailboat illustration and all |
| flag ON | **there is no tile at all** — not an empty state, not a placeholder |
| the row | `Bar │ EMPTY │ Row` at cols 0/8/16 draws as `Bar │ … │ Row` — **the hole stays** |
| a card below | a card at row 6, with the row-0 card hidden, **drew at row 0** |

So Metabase compacts **vertically and not horizontally**. That is not special-case code on their
side: `DashboardGrid` passes `compactType="vertical"` to react-grid-layout, and that is simply what
RGL does when a child disappears. The setting itself is `card.hide_empty`, stored on the DASHCARD,
and `getVisibleCardIds` (`dashboard/utils.ts:359`) filters ids out of the rendered set. It refuses to
hide a card that ERRORED (`!getDashcardResultsError(...)`), and while data is loading it returns
`!wasVisible`, i.e. a card nobody has seen yet starts hidden rather than drawing and then vanishing.

## The sizing, and where the brief was wrong

The brief expected the grid to be the expensive part — *"a hidden card leaves a hole unless the
layout reflows"* — and said to size-and-stop if it interacted with the layout.

**The grid turned out to be free.** egret's `DashboardGridLayout.tsx:64` already passes the identical
`compactType="vertical"`, and `visibleCards` already feeds BOTH the RGL `layout` array and the grid's
children. Dropping an id from that one list reproduces both halves of Metabase's measured behaviour
with no layout maths whatsoever. egret's `onLayoutChange` also already early-returns on `!editing`,
so view-mode compaction is display-only and cannot rewrite a saved `grid_row`.

The real cost was somewhere the brief did not look: **the rows are fetched inside the tile.**
`ChartCardTileView` calls `useDashboardCardData`, but `visibleCards` is computed in `DashboardView`,
which has no access to any card's result. The obvious implementation — let the tile notice it is
empty and ask the parent to drop it — **unmounts the very hook that produced the answer**. The card
disappears, stops querying, and can never learn it has rows again; it would stay hidden until a full
page reload. That is a correctness bug, not a polish issue, and it is the whole reason this unit has
a `CardEmptinessProbe` in it.

## What was built

- `hide-empty.ts` — `hiddenEmptyCardIds(cards, emptiness, editing, blocked)`, the whole decision,
  pure and therefore testable. Chart cards only (Metabase hides the toggle for virtual dashcards via
  `getHidden: isVirtualDashCard`); nothing hides while editing (Metabase's `getVisibleCards` returns
  every tab card when `isEditing`); an unresolved card counts as empty, matching Metabase's
  `!wasVisible`.

  Two guards were **not** in the first cut and were found by reading the diff rather than by any
  test failing — both are cases where "unknown" is not "still loading" but "will never be answered",
  and the unknown-means-hidden default would have hidden a card permanently:

  - a chart card whose `chart_id` is null runs no query, so nothing ever reports for it. Its tile's
    "Chart not found" is what should show — the same reasoning that keeps an errored card visible.
  - a dashboard with a **required parameter left unset** blocks every card, which renders "Set the
    required filters to load this card" and never queries. Without the `blocked` guard the whole
    board would vanish behind the filter bar with no explanation.

  `probeCards` in `DashboardView` carries the identical predicate, and it has to: if the two ever
  disagree, a card can be hidden with nothing left running to un-hide it.
- `CardEmptinessProbe.tsx` — runs the card's query OUTSIDE the grid and reports emptiness up. Renders
  `null`. It costs **no extra request**: `useDashboardCardData` is a react-query `useQuery` keyed on
  `["dashboardsDbCardRun", ws, dashboardId, cardId, nonce, filters, temporalUnits]`, so a visible
  card's probe and its tile share one cache entry. **Verified live** — a two-card board issued
  exactly one `/cards/{id}/data` request per card. `DashboardView.cardQueryArgs()` is the single
  source of those arguments precisely so the two callers cannot drift and accidentally double-fetch.
  The probe stays silent while loading, so a refetch cannot blink a visible card out.
- `CardSettingsPanel` — the toggle, using **Metabase's exact wording**, since this panel is measured
  against its modal.
- An "every card hid itself" empty state, because falling through to the existing
  *"No cards yet. Click Edit to add charts."* would be a lie on a board that has cards.

Frontend-only. `options` is a free-form `jsonb` blob end to end
(`migrations/0015_dashboard.sql:24`, `DashboardCard.options: Record<string, unknown>`), so this needs
**no Rust change, no migration and no OpenAPI regeneration**.

## Verified live

Built `dist`, served it through a reverse proxy to kind's `:30080` (bundle hash checked against the
build), scratch dashboard in `demo`, two cards, hide-if-empty on the right-hand one only:

1. no filter — both cards draw, so opting in does not hide a card that has rows;
2. filter set to a value matching nothing — **the right card is gone**, and the left card holds its
   column with the hole to its right, exactly as Metabase does;
3. filter cleared — **the card comes back**, without a reload. This is the step the naive
   implementation fails, and it is the reason the probe exists.

The parity baseline was re-checked afterwards and is untouched: 22 cards, `updated_at
2026-08-08T01:03:49.111686Z`, `vizOverride` 0, `hideEmpty` 0. The scratch dashboard was deleted.

## Description — decided separately, and NOT built

It is smaller than hide-if-empty in logic and bigger in surface, which is why it does not belong in
this unit.

The setting itself is a second textarea beside Title writing `options.description` — five lines, no
data-flow problem at all, since the tile already has everything it needs. **But there is nowhere to
render it.** egret's tile header is title-only (`DashboardCardView.tsx:55-71`): no info icon, no
tooltip, no `Tooltip` import. Metabase shows a dashcard's description behind an info icon in the card
header. So shipping Description means building that header affordance, which is net-new UI needing
its own Metabase-parity pass, plus a decision the title already has an answer for but description
does not: a chart HAS a `description` field (`api/charts.ts:494`) that no dashboard tile has ever
rendered, so a card-level description has to define whether it overrides or falls back to the
chart's, the way `title` falls back to `chart.name`.

**Recommendation: size Description with the tile-header/card-metadata work, not with this.** It is
one setting and one icon, but the icon is the part that has never existed.

## Found, not fixed

- egret's dashboard-parameter value picker offers only values present in the data, so there is no way
  to drive a card to "no results" from the UI alone. The live check above had to set the parameter
  through the URL query string. Not a defect — but it does mean a user cannot easily *see* their own
  hide-if-empty setting take effect, which is worth remembering if anyone reports it as broken.
