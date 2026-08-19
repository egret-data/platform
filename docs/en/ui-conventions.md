# egret UI conventions

How egret's frontend builds list pages, create flows, settings tables, and the tests that
guard them.

Every rule below cites the code that implements it and, where one exists, the defect that
motivated it. **A rule with neither is an opinion, and opinions belong in review comments
rather than in a standard.** Rules were cut from this document rather than written without
evidence.

It is written *from* what exists. Where a convention is only partly adopted, that is stated
rather than hidden — see [Adoption status](#adoption-status).

Paths are relative to `frontend/` unless noted. Line numbers drift; symbol names are the
stable reference.

---

## 1. The list page

### 1.1 `ListPageShell` owns the single `h1`

`src/components/explore/ListPageShell.tsx`

Every Explore list page is wrapped in `ListPageShell`. It supplies `p-4`, the page's one
`h1`, a `toolbar` slot, and an `action` slot. A page does **not** hand-write its own header.

```tsx
<ListPageShell title="Endpoints" toolbar={…} action={…}>
  …
</ListPageShell>
```

**Why it is a component and not a comment.** The `p-4` + `flex items-center justify-between`
header was written out by hand in all six Explore list pages, and three of them carried a
comment calling it "the canonical Explore list-page scaffold" — a convention documented six
times and enforced nowhere. That is recorded in the component's own doc comment.

**The defect.** In the phase before the shell existed, removing a layout wrapper silently
took a route's only `h1` with it, and no test noticed — the browser did. The guard is now
the first test in `src/components/explore/__tests__/ListPageShell.test.tsx`
(`"renders exactly one h1, and it is the title"`), which asserts the *count* is exactly one,
not merely that one exists. `EndpointDetail.tsx` — a detail route, not a list — carries the
same obligation and satisfies it with `<h1 className="sr-only">{headingName}</h1>` beneath a
visible `h2`, guarded by its own count assertion.

**The shell takes nodes, not capabilities.** `toolbar` and `action` are `ReactNode`. A shell
that knew about bookmarks, bulk mode and trash would grow one prop per capability, and the
pages differ in which they have.

**Three shell behaviours are load-bearing and tested, so do not "simplify" them away:**

- `flex-wrap` on the action group itself, not only on the header row. `Endpoints.tsx` puts
  three controls in that slot (bulk toggle, "Download OpenAPI", "+ Endpoint"), which
  overflows a narrow viewport on its own. Guarded by
  `"wraps the action group, not only the header row"`.
- The description `<p>` is **omitted entirely** when there is no description, not rendered
  empty. An empty paragraph still takes vertical space and pushes the table down on every
  page that has none. Guarded by `"omits the description element entirely when there is none"`.
- Vertical alignment is conditional: `items-center` without a description, `items-start` with
  one. Guarded, and the test deliberately asserts only the alignment class rather than the
  whole `className`, so it does not break on unrelated class changes.

### 1.2 Control order in the header

`ListPageShell` renders `{toolbar}` then `{action}` inside one
`flex flex-wrap items-center gap-2` group on the right of the header. So: **view controls
first, primary action last.**

`src/routes/explore/dataapi/Endpoints.tsx` is the reference:

- `toolbar`: `BookmarkToggle`, `BulkToggle`, `FolderToolbar` — in that order.
- `action`: `Download OpenAPI` (secondary, `variant="outline"`), then `+ Endpoint` (primary).

Toolbar **filters** are rendered **only when `hasData`**: a filter or a bulk mode over zero
rows is a control that cannot do anything.

**A view SWITCH is not a filter, and is not gated.** `Endpoints.tsx`'s Trash toggle renders
whatever `hasData` says, because it does not filter the rows in front of the user — it
selects a different server query (`GET /endpoints` vs `?trashed=true`), and an empty endpoint
list says nothing about whether the trash is empty. Gated on `hasData` it would also be the
only way *out* of an empty trash, removed exactly when it is the only control left. The guard
is `"keeps the way OUT of an empty trash"` in
`src/routes/explore/dataapi/__tests__/Endpoints.test.tsx`.

Everything the trash view does not render is cleared on the way IN, not merely hidden
(`toggleTrash`): the bookmark filter, both facets and bulk mode. A filter left engaged behind
a control that is no longer on screen makes the trash report "No endpoints match the current
filters" over rows that are genuinely there, with nothing visible to un-set.

### 1.3 Toolbar icon buttons come from `ListToolbarButton`

`src/components/explore/ListToolbarButton.tsx`

The `Tooltip` + `TooltipTrigger asChild` + icon `Button` + `TooltipContent side="bottom"`
cluster was written out four times per page across `Datasets.tsx`, `Charts.tsx`,
`Dashboards.tsx` and `Alerts.tsx`, and the copies had already drifted: some passed
`variant={active ? "secondary" : "outline"}` and some did not, and **none set `aria-pressed`**,
so a screen reader could not tell an engaged filter from an idle one.

Two consequences encoded in the signature:

- `label` is **required**, not optional. The button's only visible content is an icon, so
  without it the control has no accessible name at all.
- `filled` is separate from `active`. Bookmark and trash fill their glyph when engaged;
  bulk and folder do not. That is a per-icon decision, not a consequence of being active.

### 1.4 Search sits in a page toolbar above the table

`src/routes/explore/dataapi/Clients.tsx` (`data-testid="page-toolbar"`)

Search goes in a toolbar row above the summary strip, where the five sibling Explore pages
put theirs. `FilterableTable`'s own search box renders *below* the strip, inside the table
block — the one place no other list page has it — so pages pass `hideSearch` and filter
`rows` themselves. Two search inputs one row apart would be worse than either.

This has a knock-on that is easy to get wrong and is commented in place: with `hideSearch`,
`FilterableTable`'s internal `q` never leaves `""`, so it can no longer distinguish a
first-run empty from a search miss and always renders `emptyText`. That is correct **only
because** the genuine first-run case is `ListStates`' `empty` and never reaches the table.
See §2.

---

## 2. Loading, empty, error — three states, never two

`src/components/explore/ListStates.tsx`

Every list opens with a four-way branch, in this order:

```text
isLoading  →  skeleton
isError    →  destructive Alert
!hasData   →  the empty state
otherwise  →  the rows
```

### 2.1 `isError` is checked before `hasData`

Written out as a nested ternary in all six pages, they had already drifted: `Dashboards.tsx`
checked `hasData` before `isError`, so a failed refetch that still held cached rows rendered
the table and swallowed the error, while the other five rendered the alert.

### 2.2 Error may never fold into empty

This is the rule the component exists for.

**The defect.** Two Data API service-account pickers collapsed the two branches into one. A
failed fetch rendered the "this workspace has no service accounts yet" copy while Run sat
disabled — a dead end *with a false explanation*, and the operator went off to create a
service account that already existed.

Both are now explicit and commented in place:

- `src/routes/explore/dataapi/detail/TryIt.tsx` — `accountsQuery.isError` renders a
  destructive Alert; only the non-error path falls through to the "no service accounts yet"
  hint.
- `src/routes/explore/dataapi/detail/AccessSection.tsx` — `accountsQuery.isError ||
  grantsQuery.isError` precedes the `accounts.length === 0 && orphanGrants.length === 0`
  branch.

The same shape recurs anywhere a query's result is used to describe the world.
`src/routes/explore/dataapi/detail/ResponseSection.tsx` guards it twice over: react-query
clears `isLoading` on **any** settle, error included, so a failed version fetch for a live
endpoint would fall straight into the `!liveVersion` branch and read as "never published" to
an author who has already published. It therefore computes `columnsErrored` separately from
`columnsLoading`, and both are keyed on `endpoint.live_version_id` so a genuinely
never-published endpoint is not misreported either.

### 2.3 "Empty" means the unfiltered list is empty

`Clients.tsx` computes `hasData` from the **unfiltered** count, with the reason in the code:
the first-run call-to-action must not appear just because a search missed. A search that
narrows to zero is the table's own empty body, which is a different sentence for a different
situation.

### 2.4 The empty state itself

`src/components/explore/EmptyState.tsx`. Use `variant="dashed"` for a first-run page block
and the default `plain` inside `FilterableTable`'s `emptyText` slot. It is one component with
a variant rather than two components because the content is identical and only the frame
differs.

---

## 3. Create flows: when a page, when a dialog

**A create flow becomes a page when the thing it creates has a URL worth linking to, or when
it produces something the user must not lose to a stray click.** Otherwise a dialog is still
right.

All three Data API creates are routes, registered in `src/App.tsx` with the reasoning written
above each one:

| Route | Component | Why a page |
|---|---|---|
| `data-api/endpoints/new` | `EndpointCreate` | The created endpoint has a detail URL; the form can be linked, bookmarked and reached with Back. Navigates **into** the created endpoint. |
| `data-api/clients/new` | `ServiceAccountCreate` | Same, except a service account has no detail route — it is a row in the Clients table — so this one navigates **back to its list**. |
| `data-api/clients/:accountId/new-client` | `ClientCreate` | The response contains a secret shown **once**. |

### 3.1 The one-time secret is the strongest case

`src/routes/explore/dataapi/ClientCreate.tsx`

As dialogs this was a modal opening on top of another modal: the issue dialog was unmounted
while still open as the secret dialog mounted — the classic stuck-`pointer-events` shape. The
secret now lands on the same route in a **created** state, which cannot be dismissed by an
outside click or an accidental `Escape`.

The page has three states, and the third is the point: **form**, **created** (the secret, in
place, with one copy affordance and a "Done" link and deliberately no navigation beside it),
and **spent** — this tab issued a client but no longer holds the secret, e.g. after a reload.
Spent says so plainly and offers the way forward (revoke and reissue) rather than rendering
blank. The commit message says outright: do not "improve" this by caching the secret to
survive a refresh.

### 3.2 A dedicated component, or the detail component with no id?

Measured precedent, both real:

- `control-plane/engines/new` → `EngineCreate` — a dedicated create page.
- `charts/new` → `ChartExplore` and `alerts/new` → `ScheduleDetail` — the detail component
  reused with no id.

The reuse works for charts and alerts because those objects have no id-dependent
sub-resources. It would not have worked for endpoints: `App.tsx` registers **six** sections and
every one of them reads the id (Query's Preview `POST`s to `…/endpoints/{id}/preview`; Response,
Access, Try it, Versions and Config all load the saved row — Preview is part of `Query`, not a
seventh section), so reusing that shell would have shipped a page
whose Preview cannot run and whose right-hand tabs are meaningless until the first save.
`EndpointCreate` is therefore its own component.

### 3.3 Entry points are links, not imperative pushes

`Clients.tsx` and `Endpoints.tsx` both use `<Button asChild><Link …>`. A button that pushes
the route looks identical on screen but cannot be middle-clicked, opened in a new tab, or
copied.

### 3.4 Write `new` before `:id` — as a convention, not a fix

Do it for readers. It is **not** load-bearing, and the comments in `App.tsx` say so, because
they used to claim the opposite. Measured on the installed react-router-dom (7.18.2): with
`:id` registered first, `/endpoints/new` still matches the static route, because v7 ranks
segments by specificity rather than by declaration order. The false claim was in two places —
the new Data API comment and the pre-existing `alerts/new` one — and both were corrected.

### 3.5 A create page reuses `ListPageShell`

`EndpointCreate`, `ServiceAccountCreate` and `ClientCreate` all render inside `ListPageShell`
with a `title` and a `description` — so the create page gets the same one-`h1` header and
padding as the list it came from, and §1.1 holds for it too. The shell's `description` slot
exists for exactly this.

### 3.6 On success: invalidate and no toast — navigate unless the response *is* the point

`EndpointCreate` and `ServiceAccountCreate` do all three, and `control-plane`'s `EngineCreate`
is the precedent they cite. The hand-off **is** the confirmation — the author lands either on
the new object's editor with the name they typed in the header, or on the list holding the row
they just created — and a toast on top of that reports what is already on screen.

**`ClientCreate` invalidates and deliberately does not navigate**, which an earlier draft of
this section got wrong by saying "all three creates do this". It renders the created state in
place, because the response carries the client secret and that is shown exactly once (§3.1) —
a developer who followed the old wording here would navigate away and destroy the secret. So
the invalidate-and-no-toast half is universal; the navigate half applies only where the
response holds nothing the author still needs to read.

The dialog needed a toast because it closed over a list that looked unchanged. A page does
not. (The invalidation runs before the navigation, so the row is already there when the list
renders.)

### 3.7 On failure: clear nothing

Also in all three: the server's own message goes inline above the form, and **every field
keeps its value**. A dialog could be dismissed and reopened; a page that resets itself on
failure loses work the author cannot get back.

### 3.8 These forms carry no draft persistence

Stated in the route comments so nobody reports it as a bug: the forms are plain component
state and a reload empties them. For `ClientCreate` this is deliberate — the worst outcome of
not remembering is a worse first-run form; the worst outcome of remembering is a leaked
secret.

---

## 4. `Setting | Details | Value` tables, and when a card is still right

`src/components/config/ConfigTable.tsx` — `ConfigTable`, `ConfigTableGroup`, `ConfigTableRow`.

A group of settings is a **table**, not a stack of cards. Three columns: the setting's label,
muted help text, and the live edit control. There is no view/edit toggle — the surface is a
single editing surface with one Save.

### 4.1 Groups are rows, not cards

`ConfigTableGroup({ label })` renders a full-width heading row inside the table.

**The defect.** The Data API detail sections used to stack one `ConfigSection` card per
group — `Endpoint` framing a hand-written four-field form, `Cache` three table rows, `Limits`
two, `Rate limit` three. Read down the page, that was a column of mostly-empty boxes.
`src/routes/explore/dataapi/detail/Config.tsx` is now **one** table with four
`ConfigTableGroup` rows (`Endpoint`, `Cache`, `Limits`, `Rate limit`), and the identity fields
(name, description, method, path) are ordinary rows, so every setting on the tab reads the same
way and the Details column can explain them. Those fields were *inside* the first card, not a
separate form above the cards, as an earlier draft of this section said.

Two details of `ConfigTableGroup` are load-bearing:

- It spans `colSpan={3}`. A group row occupying only the Setting column would read as a
  setting with no value, which is the confusion the cards were replaced to remove.
- `role="rowheader"`, and that alone. The cell also carried `scope="colgroup"`, which the fix
  wave removed: `scope` is a `<th>`-only attribute and is inert on a `<td>`. The branch's own
  mutation had already said so and it was misread as proving the pair — dropping `role` while
  **keeping** `scope` kills four tests, so `scope` was carrying none of the meaning. It stays a
  `<td role="rowheader">` rather than becoming a `<th scope="colgroup">`, because a `<th>`
  brings its own implicit role to argue with.

One column, deliberately. A two-column grid pads the short groups to the height of the tall
one beside them, and CSS `columns` fights the flex container the page sits in and clips the
form at the fold. Both were tried on this editor and both were wrong.

### 4.2 When a card is still right

`src/routes/explore/dataapi/detail/ResponseSection.tsx` keeps two cards deliberately, and its
doc comment records why:

- **Frozen columns** — a read-only `Name | Type` listing of the *published* contract. There
  is no Value to edit. Forcing it into the shared table would either fabricate a Value column
  or nest a table inside a cell, and would make read-only output look editable.
- **OpenAPI document** — a paragraph, a download button, and a derived base URL. An action,
  not a setting.

Only `Output shape` was config-shaped, and only it became a `ConfigTableGroup` + rows.

**The rule:** the complaint that removed the `ConfigSection`s was about cards framing
near-empty tables, not about cards. A read-only listing is not a setting, and an action is
not a setting.

---

## 5. Width

### 5.1 No page-level cap on a route that hosts settings tables

`src/routes/explore/dataapi/EndpointDetail.tsx` — the `<Outlet/>` wrapper carries no
`max-w-*`, and there is a test that says so.

**The defect.** That wrapper used to carry `max-w-3xl`, which left the right third of the
page empty while Config's Details column wrapped to three lines. A single page-level cap is
the wrong instrument: the six sections do not want the same width. Config and Query should use
the page — Config holds the two widest controls of any tab, Query a SQL editor and a result
grid; Access, Try it, Versions and Response are prose, short-column lists and short controls
that read worse stretched across a wide monitor.

**So sections own their own caps**, on their own root element, visible next to the layout they
constrain: `AccessSection.tsx`, `TryIt.tsx`, `Versions.tsx` and `ResponseSection.tsx` all use
`max-w-5xl`, and `ResponseSection.tsx`'s frozen-column table adds `max-w-xl`. Query needs none
and gained the most —
its SQL editor went from 768px to 1424px (measured during the width pass, not by this
document; the same applies to the figures in §5.4 and §5.5).

### 5.2 Removing the cap is not enough, because `ConfigTable` is `table-fixed`

This is why width here is not a matter of taste. Under `table-layout: fixed`, a column is
exactly as wide as its `<th>` says. `ConfigTable` sizes Setting (`w-72`) and Value
(`valueClassName`, default `w-52`) and leaves **Details** unsized — so Details is the only
column that can absorb surplus, and it absorbs *all* of it. Removing a page cap therefore
hands 100% of the new width to muted help text and leaves the controls exactly where they
were.

Measured, before the width pass: at `w-52` (208px), Config's Description (a `rows={2}`
Textarea) showed about 50 characters and its Path input about 25 before scrolling. Config now
opts into `valueClassName="w-52 lg:w-96"`; measured after, both render 368px at a 1728px
viewport.

