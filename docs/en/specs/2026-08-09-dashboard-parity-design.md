# Dashboard Parity with Metabase — Design

**Status:** approved 2026-08-09
**Scope:** the egret dashboard surface (view mode, edit mode, cards, filters, tabs, info)
**Reference:** Metabase OSS checkout at `data-portal/metabase`, live instance `localhost:3033`
**Subject:** egret at `data-portal/egret`, live instance `localhost:30080`

This is the dashboard counterpart to `2026-08-01-chart-parity-design.md`. That programme
brought the nineteen chart renderers and their settings panels to parity. The chart *inside*
a dashboard card is therefore already correct; what follows is about everything around it.

---

## 1. How the divergences were found

Two independent code inventories (one per side) plus a live measurement pass driving both
running apps at 1440×900, 1800×900 and 390×844. Every claim below carries either a `path:line`
or a measured number. Findings that evaporated on inspection are recorded in §9 so nobody
re-derives them.

The harness dashboard is "ZZ Chart Parity", 22 cards per side on byte-identical data
(`docs/en/guides/chart-parity-harness.md`). It was built to compare *renderers*, so it is a
weak instrument for dashboard chrome — most findings here came from driving the UI, not from
the harness.

---

## 2. Measured defects

### 2.1 The dashboard is unusable below the mobile breakpoint

Measured at a 390×844 viewport on the harness dashboard:

| | grid width | card size | horizontal scroll |
|---|---|---|---|
| egret | 86px | **9 × 224 px** | yes — `scrollWidth 441 > 390` |
| Metabase | 362px | 362 × 290 px | no — `scrollWidth 390 === 390` |

Two independent causes:

1. `DashboardGridLayout.tsx:57-60` renders a plain `RGL` with `cols={GRID_COLS}` hard-wired to
   24. There is no breakpoint, no responsive wrapper, no mobile column count. Twenty-four
   columns of an 86px container is a 3.6px column.
2. `AppShell.tsx:9-19` collapses the navigation sidebar from `localStorage` only. There is no
   viewport query anywhere in `components/layout/` (the single `matchMedia` call at
   `Header.tsx:58` is `prefers-color-scheme`). At 390px the sidebar still claims 264px.

Metabase's rules, quoted literally from `utils/dashboard_grid.ts:11-26` and
`dashboard/components/grid/utils.ts:21-36`:

- `GRID_BREAKPOINTS = { desktop: 753, mobile: 752 }`
- `GRID_COLUMNS = { desktop: 24, mobile: 1 }`
- margins `{ desktop: [6, 6], mobile: [6, 10] }`
- on mobile every card is forced to `x: 0, w: 1` and stacked in layout order
- mobile height by display type — `action` 1, `link` 1, `heading` 2, `scalar` 4,
  `text` `max(2, desktopHeight)`, everything else `MOBILE_DEFAULT_CARD_HEIGHT = 6`
  (`visualizations/shared/utils/sizes.ts:36-58`)
- drag and resize are disabled at the mobile breakpoint
  (`dashboard/components/grid/GridLayout.tsx:234-235`) and layout changes are **ignored**
  unless editing *and* on desktop (`DashboardGrid.tsx:299-301`) — without that second guard a
  phone visit would silently rewrite the saved desktop layout to one column

### 2.2 Cancelling an edit destroys work with no confirmation

`DashboardView.tsx:249` discards all draft state synchronously. Live-proven: a filter added in
edit mode disappeared on Cancel with no prompt.

Metabase routes Cancel through `DashboardLeaveConfirmationModal` when the dashboard is dirty —
"Discard your changes?" with Cancel / Discard changes
(`DashboardLeaveConfirmationModal.tsx:29-46`, dispatched at `DashboardHeader.tsx:69-87`).
It also guards route navigation, not only the in-app Cancel button.

egret has a `beforeunload` guard (`lib/useUnsavedChangesWarning.ts:14-24`) but explicitly
declines to intercept SPA navigation. So today the only protected exit is closing the tab —
the *least* likely way to lose the work.

### 2.3 The filter→card mapping control cannot show a column name

Measured with the filter sidebar open at 1440px: the mapping `<select>` in each card overlay is
**97px wide** (`clientWidth === scrollWidth === 97`), rendered by
`CardFilterMapOverlay.tsx:8-19` inside a card that is itself only 125px wide at that moment.

