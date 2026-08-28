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

`src/routes/explore/dataapi/Activity.tsx` and `ActivityCallList.tsx` are the second
application, and they read the rule twice over. **The 1h/24h/7d/30d strip is a view switch**:
it selects the window the whole page is about, and it is how a reader leaves a range that
turned out to be empty. **So is "Include trial runs"**, which is less obvious and matters
more. `crates/egret-dataapi/src/activity.rs`'s summary query ends `AND al.actor_user_id IS
NULL`, so trials are excluded from *every number the page shows* — including the one the
filter gate is computed from. A workspace whose only traffic was Try-it runs therefore reads
as an empty range, and gating that toggle would delete the one control that can reach rows
that exist: both halves of the trash argument, verbatim. It lives in the section's heading
row rather than in the gated filter row for exactly that reason, and the guards are
`"keeps the one way to reach trial runs, which the summary never counts"` and
`"can be turned back off from a trial view that found nothing"`.

**The gate's input is the unfiltered truth, and "not measured yet" is not "empty".** Activity
gates on `rangeIsEmpty` — *the summary has settled and says the range produced nothing* — not
on `!hasCalls`, which is also false for the whole of every reload. `range` is in the summary's
query key, so gating on the weaker predicate blinks the filter row out of existence on each
range switch. Guarded by `"does not blink the filter row out while a range switch is in
flight"`, which types nothing first: an engaged filter holds the row up by itself and hides
the bug.

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

All four pages named above were converted by package U on 2026-08-22, so `aria-pressed` is
now set on every list toolbar toggle in Explore except `Notebooks.tsx`.

### 1.4 Search sits in a page toolbar above the table

`src/routes/explore/dataapi/Clients.tsx` (`data-testid="page-toolbar"`)

Search goes in a toolbar row above the summary strip, where the five sibling Explore pages
put theirs. `FilterableTable`'s own search box renders *below* the strip, inside the table
block — the one place no other list page has it — so a page that uses that component passes
`hideSearch` and filters `rows` itself. Two search inputs one row apart would be worse than
either.

`Clients.tsx` no longer uses `FilterableTable` at all (§1.5), so its toolbar is now simply
the only search box on the page; it kept the `page-toolbar` testid, and its test asserts both
that the box is *inside* that toolbar and that there is exactly one of it. The `hideSearch`
escape hatch is documented here because the next page to reach for `FilterableTable` will
need it — with `hideSearch` set, that component's internal `q` never leaves `""`, so it can
no longer distinguish a first-run empty from a search miss and always renders `emptyText`,
which is correct **only** where the genuine first-run case is `ListStates`' `empty` and never
reaches the table. See §2.

### 1.5 A grouped list is the siblings' hand-rolled `<table>`, never `FilterableTable`

`src/routes/explore/Charts.tsx`, `Datasets.tsx`, `Dashboards.tsx`, `Notebooks.tsx`,
`dataapi/Endpoints.tsx`, `dataapi/Clients.tsx`

`FilterableTable` sorts, searches and paginates **one flat entity**. It cannot model a group,
so a page whose list has two levels writes the table out by hand, exactly as the BI lists do:

```tsx
<div className="overflow-x-auto">
  <table className="w-full text-sm">
    <thead className="text-left text-muted-foreground">…</thead>
    {groups.map((g) => <tbody key={g.id}>{/* header row, then its rows */}</tbody>)}
  </table>
</div>
```

One `<tbody>` per group, the group's own heading as the first row in it, `border-t` between
rows and `hover:bg-accent` on them.

**The defect.** `Clients.tsx` nested a variable-length list of OAuth2 clients inside one cell
of a `FilterableTable` row — each client a line of running text carrying a name, a
36-character id, a badge, a last-used phrase and a Revoke — because the page's list is
two-level and the component is not. Nothing aligned down the page, and an irreversible action
sat as body text between two timestamps. A client is a row now and the service account is a
group header, in the hand-rolled table above.

**Do not reach for `FilterableTable` to avoid hand-writing a `<table>`.** Its styling is not
the siblings': bold dark headers against their muted ones, `border-b` against `border-t`,
`hover:bg-muted/50` against `hover:bg-accent`. The mismatch is visible in the browser on a
page that sits next to those siblings in the sidebar, and has been reported as one.