Two consequences:

- **The width knob must live on the component, not in the caller's cell.** A `w-96` wrapper
  inside a `w-52` cell overflows into the neighbour instead of widening the column. Hence
  `valueClassName` on `ConfigTable`.
- **Cap the prose, not the column.** `detailsClassName="max-w-prose"` wraps the Details
  *content*, so the column may keep its width while the sentence stops at a readable measure.

Both props default to exactly what every prior consumer already rendered — `w-52`, and no
wrapper element at all — see §7.

### 5.3 The `lg:` in `w-52 lg:w-96` is a measurement, not a hunch

Below `lg`, Details is `hidden` but the two sized columns do **not** shrink to fit:
`table-fixed` honours their pixel widths and the shadcn `Table` wrapper (`overflow-x-auto`)
scrolls instead. At a 500px viewport the content box is 356px, so the table already scrolled
140px at `w-52`; a flat `w-96` would have pushed that to 316px. Gating the widening at `lg`
leaves the narrow regime exactly as it was.

### 5.4 A dead column below `lg`, introduced by this phase

Recorded so nobody "fixes" the widths without knowing about it, and documented in full in
`Config.tsx`'s comment. `ConfigTableGroup` spans `colSpan={3}`, which keeps the table three
columns wide even where Details is `display:none` — so fixed layout hands the leftover to a
phantom third column with no first-row cell, and it lands as dead space to the **right** of
Value. Measured at a 1010px viewport (content box 706px): columns 288/0/208 with 210px dead;
deleting the four group rows from that same live table gives 410/0/296 and no dead space.