Metabase gives the mapping control the card's width, labels it ("Column to filter on",
"Connect to", "Variable to map to" depending on card kind —
`DashCardCardParameterMapperContent.tsx:106-120`), and when a card *cannot* be mapped it says
why: "The selected filter is on another tab." `:188-195`, "This filter can only connect to its
own card." `:197-204`, plus per-kind hints for text/link/iframe cards `:134-144`.

egret renders a bare truncated select and nothing else.

### 2.4 Edit-mode grid guides are nearly twice Metabase's contrast

Both sides use the same mechanism — a repeating SVG cell grid as a CSS `background-image`
(egret `grid-background.ts:15-25`, ported from Metabase's `generateGridBackground`). They
differ only in stroke colour. Composited against each side's own canvas and measured:

| | stroke | canvas | contrast |
|---|---|---|---|
| egret (dark) | `oklch(0.992857 0 0 / 0.28)` → `#4d4d4d` | `bg-dashboard` `#0d0d0d` | **2.42 : 1** |
| egret (light) | same token, light theme | `bg-dashboard`, light | **1.77 : 1** |
| Metabase | `hsla(195, 6%, 87%)` → `#dcdfe0` | `#f9f9fa` | **1.27 : 1** |

⚠️ The first measurement of this taken for the spec read **2.36:1**, against `--background`
`#090909`. That is the wrong surface: the guide grid paints on `bg-dashboard`, `#0d0d0d`. The
conclusion is unchanged but the number is 2.42, and a live check told to expect 2.36 would look
like a miss.

⚠️ **The pixel-readback technique used to take these numbers cannot run in the unit suite.**
The test environment is jsdom with no `canvas` dependency, so `getContext("2d")` returns `null`
and a readback there measures nothing at all. Unit tests must use the house method already in
`src/__tests__/focusIndicatorContrast.test.ts` — parse `index.css`, convert OKLCH to sRGB, compute
WCAG luminance — and the real readback belongs in the live check, where there is a compositor.

`DashboardGridLayout.tsx:28` documents the choice: `--border` was judged "nearly invisible", so
the stroke is blended toward `--foreground` via `color-mix`. But Metabase's guide *is* nearly
invisible by design — it is a hint under the cards, not a table of boxes. At 2.36:1 in the dark
theme the empty grid reads as the loudest element on the canvas.

### 2.5 A multi-value `location` filter does not survive the URL

`useDashboardParams.ts:9` lists `ARRAY_TYPES`, and `location` is multi-select
(`FilterPill.tsx:87`) but absent from it.

**The break is on the read side, not the write side.** An earlier draft of this spec said the
value is written with `String(value)` as a comma-join; that is wrong. Line `:32` reads
`typeof value === "object" ? JSON.stringify(value) : String(value)`, and `typeof [] === "object"`,
so a multi-value location is **already written as correct JSON**. What fails is `:21`: because
`location` is not in `ARRAY_TYPES`, the value comes back as the JSON *string* rather than an
array, `paramToFilters` emits `in ['["Hanoi","Da Nang"]']`, and the card returns **zero rows**.
The symptom is an empty card, not a truncated filter. The one-line fix is unchanged.

**A second defect in the same function, found while planning this.** `category` *is* in
`ARRAY_TYPES`, so every scalar category value is handed to `JSON.parse`: `?tags=42` reads back
as the **number** `42`. This is reachable today — both the drill "Filter by" path
(`DashboardView.tsx:754`) and the crossfilter click behaviour (`:236`) call `setValue` with a
scalar. The decode must be gated on the raw actually looking like an array.

**A third, latent, in the same hook.** `setValue` called in a loop drops all but the last write,
because each call reads the same location. Nothing does that today, but applying a batch of
draft filter values is exactly that shape, so W6's auto-apply work needs a batched setter before
it can be correct.

---

## 3. Capabilities the backend already has and no UI reaches

### 3.1 Fixed vs full width

The column exists (`migrations/0018_dashboard_width.sql`), the DTO carries it
(`crates/egret-analytics-api/src/bi.rs:368`), the PUT validates it
(`crates/egret-bi/src/routes.rs:2702-2706` — note the crate: `egret-analytics-api` has no
`routes.rs`), and the
frontend *renders* it — `DashboardView.tsx:178,462` applies `max-w-[1200px]` when
`width === "fixed"`. But **no control anywhere sets it**, and both save paths echo the stored
value back unchanged (`DashboardView.tsx:390,404`). Every dashboard is pinned to the server
default `"full"` (`bi.rs:411-413`).

Metabase: canvas is **1048px, centred** by default — measured live at an 1800px viewport, the
grid stayed 1048px wide at x=376. The toggle is a single "Full width" switch in the edit-mode
"…" menu (`ExtraEditButtonsMenu.tsx:17-46`).

**Decision.** Adopt Metabase's 1048px for the fixed measure (replacing egret's 1200px) and make
**newly created** dashboards default to `"fixed"`. Existing dashboards keep their stored value,
so nothing already built changes appearance.