**What the hand-rolled table gives up, and how to keep it.** `FilterableTable` also paginates,
and a page whose rows each fire their own query was relying on that for more than layout:
`Clients.tsx`'s `pageSize={25}` capped how many per-account client requests could be in flight,
because only one page of rows was ever mounted. Grouping removed the pagination, so the cap is
now explicit (`GROUP_PAGE`, a "Show N more" button), and its test asserts the number of
distinct accounts queried on first paint — not just what is on screen.

**`role="rowheader"` on the group header is this page's addition, not yet the siblings'
practice — and this section would otherwise be a convention its own reference implementation
violates.** `Clients.tsx` puts the role on the header's spanning cell, the same idiom §4.1
records for `ConfigTableGroup` and for the same reason: a group row that reads as another data
cell is the confusion the shape exists to remove. But `FolderHeaderRow.tsx` — the shared
component Charts, Datasets, Dashboards and Endpoints group with — does **not** set it, so four
of the five grouped tables in Explore currently announce their folder header as an ordinary
cell.

That is a gap in those four, not a licence for the next one. A new grouped table sets the
role, and `FolderHeaderRow` should adopt it: it is a one-attribute change, held back only
because the task that wrote this section was scoped to a single page. It is stated here rather
than left implicit so that nobody reads the reference implementation's silence as the
convention.

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

Fixed by package U on 2026-08-22 (`docs/en/plans/2026-08-22-explore-list-primitives-adoption.md`,
Task 3); the four pages now branch through `ListStates`.

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

`ActivityCallList.tsx` is the second reference, and it splits the same way: "No calls match
the current filters." when a narrowing control is engaged, and otherwise the page's one
`EmptyState`, "No calls in this range". **The predicate is what the reader can see engaged and
nothing else.** Its first version counted the "Include trial runs" view switch as a filter,
and a range with no trials in it then reported a mismatch with no filter set — the same
defect class as §2.2, an interface saying something untrue about its own state. Guarded inside
`"can be turned back off from a trial view that found nothing"`.

**Where the one empty state lives is a consequence of what is mounted.** Activity's tiles and
per-endpoint table do not render when there is nothing to put in them, so the empty state
belongs to the call list, the only section mounted in every state of the summary query. That
list also sits **outside** the summary's `ListStates`, which is what keeps a range switch from
unmounting it and resetting the four filters the operator typed
(`"keeps the filters the user typed across a range switch that reloads the summary"`). A
region with nothing to say passes `empty={null}` rather than a second sentence.

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

### 4.2 A read-only listing is not a setting, and an action is not a setting

`src/routes/explore/dataapi/detail/ResponseSection.tsx`. Only `Output shape` was
config-shaped, and only it became a `ConfigTableGroup` + rows. The other two blocks are not
rows of that table, and the reason is the durable part of this section:

- **Frozen columns** — a read-only `Name | Type` listing of the *published* contract. There
  is no Value to edit. Forcing it into the shared table would either fabricate a Value column
  or nest a table inside a cell, and would make read-only output look editable.
- **OpenAPI document** — a paragraph, a download button, and a derived base URL. An action,
  not a setting.

**The rule:** the complaint that removed the `ConfigSection`s was about cards framing
near-empty tables (§4.1), not about cards.

#### The part of this section that was reversed, and why

Until the final task of the phase this section was titled *"When a card is still right"*, and
those same two blocks were `Card`s — kept deliberately in #521, on the argument above. **The
argument was right and the conclusion did not follow**, which is the useful thing to record:
"not a `ConfigTable` row" was being read as "therefore a `Card`", because at the time those
were the only two options anyone had written down.

§4.3 then supplied the third — a heading plus a plain `Table`, or plain prose, in the
section's `space-y-6` rhythm — and against it both blocks moved:

- **Frozen columns is the textbook §4.3 shape**: `Name | Type`, one row per column, read
  down. It is structurally identical to the publish history `Versions` converted, whose own
  doc comment cites this section as its precedent while doing the opposite thing to the same
  shape. Dropping the card is also what *forces* §4.3's second rule — the states became rows
  spanning the table, and before that the branch replaced the table and took `Name` and `Type`
  with it, so an author who had never published saw a sentence with nothing on screen naming
  what a frozen column is. The card title had been carrying that alone. It was, additionally,
  the section's third table idiom: a hand-written `<table>` beside a `ConfigTable`, in a tab
  set that otherwise uses the shared `Table`.
- **The OpenAPI block is not a list, so §4.3 does not reach it** — it moved on the narrower
  point that "an action is not a setting" argues against `ConfigTable` and not for a frame.
  `Versions` already renders that exact shape (prose, then a button that opens a dialog) under
  an `h3` with no card, and the one thing in the block that wants a visual boundary is the 409
  `Alert`, which brings its own.

What settled it was neither consistency on its own nor the recorded decision on its own.
**A recorded rationale binds its argument, not its conclusion**: re-read it against the
options that exist now, and if the conclusion no longer follows, say which premise stopped
carrying it. Here nothing in #521's reasoning became false — an option appeared that it had
never weighed.

`ResponseSection.tsx`'s doc comment carries this so it does not have to be re-litigated from
the git history, and `EndpointDetail.test.tsx`'s "Output shape as a group row" test pins all
three claims at once: `Output shape` is a `rowheader`, the other two are headings and not
rowheaders, and neither is inside a `.bg-card`.

### 4.3 A list is a plain `Table`, and its three states are rows inside it

`src/routes/explore/dataapi/detail/Versions.tsx` (the publish history),
`detail/AccessSection.tsx` (one row per service account), `detail/TryIt.tsx` (parameter
values), `detail/ParamEditor.tsx`, `detail/ResponseSection.tsx` (the frozen `Name | Type`
contract — the last one, and the one that made §4.2 reverse itself).

`ConfigTable` is `Setting | Details | Value` for a **fixed set of settings read across**. A
**list read down** — one row per snapshot, per principal, per parameter — takes a plain
`Table` instead: there is no Value to edit, only a per-row action, and forcing one in would
fabricate the empty third cell §4.1 records as the confusion the cards were replaced to
remove.

Two things that came out of converting `Versions` and `Access`:

- **The loading / error / empty states are rows spanning the table, not a branch that
  replaces it.** With a card, the states sat *inside the frame* and the frame carried the
  section's name. Without one, a branch that replaces the table takes the column headers with
  it — so before the first publish, `Versions` would say "never published" with nothing on
  screen to say what a version even records. As spanning rows, the columns are named in every
  state. The shared `TableCell` is `whitespace-nowrap`, so a cell holding a sentence needs
  `whitespace-normal` or it scrolls the table sideways.

  `ResponseSection`'s frozen-column list is the third case and has **four** states, not three
  — "loading", "the fetch failed", "never published" and the list itself — because the first
  two are gated on `endpoint.live_version_id` (§2.1/§2.2 inside one section). Its test asserts
  the `Name`/`Type` `columnheader`s and the `colSpan` in each of the three non-list states in
  one loop, deliberately: the regression is somebody re-writing *one* branch as a replacement,
  and a test covering only the empty state passes while the loading branch does it.