**It is not pre-existing, and three places in this document used to say it was.**
`ConfigTableGroup` is new in this phase and is the only source of `colSpan={3}` inside a `ConfigTable` in the tree:
measured at the merge base (`63a4fb90`), no file importing `ConfigTable` contained a `colSpan`
at all — so relative to `main` there was no phantom column, on any page. The narrower thing
that *is* true: the group rows landed in the earlier tasks, so the column was already there
when the **width** pass measured, and the width pass did not create it. The measurement one
paragraph above is its own disproof — delete the four group rows and the dead space goes away.

The width pass does enlarge it in one sub-regime, and that depends on the sidebar being a
user-toggled rail (`w-56` expanded, `w-16` collapsed): at 1010px collapsed, 370px dead where
the old cap gave 272px (measured during the width pass). The fix is to make Value the
unsized column below `lg`; it is deferred because it is **unverified** that an unsized Value
absorbs the phantom column rather than splitting with it.

`ResponseSection` has the same phantom column, from its one `Output shape` group row.

### 5.4a The section that fell between two tasks

`ResponseSection` was built before there was a route cap to remove, and the width pass measured
`Config` plus the tabs it rebuilt — Response was in neither set, so it shipped uncapped. Measured
at a ~1470px content box: Setting 288 / Value 208 / Details ~975, with `max-w-prose` stopping the
sentence near 585px — ~390px of empty column, and the Shape and Key-naming Selects ~1260px right
of the labels they answer to, where the old route cap put them at ~560px. That is the same
complaint that earned `Versions` its cap, so the fix wave gave Response the same `max-w-5xl` root
(~816px), which also reins in the `Frozen columns` and `OpenAPI document` Cards — the frozen
*table* had `max-w-xl`, its Card did not.