### 3.2 Dashboard-level actions reachable only from the list page

`duplicate`, `trash`, `delete`, and `folder` mutations all exist in `api/dashboards.ts:121-136`
and are fully implemented server-side (`crates/egret-bi/src/dashboard.rs`), but `DashboardView`
imports none of them (`DashboardView.tsx:61` destructures only `update, restore, addBookmark,
removeBookmark`). To duplicate the dashboard you are looking at, you must navigate away to the
list.

Metabase's "…" menu on the dashboard page itself: Reset all filters, Subscriptions, Enter
fullscreen, Edit settings, Move, Duplicate, Move to trash
(`DashboardActionMenu.tsx:88-163`) — verified live.

### 3.3 `description` on create

`CreateDashboard.description` exists (`bi.rs:376-382`); the create dialog sends `{name}` only
(`Dashboards.tsx:73`).

---

## 4. Workstreams

Ordered by dependency. W1 must precede W2 — both edit `DashboardGridLayout.tsx`.

### W1 — Responsive grid

Port Metabase's breakpoint model verbatim (constants in §2.1). Concretely:

- extend `dashboard-grid.ts` with `GRID_BREAKPOINTS`, `GRID_COLUMNS`,
  `MOBILE_HEIGHT_BY_DISPLAY_TYPE`, `MOBILE_DEFAULT_CARD_HEIGHT`, and per-breakpoint margins
- `DashboardGridLayout.tsx` switches to RGL's responsive layout, deriving the mobile layout
  from the desktop one rather than storing a second layout
- **layout changes are persisted only when editing AND on the desktop breakpoint** — this
  guard is the whole point; without it, opening a dashboard on a phone rewrites its layout
- drag and resize off at the mobile breakpoint
- `AppShell.tsx` gains a viewport rule: below 768px the navigation sidebar is collapsed by
  default and does not consume layout width. This touches every route; the localStorage
  preference continues to govern desktop.

The auto-height `ResizeObserver` path (`DashboardView.tsx:354-366`, `AutoHeightMeasure.tsx`)
must not write `size_y` while on the mobile breakpoint, for the same reason as the layout
guard.

### W2 — Edit-mode safety and fidelity