- **A column whose every cell is identical is section-level copy in disguise.** This
  document said the opposite for one commit, and the browser disproved it. `Access`'s `Data`
  column held the same 143-character sentence in every cell, and the argument for it was that
  *the constancy is the fact* — egret never knows whether an account may read the tables. The
  reasoning is sound and the conclusion was wrong: a sentence true of every row is a
  statement about the **section**, and repeating it costs a column. Measured at a 786px
  content box, the two rows came to 89px and 293px — ~382px for two accounts, with 288px of
  width spent saying one thing N times.

  **The test for it:** ask what is left of the column once the constant sentence is hoisted.
  For `Access` it was one thing, the `Try it` link carrying `?service_account=<id>` — and
  that is an *action*, so it joined the actions column and the `Data` column went. **Do not
  keep a column empty for symmetry.** Say once that the condition is not a column and why; a
  reader who expects it is then told where it went instead of assuming it was dropped.

  **Where "once" goes: the surviving column's header, not a paragraph above the table.** This
  said "above the table" until the Data API UI audit, which measured the result: `Access` had
  a page-level `SettingsNote` *and* that paragraph, ~550 characters of standing prose over a
  table whose first row is one line — and the two overlapped, both saying the invoke right on
  the page is not the data access a call needs. Merged into one §4.4 `Caveat` on the `Invoke`
  `<th>`, the sentence qualifies the answers it is about rather than the page.

  **A `<th>` names every cell in its column, so give it an explicit `aria-label`.** Without
  one the header's accessible name becomes the whole caveat and a screen-reader user hears
  550 characters before each row's answer — the audio form of the very defect that removed
  the `Data` column. `AccessSection.tsx` carries `aria-label="Invoke"`, and
  `AccessSection.test.tsx` asserts the columnheaders by **accessible name** rather than
  `textContent`, because `textContent` would have passed on exactly that regression.

  **Let the library compute that name; do not approximate it.** The first version of that
  assertion mapped headers through `aria-label ?? textContent`, which agrees with the naming
  algorithm today and diverges the moment a header uses `aria-labelledby` — a stand-in for the
  thing being tested, in a test whose whole subject is naming. `getByRole(role, { name })` runs
  the real computation, so pinning **order and name together** is
  `expect(headers[n]).toBe(screen.getByRole("columnheader", { name: … }))` with nothing
  reimplemented.

**What the labels buy.** `Access`'s Invoke condition used to be a `<dt>` inside each row's
`<dl>`, which repeated the word on every row and left each row's answer at a different
vertical offset from the one above it. As a column the label is written once and the answers
line up down the page — and comparing accounts is the whole job of that screen.

### 4.4 A caveat: the consequence in the summary, the reason behind it

`src/routes/explore/dataapi/detail/Caveat.tsx` — used by `AccessSection`, `TryIt`, `Query`
and `PreviewBlock`.

It was written row-scoped, for one `Access` row, and the Data API UI audit found the same
shape at SECTION scale — `Try it` carried ~950 characters of standing prose over one dropdown
and one button. So the rule is not about rows: **an explanation that describes one control
belongs in that control's `Details` cell, capped at about two lines; an explanation that
describes a consequence of acting goes behind a disclosure headed by the consequence.** The
component moved out of `AccessSection.tsx` when the fourth caller wanted it.

Some per-row prose genuinely *is* per-row. `Access`'s "this account also reaches invoke
through the workspace, so revoking here removes the grant but not the access" is the
difference between "revoking works" and "revoking looks like it worked", and it must stay
reachable. But as 155 characters of running text in a cell it made that row 293px against an
89px neighbour — **rows disagreeing visually for a reason that is not about importance.**

The form that fixes it without hiding anything is a native `<details>` whose **`<summary>`
carries the consequence** and whose body carries the explanation:

> ▸ Revoking will not remove access
> *(open)* This account also reaches invoke through the workspace, so revoking here removes
> the grant but not the access. [Workspace membership]

**Why this is not the drawer mistake.** §11.2 records the opposite ruling for `TryIt`'s
warning: a reader who must open something to learn a call is real has already pressed the
button. That rule holds here, and the split is what satisfies it — the load-bearing fact is
the summary and is on screen with nothing clicked. What is collapsed is the *why* and the
*where to go instead*, which is what a reader goes looking for once the headline has stopped
them. **Collapsing the headline would be the mistake; collapsing the paragraph under it is
the point.**

Three constraints on the form, all load-bearing:

- **Native `<details>`, never `ui/collapsible`** — §11.2's rule, and here the stakes are
  highest: the shell's `<fieldset disabled={trashed}>` disables a Radix trigger, so the
  caveat would be unopenable on exactly the endpoint where an admin is deciding whether to
  restore it.
- **Not hover-only.** `<summary>` is focusable and toggles on Enter/Space, so keyboard and
  touch reach it the way a pointer does. A `title` tooltip is cheaper and hides it from both.
- **Collapse a caveat because it is big, not because its neighbour is.** `Access`'s transient
  "Checking workspace-inherited invoke…" stays plain text: six words, no explanation to hold,
  so a disclosure costs a click to reveal nothing.