The general rule: **a task that rebuilds a surface and a task that measures widths must agree on
the same list of surfaces**, or the difference between the two lists ships.

### 5.5 What the width test can and cannot say

`src/routes/explore/dataapi/__tests__/EndpointDetail.test.tsx`, `describe("EndpointDetail — width")`.

The route-cap guard mounts a **sentinel** (`<div data-testid="section-content"/>`) as the
nested route and walks up from it, asserting no ancestor up to the route root carries a
`max-w-*`. Two earlier shapes were wrong in opposite directions and both are named in the
test:

- `container.querySelector(".max-w-3xl")` scanned the whole subtree, forbidding one class name
  to an entire page — but a section may legitimately cap its own children.
- Walking up from a *real* section's content and stopping at "the child of the route
  container" reads the wrapper being hunted **as** the section root whenever it exists.
  Measured during the width pass: a restored `max-w-3xl` around the `Outlet` passes 31/31
  under that shape. The
  guard would have been dead in exactly the case it was written for.

The sentinel makes the contradiction impossible by construction rather than by picking the
right starting element.

The sentinel's strength is also a hole, and the fix wave closed it. Being scoped to the route
by design, the guard says nothing about what a section does to itself — so re-applying
`max-w-3xl` to **`Config`'s own root**, the exact defect this phase exists to remove, passed the
sentinel guard and every other Config test: 230/230 green. `Versions` had a guard saying it
*owns* its cap; `Config`, the tab the whole width pass was motivated by, had no counterpart. It
has one now — the inverse assertion, that Config's own root carries no `max-w-*` (responsive
prefixes included) — and it was mutation-checked with that exact mutation, which now fails one
test and only that one. `Response`'s new cap is pinned the same way as `Versions`'s.