- **Discard confirmation.** Cancel with a dirty draft opens a confirm dialog ("Discard your
  changes?" / Cancel / Discard changes). Clean drafts exit immediately, as Metabase does.
  Dirtiness compares the draft against the snapshot taken by `startEdit()`
  (`DashboardView.tsx:240-248`).
- **Undo toasts** for the three destructive edit actions Metabase makes undoable: remove card
  (`cards-typed.ts:472-523`), delete tab (`use-dashboard-tabs.ts:43-61`), replace card
  (`DashboardGrid.tsx:399-406`). Each restores the prior draft state; they are draft-level, so
  no server round-trip.
- **Grid stroke.** Bring the edit-guide contrast to Metabase's band. Target ≤ 1.5:1 against the
  dashboard canvas in both themes, verified by computing the composited value — not by eye.
- **Mapping control.** The filter→card overlay select spans the card width and carries the
  Metabase header text for its card kind. Cards that cannot accept the current filter render
  the reason instead of an empty select.

  ⚠️ **"The selected filter is on another tab." has no reachable precondition in egret**, and
  this spec was wrong to list it as a minimum. Metabase's trigger is its *inline-parameter*
  model — a parameter owned by a dashcard on a different tab
  (`DashCardCardParameterMapperContent.tsx:86-92`). egret's `DashboardParameter`
  (`api/dashboards.ts:54-65`) has no owning card and no tab; every filter is dashboard-level.
  And `visibleCards` is already tab-filtered (`DashboardView.tsx:188-192`), so a card from
  another tab is never mounted to be told anything. Build the branch, unit-test it, and leave it
  inert behind an explicit flag — W7 is what activates it. Do **not** invent a tab field on the
  parameter to make the string reachable.

  ⚠️ **Metabase's link-card hint would be false here.** `LinkCard.tsx:8,18` uses the raw `url`
  and `label` with no `interpolateParams` call, so "you can connect widgets to {{variables}} in
  link cards" would document a feature egret does not have. The reverse also holds: egret's
  **heading** card does interpolate (`DashboardCardView.tsx:104`) where Metabase's map gives
  `heading: ""`, so egret earns a hint Metabase has no reason to show. Write both strings from
  egret's behaviour, not Metabase's table. "Variable to map to" — Metabase's native-SQL template
  variables — has no egret analogue at all and is not ported.

### W3 — Fixed / full width control

- A "Full width" switch in a new edit-mode "…" menu, writing `width` through the existing save
  path.
- Fixed measure becomes 1048px.
- New dashboards default to `"fixed"` (server-side default change plus the create path).

### W4 — View-mode header parity

- Replace the native `<select>` auto-refresh with a popover menu matching Metabase's — heading
  "Auto Refresh", checkmark on the active row, the same seven values (already identical:
  Off/1/5/10/15/30/60 minutes, `RefreshWidget.tsx:12-55` vs `DashboardView.tsx:485-495`).
- **Manual refresh**, absent today — nothing but the interval bumps `refreshNonce`
  (`useDashboardViewControls.ts:27`).
- **Countdown indicator**: the trigger shows time to next refresh when a period is set
  (`RefreshWidgetTarget.tsx:33-54`).
- **"…" actions menu** on the dashboard page: Duplicate, Move to folder, Move to trash, Edit
  settings, Reset all filters. All mutations already exist (§3.2).
- **Keyboard shortcuts.** Port the subset that maps onto features egret has:
  `e` edit / cancel edit, `s` save, `o` bookmark, `]` info sidebar, `f` add filter,
  `a` add card, `1`–`9` select tab. Not ported: `q`/`n` (new question from dashboard — egret
  has no such flow), `$mod+backspace` (trash from the dashboard arrives with W4's menu, but a
  destructive shortcut without undo is worse than none).

### W5 — Card-level parity

- **Download formats.** Today CSV and PNG only (`ChartExportMenu.tsx:40-56`). Add `.xlsx` and
  `.json`, plus Metabase's "Keep the data formatted" toggle and its row-limit note. PNG stays
  conditional on the card rendering an `<svg>`.

  ⚠️ **The toggle's default is a behaviour change, not a no-op — this spec originally said
  otherwise and was wrong.** `rowsToCsv` (`export-chart.ts:18-34`) renames the *header* via
  `column_title` but writes every *cell* with a bare `String(v)`, calling `formatValue` nowhere.
  Today's CSV is a hybrid that Metabase has no equivalent of, so **neither** setting of the
  toggle reproduces it: ON formats the cells (a `1234` becomes `1,234`, since `formatNumber`
  builds with `useGrouping: true`, `format.ts:105`), OFF drops the renamed headers.
  **Decision: match Metabase — default ON.** The toggle exists precisely so that anyone
  consuming these files by machine can turn it off, and parity is this programme's whole
  premise. The PR must call this out as a user-visible change to an existing export.

  egret's export is entirely client-side (`export-chart.ts`), so `.json` is free but `.xlsx`
  needs a writer, lazily imported so it stays out of the main bundle. **Not npm's `xlsx`** —
  it is stale at 0.18.5 with known advisories. Use `write-excel-file`: pure ESM,
  `sideEffects: false`, one transitive dependency. Note its v4 API is
  `writeXlsxFile(sheetData).toBlob()`, not the v3 `{ fileName }` shape most documentation
  still shows.
- **"Edit question"** menu item, and the card title becomes a link to the chart, matching
  `getDashcardHref` (`dashboard/selectors.ts:228-248`). Suppressed when the title override is
  `""` (header hidden) and while editing.
- **Per-card description** (`card.description` in Metabase,
  `visualizations/lib/settings/visualization.ts:36-44`) surfaced as an info affordance on the
  card header.
- **`dashcard.background` toggle** — a card can drop its chrome, as Metabase's heading cards do
  by default (`Heading/index.ts:11-40`). egret's heading card already renders chrome-less
  (`HeadingCard.tsx`); this generalises it to any card.

### W6 — Filter and parameter parity

- **"People can pick": Multiple values / A single value.** egret hard-codes multiplicity by
  type (`FilterPill.tsx:87`); Metabase exposes it whenever the type/operator pair is in
  `SINGLE_OR_MULTI_SELECTABLE_TYPES` (`metabase-lib/v1/parameters/constants.ts:243-259`).
- **Drag-reorder of filter chips** (`ParametersList.tsx:56-64,90-104`; 15px activation
  distance). Today they render in creation order with no way to change it
  (`DashboardFilterBar.tsx:22-25`).
- **Auto-connect.** On mapping a filter to one card, offer to connect it to every card with a
  column of the same name — Metabase's toast wording and its 12s / 8s timeouts
  (`auto-wire-parameters/toasts.ts:26-91`, `constants.ts:1-2`). This is the single largest
  time-saver missing from egret's filter flow: today every card must be mapped by hand.
- **Auto-apply toggle + Apply toast.** A dashboard-level setting; when off, filter changes stay
  draft until Apply (`FilterApplyToast.tsx:44-71`). egret is auto-apply-only
  (`DashboardView.tsx:564`).
- **Linked filters panel** gains Metabase's framing — the "Limit this filter's choices"
  heading, the explanatory sentence, and the three blocked states (no usable parameter, custom
  value source, input-box widget) that today are silently an empty checkbox list
  (`ParameterLinkedFilters.tsx:52,93-103`).
- **Typed default-value widget** replacing the plain text input
  (`FilterSettingsPanel.tsx:100-105`), so a date default is picked with the date picker and a
  category default from the value list.
- **`location` URL round-trip** — add `location` to `ARRAY_TYPES` (§2.5).

### W7 — Inline parameters

Metabase can move a filter off the top bar and into a heading or question card
(`MoveParameterMenu.tsx:67-134`, `supportsInlineParameters`), with a collapse-to-"Show filters"
behaviour when the card is too narrow (`CollapsibleDashboardParameterList.tsx:40-86`).

This is the tail of the programme. It depends on W6's reordering model and is the one
workstream that can be cut without leaving anything half-built.

### W8 — Info sidebar and revision history

⚠️ **This spec originally called revision history "the only item needing backend work". All
three items need it**, and W8 needs **two** migrations rather than the one the plan index
reserved.

- **Owner.** `DashboardView.tsx:691` passes `dash.created_by` raw. Resolve it to a display name
  and show both created and last-edited, as Metabase does ("August 1, 2026 by Ref User").
  Backend work, for two reasons. `analytics.dashboards` has **no `created_at` and no
  `updated_by`** — nothing has been added since migrations 0015/0021 — so "created … by / edited
  … by" cannot be answered from the current row at all. And **there is no frontend
  user-id→name lookup to use**: `/api/control-plane/users` needs `user:read`,
  `/ws/{ws}/members` needs `workspace:manage_members`, and `/api/control-plane/principals`
  carries no `id`, so a `ws_editor` opening the sidebar on their own dashboard would 403.
  Follow what this repo already does in five places (`MemberDto.principal_name`,
  `ObjectGrantDto.principal_name`, the grant rollup, who-can-access, effective access): resolve
  the name **server-side with a LEFT JOIN** and add `created_by_name`/`updated_by_name` to the
  DTO, rendering `?? raw_id` as `ShareDialog.tsx:316` does.
  ⚠️ `ADD COLUMN created_at timestamptz DEFAULT now()` **backfills every existing row** on
  Postgres 11+, stamping every dashboard in the deployment with the migration's timestamp. Add
  the column bare, then `SET DEFAULT` in a second statement, and leave pre-existing rows NULL.
- **"Saved in"** folder link. Also backend: `DashboardSummaryDto` carries `folder_id` but
  `DashboardDetailDto` does not, so the dashboard page cannot know its own folder.
  ⚠️ **Folders have no route.** `App.tsx` has no folder path; a folder on the list page is a
  collapsible `<tbody>` header with its collapse state in `localStorage`. So this cannot link to
  a folder page — link to `/explore/{ws}/dashboards?folder=<id>` and teach the list page's
  collapse state to expand the linked section on arrival. Without that second half the link
  lands on a page where the dashboard you came from is invisible.
- **Revision history.** A revisions table, capture on dashboard PUT, plus list and revert
  endpoints, then Metabase's History tab with a timeline and revert
  (`DashboardInfoSidebar.tsx:225-265`, `actions/revisions.ts:11-35`).

Ordered last so it can be cut without stranding W1–W7.

### W9 — iframe card

`createVirtualCard("iframe")` with a URL allow-list (`IFrameViz.tsx:13-16`), min 4×3 / default
12×8. egret's card-type column is free text with no CHECK
(`migrations/0015_dashboard.sql:21`), so this is frontend-only.

---

## 5. Explicit non-goals

**Action button card — dropped from scope.** Metabase's action cards are the UI for its
model-actions/writeback subsystem. egret has none: a repo-wide grep for `writeback`,
`ModelAction` and `implicit_action` across `.rs`, `.ts` and `.tsx` returns nothing. Building
the card without the subsystem produces a button that cannot act.

**Subscriptions, public links, embedding.** Absent from egret's backend *and* frontend. egret's
sharing is grant-based (`ShareDialog`, principals + roles + expiry) and its auth model is
per-user token delegation, not signed anonymous links. This is a product decision, not a gap.

**Visualizer / multi-source card composition.** Already argued and declined during the chart
programme: egret's line is that a dashboard card may override *presentation*, never *data*.

**Collections vs folders, moderation, granular caching, Metabot/Insights.** Enterprise-gated in
Metabase, or a different architecture in egret.

---

## 6. Testing strategy

Every workstream lands with unit tests, but the programme's own history is unambiguous: **every
defect that mattered was found by measuring the running app, not by review and not by the test
suite.** So each workstream also carries a live check with a recorded number.

- **W1** — assert grid width, card width and `document.scrollWidth` at 390px and at 1440px on
  the harness dashboard. The mobile assertion is `scrollWidth === innerWidth` and card width
  within a few px of the container; the desktop assertion is that the saved layout is
  *unchanged* after a mobile visit.
- **W2** — the discard dialog is asserted by making a draft change, cancelling, and confirming
  the change survives a Cancel-the-cancel. Grid contrast is asserted by computing the
  composited stroke value against the canvas in both themes, as §2.4 does, not by snapshot.
- **W4/W5** — menu contents asserted by role and name.
- **W6** — the `location` fix gets a round-trip test (write multi-value → read back), since the
  bug is precisely that the value does not survive serialisation.

Tests must survive mutation. The house rule applies: stub the implementation and watch the test
fail before claiming it verifies anything. Six occurrences of an assertion that passed against a
stubbed implementation were found during the chart programme; three of them were tests that read
a *display* string where the code keyed on a *raw* one.

---

## 7. Risks

**W1 touches the app shell.** The sidebar breakpoint changes every route, not only dashboards.
Mitigation: the change is additive (a viewport rule that forces collapse below 768px) and the
existing localStorage preference continues to govern desktop, so no desktop behaviour moves.

**W6 is the largest workstream and the most user-visible.** Auto-connect in particular writes
mappings across many cards at once. It must be undoable — Metabase pairs its auto-connect toast
with a follow-up undo toast for exactly this reason (`toasts.ts:26-91`).

**W3 changes a default.** New dashboards become `"fixed"`. Existing rows are untouched, but the
migration must be read carefully: it changes the *column default*, not the existing values.

---

## 8. What "done" looks like

The harness dashboard, opened side by side at 1440px and at 390px, differs from Metabase only
in the items listed in §5. Specifically: it stacks to one column on a phone without horizontal
scroll; Cancel cannot silently destroy work; a filter can be connected to twenty cards without
twenty manual selections; and every dashboard-level action that the backend already implements
has a way to be reached from the dashboard itself.

---

## 9. Findings that did not survive inspection

Recorded so they are not re-derived.

- **Grid geometry is not a divergence.** `dashboard-grid.ts:3-8` matches Metabase exactly — 24
  columns, 10/9 aspect, 6px margins, `[0,0]` container padding, 40px minimum row height, and
  the per-viz default/min size table matches `dashboards/constants.cljc:11-37` entry for entry.
  The measured 184px card at 1136px container resolves to exactly `size_x: 4`. The port is
  faithful.
- **The per-card edit toolbar is not always-on.** It looked permanently rendered in the DOM,
  but its container is `opacity-0 transition-opacity group-hover:opacity-100` — hover-revealed
  on both sides. egret places it top-centre straddling the card edge, Metabase top-right
  inside; a placement difference, not a defect.
- **"Replace chart" clearing `options` is not a divergence.** egret resets `options` to `{}`
  (`DashboardView.tsx:274-277`); Metabase clears series, `parameter_mappings` and viz settings
  too (`cards-typed.ts:245-284`). Both discard. What egret lacks is the undo toast, which is
  why the behaviour felt worse — that is covered by W2, not by changing the reset.
- **Auto-refresh values already match.** Off/1/5/10/15/30/60 minutes on both sides. Only the
  control is different (native `<select>` vs popover), which is W4.
- **Section layouts already match.** Both ship three prebuilt layouts with the same shapes
  (`dashboard-sections.ts:36-77` vs `dashboard/sections.ts:202-224`).