**An action column's header is `sr-only`.** A visible "Actions" heading over one small
button is louder than what it labels; an unnamed column is worse for anyone not reading it
visually. `getAllByRole("columnheader")` still sees the name, which is what the shape tests
assert on.

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
`max-w-5xl`, and `ResponseSection.tsx`'s frozen-column table adds `max-w-xl` (it kept that cap
across the §4.2 conversion — the class moved from a hand-written `<table>` onto the shared
`Table`, which is where the `w-full` it caps comes from). Query needs none
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
(~816px), which also reined in the `Frozen columns` and `OpenAPI document` Cards — the frozen
*table* had `max-w-xl`, its Card did not. (Those two Cards are gone as of §4.2's reversal. The
measurement is kept: it is why the section root is capped, which is still true, and "the frame
stretched further than the content it framed" was one input to removing them.)

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

### 5.6 The section guard is one helper, not one copy per section

`src/routes/explore/dataapi/__tests__/sectionWidth.ts` —
`expectSectionCapsItsOwnWidth` / `expectSectionLeavesWidthToThePage`.

Three sections had the walk in §5.5 copy-pasted and **two had no guard at all**: `Try it` and
`Access`. That is how `Try it` shipped a `max-w-5xl` nothing was holding in place — §5.1
lists it among the four sections that cap themselves, and nothing said so in code. The walk,
the host resolution and the two regexes now live in one module and all five sections call it.

Two details are load-bearing and were preserved from the copies:

- **The two regexes differ, on purpose.** The positive form is unprefixed
  (`/(?:^|\s)max-w-/`), because a section capped only at `lg:` is uncapped on exactly the
  narrow viewports nobody measured. The negative form includes responsive prefixes, because
  `lg:max-w-3xl` wastes the page on exactly the wide ones the pass *did* measure.
- **The host is resolved, not assumed**, so the same helper works under the real
  `EndpointDetail` shell (walk stops at the `fieldset`) and under a harness that stubs the
  context and renders the section under a bare `Outlet` (`TryIt.test.tsx`), where the section
  root is a child of the render container.

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
- A `Card` count in this document's own [Adoption status](#adoption-status) read "8
  occurrences … two are the deliberate keeps of §4.2; the other six are the open question".
  The eight were `<Card>`/`<CardHeader>`/`<CardTitle>`/`<CardContent>` on each of the two
  keeps — **there was no third block**, and the plan written from that line inherited the
  arithmetic and asked an implementer to "convert the rest". A **symbol-family** count is not
  a **block** count. The same line's "down from 4 files" was 4 sections; the measured answer
  under `detail/` is 5, because `ParamEditor` imported `Card` too.

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

## 11. "Configure, run, read the result" is a split, not a stack

`src/routes/explore/dataapi/detail/Query.tsx` (the first), `detail/TryIt.tsx` (the second).

Two sections of the endpoint editor have the same shape — set something up, run it, read what
came back — and both are now a vertical `Group` from `react-resizable-panels` with a drag
handle between the halves, rather than a page that scrolls.

**The defect.** Stacked, the result sits below the setup and off-screen: run it, scroll down,
read, scroll back up to change a value, scroll down again. `Query`'s preview was below
`ParamEditor`, and `TryIt`'s result below a `Run as` card. In a browser `Query`'s first
version of the split was worse still — the strip of tables took 370px of a 615px section and
left the SQL editor 107px, about four lines, which no test could see.

The parts, all four load-bearing:

- **The setup is tables** (§4) — a `ConfigTable` row for a setting, a plain `Table` for a list.
  `TryIt`'s service-account picker is one `ConfigTableRow`; its per-parameter trial *values*
  are a plain table, one row each.
- **The result is the lower panel, and it owns the run control.** The button and the shortcut
  live in the *result* panel's header, not with the setup: `PreviewBlock` and `TryIt` both put
  `Run` there, and the header stays put while the body scrolls.
- **The shortcut is named beside the button it fires.** `⌘↵`, in the header. An affordance
  nobody can see is not an affordance. `Query` registers it as a Monaco command (`SqlEditor`'s
  `onRun`); `TryIt` has no editor, so it uses a `window` `keydown` listener, the idiom
  `components/sqllab/SqlTab.tsx` already uses for `⌘K`.
- **Sizes are percent STRINGS.** In `react-resizable-panels` v4 a bare number means *pixels*,
  so `defaultSize={55}` is a 55-pixel editor that reads as correct in the source. Both sections
  are tested for the string *type*, not only the value.

### 11.1 The shortcut is not covered by any of the guards the button has

A `window` key handler is not a form control, so it walks straight past the shell's
`<fieldset disabled={trashed}>`, past the button's own `disabled`, and past a pending
mutation. Every guard the button expresses declaratively has to be re-checked at the top of
the run function — `PreviewBlock` re-checks `isPending`, and `TryIt` re-checks account,
`isPending` and `trashed` in one `canRun`. Before it had a shortcut, `TryIt` did not read
`trashed` at all; adding one without the guard would have made a trashed endpoint runnable by
keyboard only. `Query` solves the same problem one level up, by passing
`onRun={trashed ? undefined : …}` so the command is never registered.

The listener must also reach the *current* form. A `[]`-dependency listener closes over the
first render's callback and posts an empty body; re-subscribing every keystroke churns a
window listener per character. Both sections refresh a ref after every render instead
(`PreviewBlock`'s `useImperativeHandle` carries no dependency array, `TryIt` an effect with
none).

### 11.2 A `<details>` strip, and when *not* to use one

`Query` puts its tables in a native `<details>` above the split, collapsed by default, because
`ParamEditor` is 370px of the section (the measurement above).

**Native `<details>`/`<summary>`, never `ui/collapsible`.** The shell disables the whole
section for a trashed endpoint, and a Radix trigger is a `<button>` — so a trashed endpoint
could not collapse the strip. `<summary>` is not a form control and the `fieldset` does not
touch it. §4.4 applies the same rule to a per-row caveat, where the consequence of getting it
wrong is larger: a strip that will not collapse is an annoyance, a caveat that will not open
is a warning the reader cannot reach.

**`userEvent` will not click it inside a trashed section, and the product is fine.**
`userEvent.click` refuses to open a `<details>` under `<fieldset disabled>`; `fireEvent.click`
opens it, and jsdom's own summary activation behaviour runs. That is userEvent being stricter
than the platform — `fieldset[disabled]` disables *form-associated* descendants, and
`<summary>` is not one, so every browser opens it. Measured in `AccessSection.test.tsx`'s
trashed-caveat test, whose only difference from the passing untrashed test is `deleted_at`.
**The failure mode is a false negative**, so a test of any non-form affordance inside a
trashed section must use `fireEvent`, or it reports the product broken when it is not.

**`TryIt` deliberately has no strip**, and the reason still holds for the SETUP: it is one
picker and a short row per parameter, so there is nothing to win. **Collapse a strip because
it is big, not because the other section has one.**

#### The part of this section that was reversed, and why

This paragraph used to end differently. It said `TryIt`'s prose could not be collapsed *at
all*, on `PreviewBlock`'s rule — a reader who has to scroll, or open a drawer, to learn that a
trial is a real call has already pressed the button — and concluded that the ~500-character
warning must stand beside Run.

**The argument was right and the conclusion did not follow**, which is the same shape §4.2
records. That rule argues against hiding the FACT. It says nothing about the four sentences of
*mechanism* under it, and reading it as "nothing may be collapsed" is what let the section
reach ~950 characters of standing prose over one dropdown and one button. §4.4 supplies the
option the old wording never weighed: split at the hinge, headline on screen, mechanism
behind it. Both `TryIt` blocks and `PreviewBlock`'s note are `Caveat`s now, headed by "A trial
is a real call, not a dry run" and "A green preview does not prove a consumer can call this
endpoint" — the sentences the old ruling was actually protecting, still on screen with nothing
clicked.

What did not move is as informative. `TryIt`'s two-line published-version note and `Access`'s
conditional state paragraphs stay plain text, on the same rule pointing the other way:
**collapse a caveat because it is big.** Two lines with no explanation to hold cost a click to
reveal nothing.

### 11.3 What a test can say about any of this

jsdom does no layout (§10), so the drag, the heights and the widths are a browser check. What
is testable is the wiring, and it is where the regressions were: which panel each half is in
(`within(panels[0])` / `within(panels[1])`), that the handle is *between* them
(`compareDocumentPosition`), that the sizes are strings, that the shortcut fires the mutation
with the on-screen values, and that it fires nothing when a guard in §11.1 should stop it.

**jsdom does not hide a closed `<details>`** — it has no layout to hide it with, and Testing
Library does not exclude its contents. Assert the `open` **attribute**; `toBeVisible` passes
either way and says nothing.

**So test that a sentence is still REACHABLE, not merely still present.** Once copy moves
behind a §4.4 disclosure, every pre-existing `getByText` for it keeps passing whether or not a
reader can get to it — the Data API audit's relocation turned four such tests into assertions
about the DOM rather than about the product. The form that says something: find the summary,
assert the `<details>` is closed, `fireEvent.click` it, assert `open`, and assert the sentence
is inside that element and NOT inside the `<summary>`. Do it inside a genuinely disabled
`<fieldset>` at least once per section, which is the only place the `<details>`-vs-Radix
choice is observable.

**All five steps, or none of them counts.** One of those four rewrites clicked the summary and
then asserted the sentences were in the document, and that is not a reachability test — a
click is not an observable. Review mutated `Caveat` to a plain `<div>`/`<div>`, `tsc` clean:
fifteen tests in the feature died and that one stayed green. The two steps it was missing are
the two that carry the meaning — `open` after the click (else a Radix collapsible with
`forceMount` plus CSS hiding passes) and containment (else a `<summary>` that swallowed the
paragraph passes). **Asserting `not.toHaveAttribute("open")` BEFORE the click is the third**,
without which `<details open>` — a disclosure that collapses nothing — passes too.

---

## Adoption status

Honest scope, measured on the current tree, so nobody reads this as describing code that does
not exist yet:

- `ListPageShell` — **9** route files: 5 Data API (`Endpoints`, `Clients`, `EndpointCreate`,
  `ServiceAccountCreate`, `ClientCreate`) and 4 Explore list pages (`Datasets`, `Charts`,
  `Dashboards`, `Alerts`), converted by package U on 2026-08-22. Measured with
  `grep -rl 'from "@/components/explore/ListPageShell"' src --include='*.tsx' | grep -v __tests__ | wc -l`
  from `frontend/`. **`Notebooks.tsx` still hand-writes its header** (735 lines, `aria-pressed`
  = 0). It was named in neither §1.3's list of four nor package U's scope, and is recorded
  here so this section is not read as "every Explore list page is converted".
- `ListStates` — **6** route files (`Endpoints`, `Clients`, `Datasets`, `Charts`, `Dashboards`,
  `Alerts`). Measured with
  `grep -rl 'from "@/components/explore/ListStates"' src --include='*.tsx' | grep -v __tests__ | wc -l`
  from `frontend/`.
- `EmptyState` — **14** files (2 Data API, 8 control-plane, 4 Explore list pages). Measured
  with `grep -rl 'from "@/components/explore/EmptyState"' src --include='*.tsx' | grep -v __tests__ | wc -l`
  from `frontend/`.
- `ConfigTable` — **16** production files; `ConfigTableGroup` is used by Data API's `Config`
  and `ResponseSection`. Measured with
  `grep -rl 'from "@/components/config/ConfigTable"' src --include='*.tsx' | grep -v __tests__ | wc -l`
  from `frontend/`. Earlier revisions, measured with
  `git grep -l 'from "@/components/config/ConfigTable"' <rev> -- frontend/src`, minus
  `__tests__`: **13** at the merge base `63a4fb90`, 14 after `Query` (#529), 15 after `Try it`.
  The three sections after it (`Versions`, `Access`, `Response`) added none, and that is the
  expected result rather than a gap — all three converted *lists* (§4.3), and a list does not
  want a `Setting | Details | Value` table. The sixteenth is `ScheduleEditor.tsx` (alerts),
  converted to `ConfigTable` rows section by section. Ten earlier consumers already nest
  `ConfigTable` inside a `ConfigSection` — `EngineSecurity`, `ImageSection`,
  `EngineAutoscaling`, `EngineConfigReliability`, `EngineRoleConfig`, `StarRocksConfigTab`,
  `ContainersMountsSection`, `PodSection`, and Data API's `Config` and `ResponseSection` — so
  keeping the frame here is not new; it is not §4.1's stacking defect when the page renders
  only one section at a time.
- The plain-`Table` list of §4.3 — **5** files, all Data API endpoint detail (`ParamEditor`,
  `TryIt`, `Versions`, `AccessSection`, `ResponseSection`). Measured with
  `grep -rl 'from "@/components/ui/table"' src/routes/explore/dataapi/`, which also returns
  `Activity.tsx` and `ActivityCallList.tsx` — list pages, not detail sections, and not counted
  here.
- `Card` under `routes/explore/dataapi/` — **0** files, down from **5** at the merge base
  (`git grep -l 'from "@/components/ui/card"' 63a4fb90 -- frontend/src/routes/explore/dataapi/`
  gives `AccessSection`, `ParamEditor`, `ResponseSection`, `TryIt`, `Versions`). Two numbers in
  the previous revision of this list were wrong and are corrected rather than carried forward,
  which is §9 happening to this document again:
  - "down from **4** files" — that was the four *sections*; `ParamEditor` also imported `Card`
    and is under `detail/`. Five files, by the command above.
  - "8 occurrences … two of those eight are the deliberate keeps of §4.2; the other six are
    the open question". There was no third card block: `grep -o '<Card[A-Za-z]*'` on
    `ResponseSection` at that revision gives `<Card` ×2, `<CardHeader` ×2, `<CardTitle` ×2,
    `<CardContent` ×2 — **the eight occurrences *are* the two deliberate keeps**, four JSX tags
    each. The plan inherited the same arithmetic ("8 `<Card>`… convert the rest"), and there
    was no rest. A symbol-family count is not a block count — the same trap §7.2 records for
    consumer counts, one level down.
- The `Caveat` disclosure of §4.4 — **4** files, **9** call sites, all Data API endpoint
  detail (`AccessSection` 4, `TryIt` 2, `Query` 2, `PreviewBlock` 1). Counted by import
  specifier (`from "./Caveat"`), per §7.2. It was 1 file and 3 call sites for one phase, all
  in `AccessSection`, where it was written for a row-scoped explanation big enough to set the
  row height; the Data API UI audit found the same shape at section scale and it moved out of
  that file into `detail/Caveat.tsx`. Still deliberately not in `components/`: the rule it
  encodes has been measured on this editor and nowhere else.

  What has NOT adopted it is the rule working rather than an inconsistency — `Access`'s short
  transient states, `TryIt`'s two-line published-version note and `Access`'s conditional state
  paragraphs are all plain text, because a disclosure over two lines costs a click to reveal
  nothing.
- The split of §11 — **2** sections, both Data API endpoint detail (`Query`, `TryIt`); `react-resizable-panels` itself is also used by `components/sqllab/SqlTab.tsx`, which is
  where the idiom came from. `Versions` and `Access` are deliberately not splits: neither is
  "configure, run, read the result" — there is nothing to run.
- The section-width guard of §5.6 — **5** sections (`Config`, `Response`, `Versions`,
  `Try it`, `Access`), one helper. It was 3 copies and 2 absences before this phase.
- `ConfigSection` — 13 files, down from **15** at the merge base. This is the fifth count of
  this number in the programme and the first measured at both ends:
  `grep -rl 'from "@/components/form/ConfigSection"' src` gives 15 at `63a4fb90` and 13 at
  `HEAD`. 14 was a mid-branch state, quoted as if it were the base. If further sections convert
  to group rows, revisit whether `ConfigSection` still earns its place. Still 13 after this
  package: `ScheduleEditor.tsx` was already one of the 13, since it kept its `ConfigSection`
  frame while its rows moved to `ConfigTable` — the frame did not change, so the count doesn't
  either.

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