The test's own comment is honest about its limits: the class-name assertions are weak and are
meant to be — they exist only so a cap cannot come back, or vanish, silently. **Whether the
width is well spent is a question only a browser can answer.** See §10.

---

## 6. `ConfigTableRow.details` is a plain string

No markdown. No backticks. They render as literal backtick characters.

The prop is typed `details?: string | null` in `ConfigTable.tsx`, and `null` explicitly means
"force an empty Details cell even though this field has `fieldHelp`", distinct from omitting
it (which falls back to `fieldHelp[fieldKey]`).

**Do not widen the prop to `ReactNode`** to make markup possible. This was found in a browser,
not by a test, and it is listed among the browser-only defects in
`docs/en/plans/2026-08-17-data-api-p6-authoring-loop.md` and
`docs/en/plans/2026-08-18-explore-list-primitives.md`.

A practical note from the rebuild: a byte scanner for backticks will fire on JSX **code
comments** near the copy (`unlike \`location\``) — a false positive. Check the string
literals, not the file.

---

## 7. Changing a shared component: additively, and measure by import path

### 7.1 Additive only

Measured on the current tree:

| Component | Importers |
|---|---|
| `@/components/config/ConfigTable` | **14** (13 production + its own test) |
| `@/components/form/ConfigSection` | **13** |

So `ConfigTable`'s two width knobs are opt-in with defaults chosen to leave every existing
consumer byte-identical: `valueClassName` defaults to `"w-52"` — the exact literal previously
hardcoded — and `detailsClassName` defaults to `undefined`, which emits **no wrapper element
at all**. Eleven other consumers render that Details cell today and none of them were
measured, which is the reason the default emits nothing rather than something reasonable.

The strongest form of "additive" is a diff whose file list proves it. When `ConfigTableGroup`
was added, only `ConfigTable.tsx` and a new test file changed, and `ConfigTable` /
`ConfigTableRow` were byte-identical — so no consumer *could* be affected, without needing to
reason about any of them.

### 7.2 Measure by import specifier, never by symbol name

`grep -rl "ConfigSection"` is a **substring** search. On this tree it returns 19 files;
`grep -rl 'from "@/components/form/ConfigSection"'` returns 13. The six extras are worth
naming one by one, because "doc-comment mentions" flatters them:
`src/components/form/ConfigSection.tsx` is the component's own definition;
`src/routes/control-plane/StarRocksConfigTab.tsx` does **not** import `ConfigSection` at all —
it defines its own local `FeConfigSection` and `CnConfigSection`; `ConfigTable.tsx` carries the
one real `{@link ConfigSection}`; and `Config.tsx`, `ResponseSection.tsx` and
`EndpointDetail.test.tsx` mention it in plain prose, describing what they replaced. The
substance holds — the substring grep over-counts by six — but exactly one of the six is a
`{@link}`.

This number was wrong three times, and each way of being wrong is worth recognising:

1. **Guessed** — "nine Control Plane usages", asserted and never measured.
2. **Mismeasured** — the substring grep above, giving 16/15. *This is the instructive one: it
   looked like diligence because it cited a command.*
3. **Unstated arithmetic** — a comment then said "12 other files", true only after silently
   subtracting the file itself and the component's own test from 14.

Use the import specifier — the thing that actually creates the dependency — and state what
you counted.

---

## 8. An assertion that a hardcoded implementation would also pass, pins nothing

**Not a UI rule. It is the most expensive thing this programme learned, and it generalises
past this codebase.**

Roughly ten fix-round commits across five phases went to this single class of defect; by one
branch ledger's own count, seven of them before an implementer finally audited every field for
it up front. Every instance below is real, and every one was caught downstream rather than by
the person who wrote the assertion.

### The check

**Before writing an assertion, ask what constant would satisfy it.** If a hardcoded value, a
fixed id, or a `return null` would pass, the assertion pins nothing. Then arrange the fixture
so that no constant can.

### The cases

| The fixture / assertion | The constant that passes it |
|---|---|
| **A one-row list.** | A star, a label, or an id hardcoded to *that* row looks identical on one row and is wrong on every other. It cost this project enough rounds that the reason is now written into the code: `Endpoints.tsx`'s `toggleBookmark` doc comment, `__tests__/BookmarkToggle.test.tsx`'s `"gives two rows two different accessible names"`, and `crates/egret-dataapi/tests/bookmarks.rs`. |
| **An empty folders list.** SQL Lab's saved tab short-circuits with `folders.length === 0 ? <flat list> : groups.map(…)`, so orphaned rows were already visible on a workspace with no folders. The bug only bit once at least one folder existed. | A grouping that drops rows into a bucket the output loop never reads — the defect `groupByFolder` in `src/components/explore/folders.ts` now documents and guards. (That asymmetry — same data, visible or invisible depending on whether an *unrelated* folder exists — is itself evidence it was never intended.) |
| **One user.** No test ever had a second user with a row to destroy, so **both** predicates on `remove_bookmark`'s `DELETE` were free. | A `DELETE` missing `user_id = $1`. Closed by one fixture where A and B both star the same endpoint and A deletes; the mutation now fails on *"B's star on the same endpoint must survive A's DELETE"* alone (`crates/egret-dataapi/tests/bookmarks.rs:463`). |
| **One database in a picker.** | `database_id = databases[0].id`. Closed by a two-database fixture that chooses the second. |
| **A test value identical to the field's own placeholder.** All five tests typed `"reporting-bot"`, which is the input's `placeholder`. | `name: "reporting-bot"` hardcoded into the request body — it passed all five. Closed by making **no test that asserts `name` in the body** type the placeholder, and no two of them type the same value; the reason is written at the top of `__tests__/ServiceAccountCreate.test.tsx`. (Tests that assert nothing about the body still type it, so the rule is about the asserting tests, not about the file.) |
| **An enum where you assert the non-default value.** Picking POST and asserting POST *still* passed a body hardcoded to POST. | Asserting the non-default does not make a two-value enum falsifiable — **it only moves which constant is unfalsifiable.** Both values must be pinned: one test picks POST and asserts POST, another never touches the picker and asserts GET. Only an implementation that reads the control passes both. |
| **A test named for a relationship whose assertions never reach the related thing.** `"keeps ordinary rows in their three columns"` survived `ConfigTableGroup → return null`; it only re-confirmed pre-existing `ConfigTableRow` behaviour. | Anything. This is the hardest to see, because the *name* describes an interaction and each assertion looks reasonable in isolation. The fix was not a third assertion beside two that pass, but making the existing ones **depend** on the group row — `getByRole("rowheader")` as a precondition, plus `nextElementSibling` for adjacency, a property `return null` cannot satisfy. |

A near-miss of the same shape, caught before commit: an assertion of "one table, four group
rows, in order" passes a Config page with `Name` filed under `Cache`. Only a row-**membership**
assertion (`endpointAt < fieldAt < cacheAt`) dies under that mutation.

### Corollary 1 — a mutation that does not compile proves nothing

Three separate times in this programme, a mutation's first cut failed to build: `TS6133`
unused parameter under `noUnusedParameters`, and an import left unused. **A mutation that does
not build looks exactly like one where no test died.** "Nothing failed" is then an artifact of
a typo, not evidence.

So: **build the mutation first, confirm zero errors, and only then read the test output.** Re-cut
it if it does not build — `(_label)` instead of `(label)`, or `{false && …}` to keep an import
live while rendering nothing — and say why the re-cut is equivalent in force.

### Corollary 2 — a surviving mutation is two very different things

Either the assertion is **vacuous**, or the mutation is a **genuine equivalence** (it does not
actually change behaviour). These demand opposite responses — strengthen the test, versus cut a
better mutation — and must be told apart before either is done. A prescribed fix in this
programme would have shipped a *dead guard* precisely because a surviving mutation was
attributed to the wrong one of the two; see §5.5.

### Corollary 3 — a reported green is a claim

When a mutation result matters, re-run it rather than accepting the report. This programme did
exactly that twice, once explicitly *"because this is the second time this specific test's
coverage has been mis-assessed"*, and once by reproducing a cross-wiring mutation to confirm
`path` was absent and `name` carried the garbled value.

---

## 9. Measure, do not recall

Seven claims in this programme asserted a state of the world without measuring it. Each was
caught only downstream.

- A field was reported **missing** when it already existed (`folder_id`).
- A component was reported **unused** — `EmptyState`, which **ten files** import today
  (measured: `grep -rl 'components/explore/EmptyState'`).
- A permission entry was reported **absent**, blocking folder creation. It had shipped
  **months earlier**: `kind_perms` at `crates/egret-bi/src/routes.rs`, dated by `git log -S`,
  with migration `0122` seeding `data_api_endpoint:read` and `:manage` onto `ws_admin`.
- A consumer count was guessed, then mismeasured, then quietly rounded — §7.2.

The false belief was also **written into the codebase**, in three doc comments asserting
endpoint folders were not creatable. Two were fixed and a third was found later — the one a
reader actually lands on, because it sat *on* the line it explained. One of them used the
false belief as the **justification** for real behaviour (passing `folder_id: None` to the
PDP). That is the dangerous shape: correct code resting on a reason that is no longer true.

**Asserting absence is the specific failure mode.** "There is no X" is a claim about the whole
tree and cannot be recalled — it has to be run.

---

## 10. Reading the code does not settle layout

Five successive accounts of one table's behaviour below the `lg` breakpoint were wrong, each
corrected by the next. Only a browser measurement settled the geometry — and a `git grep` at the
merge base settled the attribution.

1. Two implementers inferred from the classes that the Value column grows below `lg`.
2. A third opened a browser: **wrong** — and incidentally found that a `max-w-md` just added
   to two `SelectTrigger`s was a **no-op**, because shadcn sets `w-fit`.
3. A reviewer built a repro in Chrome: **both earlier accounts had the mechanism wrong.** The
   surplus goes to a phantom third column with no first-row cell (§5.4) — not a wider Value,
   and not a scroll.
4. The implementer reproduced it on the **real** table (288/0/208, 210px dead at 1010px) and
   added that the dead column is pre-existing.
5. The reviewer narrowed it once more: the measurement holds only with the sidebar rail
   **expanded**. Collapsed (`w-16`), the old cap did bind, and the pass widens the dead column
   272px → 370px there.
6. The final whole-branch review found the **label** wrong, after five rounds had accepted it:
   "pre-existing" was true only relative to the width pass. `ConfigTableGroup` — and with it the
   only `colSpan={3}` inside a `ConfigTable` — is new in this branch, so relative to `main` the dead column
   is this branch's own (§5.4). Nobody re-measured the claim that needed a `git grep`, not a
   browser, and the document contained its own disproof one sentence earlier.

Practical rules that fall out of this:

- **jsdom cannot judge layout.** A class-name assertion pins that a class is present, nothing
  about the resulting geometry. Say so in the test, as §5.5 does.
- **Check the instrument first.** Twice in this programme a running dev server was older than
  the built binary (once by 66 minutes, once by 9 hours). Without rebuilding, the check would
  have measured nothing — and in one case every click under test would have 404'd.
- **Name the viewport and the sidebar state** with any measurement. "1010px" alone was
  ambiguous enough to make a correct claim look wrong.
- **A tab whose Details column is `hidden lg:table-cell` behaves in a different shape below
  1024px.** Two viewports at 1440px and 1280px are the *same* regime; a browser check needs a
  third below `lg`.

---

## Adoption status

Honest scope, measured on the current tree, so nobody reads this as describing code that does
not exist yet:

- `ListPageShell` — 5 route files (`Endpoints`, `Clients`, `EndpointCreate`,
  `ServiceAccountCreate`, `ClientCreate`), all Data API. The other Explore list pages still
  hand-write their headers.
- `ListStates` — 2 route files (`Endpoints`, `Clients`).
- `EmptyState` — 10 files (2 Data API, 8 control-plane).
- `ConfigTable` — 13 production files; `ConfigTableGroup` is used by Data API's `Config` and
  `ResponseSection`.
- `ConfigSection` — 13 files, down from **15** at the merge base. This is the fifth count of
  this number in the programme and the first measured at both ends:
  `grep -rl 'from "@/components/form/ConfigSection"' src` gives 15 at `63a4fb90` and 13 at
  `HEAD`. 14 was a mid-branch state, quoted as if it were the base. If further sections convert
  to group rows, revisit whether `ConfigSection` still earns its place.

Bringing a page onto these primitives is a change to that page. **The primitives themselves
change additively** (§7).

## Sources

- Components: `frontend/src/components/explore/`, `frontend/src/components/config/ConfigTable.tsx`,
  `frontend/src/components/form/ConfigSection.tsx`
- Reference pages: `frontend/src/routes/explore/dataapi/`
- Routes and their rationale: `frontend/src/App.tsx`
- Design specs: `docs/en/specs/2026-08-18-data-api-frontend-rework-design.md`
- Plans carrying the browser-check discipline:
  `docs/en/plans/2026-08-18-explore-list-primitives.md`,
  `docs/en/plans/2026-08-17-data-api-p6-authoring-loop.md`
