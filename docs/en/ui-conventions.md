# egret UI conventions

> **The rules now live in [`frontend-rules.md`](frontend-rules.md), which is normative.** This document is the rationale and the record of reversed decisions that the rulebook links to.

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
turned out to be empty.

**What that classification decides is GATING, not placement — despite this section's title.**
"View controls first, primary action last" orders the header for controls that are *in* the
header; calling something a view switch settles that it renders whatever `hasData` says, and
says nothing about which slot it renders into. The range strip is the case that separates the
two: it is a view switch, and it lives in Activity's **body** rather than in `ListPageShell`'s
`toolbar` slot, because `RangePicker` is a two-row block — a button strip over a `<details>`
retention caveat — and moving that into the header's `flex flex-wrap items-center` group is a
layout change no jsdom test can check (§10). Neither the classification nor this section's
title argues for moving it; recorded here because the title alone reads as though it does. **So is "Include trial runs"**, which is less obvious and matters
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
`Alerts.tsx`, `dataapi/Endpoints.tsx`, `dataapi/Clients.tsx`

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

**`role="rowheader"` on the group header, on all seven.** `dataapi/Clients.tsx` puts the role on
its header's spanning cell, the same idiom §4.1 records for `ConfigTableGroup`, and
`FolderHeaderRow.tsx` — the shared component Charts, Datasets, Dashboards, Notebooks, Alerts and
Endpoints group with — now does too.

**It was verified before it was added.** The role is right for the shape rather than borrowed
from the house style: `rowheader` is what HTML-AAM computes for `<th scope="rowgroup">`, which
is the HTML-native spelling of precisely this construct — a full-span header cell heading the
rows of one `<tbody>`, each folder group being its own `<tbody>` with the header row first.
ARIA has no separate rowgroup-header role, so `rowheader` is what this construct gets. Its
required context is a `row`, which the cell has.

**Claim exactly this much and no more: it announces as a header instead of as a data cell.**
The role does **not** carry a group association. In HTML, `scope="rowgroup"` says "this heads
the remaining cells in this row group"; ARIA has no `scope` equivalent, so the cell is a
rowheader for its own row — which contains nothing else — and is not associated with the rows
beneath it. Nothing is re-announced on those rows, but that is because no association exists at
all, not because one was scoped correctly. Against the `cell` role it replaces this is a strict
improvement, and it is not the full semantics of the HTML spelling. An earlier revision of this
section wrote the true sentence ("nothing is re-announced") with the false reason attached, and
that is the kind of overclaim a later reader inherits as a fact.

It stays a `<td>` with an explicit role rather than becoming a `<th scope="rowgroup">`, for the
reason §4.1 records: a `<th>` brings its own implicit role to argue with. **That choice has a
cost, named here rather than left implicit: `<th scope="rowgroup">` would carry the association
the `<td>` does not.** `scope` is absent because it is a `<th>`-only attribute and inert on a
`<td>`; adding it would be decoration.

**Do not add an attribute because this document predicted it.** The previous revision of this
section said the change "should" be made and called it a one-attribute change; that is a
prediction, not a verification, and it would have read identically had the role been wrong for a
spanning cell. The check that settled it was the HTML-AAM mapping.

**And check that the guard you cite can fail.** The code check is
`getByRole("rowheader", { name: /Revenue/ })` — the real role and accessible-name computation,
not `getAttribute("role")` — on `FolderHeaderRow`'s own test, plus **"the folder headers are the
only rowheaders, and the data rows stay cells"** in
`src/routes/explore/__tests__/Charts.test.tsx`. The uniqueness half lives on a page and not on
the component **because the component cannot fail it**: it emits one cell, and its fixture has
no data rows for a stray role to land on, so the assertion could only have broken if
`FolderHeaderRow` emitted two rowheaders — which it structurally cannot. It sat on the component
for one revision and this section cited it as the reason the ruling was safe. The failure it now
actually catches is a data cell in a page's `renderRow` gaining the role, which makes the folder
header no longer distinguishable from the rows it heads — the exact confusion the role was added
to remove, and invisible to every other test in the file.

**It was recorded as four, and the counting is worth more than the number.** `Notebooks` was
named in this section's own file list above and then dropped from the count in the paragraph
this replaces; `Alerts` was in neither, because it was missing from the list the count was read
off. The count was taken from a prose list in this document instead of from the tree. **Count by
resolved module:**

```bash
grep -rln 'from "@/components/explore/FolderHeaderRow"' frontend/src   # 7: the 6 pages, plus its own test
```

Same failure as §7.2's `Caveat` inventory, and the same fix: an inventory in this document is a
claim about the tree, so it is taken FROM the tree.

### 1.5a A workspace with no folders gets no section header

`src/components/explore/folders.ts` — `showFolderSections(folders, showTrash)`

`groupByFolder` always returns at least the folderless bucket, so "is this list grouped" is
**not** the question "does this list get section headers". A workspace that has never made a
folder still gets one group, and heading it turns a plain list into a collapsible
"Uncategorized (1)" — a section introducing a taxonomy the workspace is not using, whose
collapse control's only effect is to hide everything. "Uncategorized" carries information only
beside a NAMED folder; alone, it labels the whole list after the one thing it is not.

```tsx
const showFolderHeaders = showFolderSections(folders, showTrash);
```

**Read off `folders`, never off `groups.length`.** With a filter engaged `groupByFolder` drops
empty named folders, so a search matching only folderless rows collapses `groups` to one and
would make the header blink out mid-keystroke and back on deleting the character.

**The trash is never sectioned.** A trashed row's folder is where it will go BACK to, not where
it is, and the trash is the one view whose whole job is to let the author find something they
deleted.

**Feed the one flag to the render loop AND to `visibleRows`.** Those are two derivations of the
same question. Gating only the render leaves a collapse state that `useCollapsed` restored from
localStorage — it outlives the folder that produced it — hiding rows from "Select all" that are
in plain sight, with no header left anywhere to expand.

**The defect.** `sqllab/SavedTab.tsx` wrote this gate first and `dataapi/Endpoints.tsx` second;
`Charts.tsx`, `Datasets.tsx`, `Dashboards.tsx`, `Notebooks.tsx` and `Alerts.tsx` headed the
folderless bucket unconditionally. Two copies of a rule and five pages without it is how the
rule was going to be deleted for "consistency", so the gate is a shared helper now and the
call sites cannot drift.

**`folderCollapseKey(ws, kind)` for the same reason.** Every one of these pages spelled
`` `folders:${ws}:${kind}` `` inline, and its test spelled it again. A test that seeds
localStorage under a key the page does not read **passes anyway**, because a seed that misses is
indistinguishable from a page that correctly ignored it. Its parameter is `FolderKind`, not
`EntityKind`: `data_api_endpoints` has folders but no `PATCH …/{id}/folder` route, so it exists
only in the wider type. `Endpoints.tsx` keeps its one-argument export as a thin wrapper so
`FOLDER_KIND` stays bound in one place.

**Counted from the tree, not from this paragraph:**

```bash
grep -rl 'showFolderSections' frontend/src   # 9: folders.ts, its test, and the 7 grouped lists
```

**Both fixtures, on every page.** A page tested only without folders cannot tell a working gate
from one wired permanently off; a page tested only with folders cannot see the gate at all.

### 1.6 `SummaryStrip` owns the singular, and the pages own the words

`src/components/explore/SummaryStrip.tsx`

The strip prints `{value}` then a word. **The rule that picks the word lives in the
component**: the number `1` — and only the number `1` — renders `singular`; every other
count, zero included, renders `label`.

```tsx
<SummaryStrip items={[{ label: "charts", singular: "chart", value: rows.length }]} />
```

**The defect.** Seven Explore list pages render counts through this component and five of
them read "1 charts", "1 folders", "1 dashboards". The other two — `dataapi/Endpoints.tsx`
and `dataapi/Clients.tsx` — had been fixed with a ternary *at the call site*, which is how a
rule ends up applied on two pages and forgotten on five. Both ternaries are gone; all seven
now pass the pair.

**Why a `singular`/`plural` pair and not a derived plural.** Most labels on this strip are
not nouns. "live", "draft", "disabled", "active", "paused", "failing", "ok", "granted",
"queued" are adjectives, "phase" and "auth" are not counts at all, and "endpoints total" and
"queries run" do not carry the noun at the end. A component that inflected `label` would have
to tell those apart from "charts" with no information to do it, and English gives it no rule
to borrow ("bus", "was"). Naming both words at the call site costs one field, needs no
dictionary and no i18n dependency, handles an irregular ("queries"/"query") for free, and
makes an adjective gaining an `s` **unrepresentable** rather than merely remembered.

**It also keeps the key stable, and that is not a coincidence.** `SummaryStrip` keys its items
by `it.label`. Because `singular` is a *separate field*, `label` is count-independent, so the
rendered word changes at 1↔2 while the key does not. The alternative API — a count-aware
`label` formatter — would have made the key count-dependent and remounted a focused toggle
chip the moment its own count crossed one. Guarded by `"keeps the same DOM node across a
1 -> 2 transition"` in `src/components/explore/__tests__/SummaryStrip.test.tsx`.

`value === 1` is strict identity on a **number**. `value` also carries formatted strings
(`EngineDetail.tsx` passes `"3/4"` and a phase name), and a string that happens to read `"1"`
is not a count of one.

**Adoption, counted from the tree and not from this paragraph:**

```bash
grep -rl 'from "@/components/explore/SummaryStrip"' frontend/src   # 26, one of them the test
grep -rl 'singular:' frontend/src                                  # 8, one of them the test
```

25 modules render a strip; **7 pass `singular`** — the seven Explore list pages. The
remaining 18 are control-plane pages, Explore settings, `MembersPanel.tsx` and
`DatabasesPage.tsx`, and they still read "1 users", "1 engines", "1 secrets". That is a
stated gap, not a second convention: the field is opt-in, so those pages are unchanged rather
than differently-behaved, and the next one to be touched sets the pair.

**Accessible name — TRACKED FOLLOW-UP, open.** The toggle-chip variant's accessible name is
`"1folder"`, with no space: the number is in its own `<span>` and the separation on screen
comes from the flex `gap-1`, which the name computation does not see. It is pre-existing and
true of every chip on the strip (Endpoints' live/draft/disabled, Alerts' failing), not
something the singular rule introduced.

*Open*, not accepted. It is unfixed here because inserting a text node into an `inline-flex`
row adds a flex **item**, and the resulting spacing is a change no jsdom test can check (§10) —
so the fix needs a browser, which the task that found it did not have. It is not papered over
with a hand-written `aria-label`, which would hide the defect behind a second source of truth
for the same string.

**A defect encoded as an expectation becomes the convention**, which is why it is written down
in both places rather than only pinned. In
`src/components/explore/__tests__/SummaryStrip.test.tsx` the expected value is bound to a
constant named `DEFECT_NAME_MISSING_SPACE` rather than passed as the bare literal `"1folder"`:
a literal in an assertion is indistinguishable from a desired value, and the next person to add
a chip copies it. **What closes this:** render the separator, check the chip's spacing in a
browser at the sizes §5.3 uses, and rename that constant out of existence.

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

### 3.9 Copy that names a control is a claim about the tree, and is checked like one

A create page's subtitle usually says what happens next, and next is usually somewhere else.
That sentence is not decoration: a reader follows it. **`ServiceAccountCreate` shipped
"issue it OAuth2 clients and grant it endpoints from the Clients list", and the Clients list
has no grant affordance at all** — the account's group-header menu is `Issue client…` +
`Delete`, a client row offers `Revoke…`, and `useDataApiGrantEndpointInvoke` has exactly one
caller in the tree, `detail/AccessSection.tsx`. The reader went hunting for a control that
does not exist, which is strictly worse than saying nothing: silence sends nobody anywhere.

The rule, and the reason it is here rather than in one file's comment:

- **Name a destination specific enough to walk to, and no more specific than is true.**
  "Access" alone is a **tab on a detail route** — there is no sidebar item by that name, so
  it names a place reachable only through a parent it does not mention. The shipped wording
  is "on that endpoint's Access tab under Endpoints": `Endpoints` is a real sidebar link
  (pinned by `layout/__tests__/Sidebar.test.tsx`) in the same `Data API` group as the Clients
  page the reader is standing on, so the sentence hands them a first step.

  **An earlier revision of this bullet gave a different reason, and it was false — in the
  section written about exactly that failure.** It said `Access` was ambiguous with an
  `Access` sidebar group. That heading is in `CONTROL_PLANE_GROUPS`, and `Sidebar.tsx` picks
  control-plane groups **or** Explore groups on `isAdmin`, never both, so on
  `/explore/:ws/data-api/clients/new` there is no `Access` heading at all — for anyone, admin
  included. The check that would have caught it is this section's own: the claim was about a
  control, and the control was never rendered to look at. Its absence is now pinned
  (`layout/__tests__/Sidebar.test.tsx`, "no Access heading on an Explore route"), which is the
  general repair — **a reason nobody can render is a reason nobody can check.**
- **One fact, one greppable stem** — which is weaker than "one phrasing", and the weaker
  claim is the true one. The stem is **`Access tab`**, and all three shipped occurrences carry
  it (`ServiceAccountCreate.tsx`, `ClientCreate.tsx` created and spent). The **tails differ on
  purpose**: the created state and the subtitle append "under Endpoints" because neither
  carries a link, while the spent state stops at "Access tab." because its own sentence ends
  in a `Link` to the endpoints list — repeating the parent in prose beside a link to it is
  noise. So do not read this bullet as "these sentences are byte-identical". Read it as: the
  stem is fixed so `grep "Access tab"` finds every one of them, and each tail is answerable
  for itself.
- **A link is not automatically the better answer** (§3.6's neighbour). `ClientCreate`'s
  created state refuses one because leaving destroys a secret shown once;
  `ServiceAccountCreate` refuses one because it is a form with unsaved input and no draft
  persistence (§3.8), so a link in its subtitle would discard what was typed. Prose naming a
  reachable place is the floor; a link is an upgrade only where leaving costs nothing.
- **Where the copy binds two verbs, check both.** `TryIt`'s empty-account prompt read "Create
  one, then grant it invoke, in [Access]" and only the second half was true of Access —
  `AccessSection` has no create affordance, and in that exact state its own table renders the
  same "no service accounts yet" sentence, so the one link on screen led to the problem
  restated. Half a true sentence is a false one.

**The check is §5.1's, applied to copy: grep the class the claim is about, and reconcile every
hit.** Run from `frontend/` for this class:

```bash
grep -rn --include='*.tsx' -iE "Access tab|Access section|Clients list|Clients page|Endpoints list|Endpoints page|actions menu|row menu|in Access|in Clients|under Endpoints" src/routes/explore/dataapi
```

**63** hits on the current tree, of which **7** are shipped user-visible copy —
`ClientCreate.tsx:225` (created state), `:266`, `:276` (spent state),
`ServiceAccountCreate.tsx:128` (the subtitle), `detail/TryIt.tsx:423` and `:425` (the
empty-account prompt) and `:659` ("Grant it in Access", a real `Link` to `accessHref`) — and
each of the seven names a place that exists. The other 56 are **21** code comments and **35**
test lines, neither of which a reader follows. 7 + 21 + 35 = 63.

**That total was 60 when this section was written, three commits ago, and the seven did not
change** — the drift is entirely comments and test lines, added by the very review that
corrected this section. Recorded rather than quietly overwritten, because it is the shape of
the staleness §5.1 warns about and because it shows what the count is FOR: the number that
must be re-derived and defended is **7**, the shipped sentences a reader can follow. The other
two numbers only have to add up. A second sweep,
`grep -rn --include='*.tsx' -i grant src/routes/explore/dataapi` with the identifiers filtered
out, is what surfaced the `TryIt` instance; this section's terms alone would not have caught
it, which is the argument for running more than one.

**And this grep is not a complete census of shipped copy that names a place — do not use it as
one.** Two true sentences fall outside its terms: `ClientCreate.tsx:279` ("pick the endpoint
from Endpoints", the spent state's link) and `detail/TryIt.tsx:624` ("appears in Activity's
call list under \"Include trial runs\"", whose control is the checkbox beside `<h2>Calls</h2>`
in `ActivityCallList.tsx`, deliberately not gated on `hasCalls` so it is present in exactly
the state that sentence renders in). Both were checked by hand and both hold. The terms above
are a **starting sweep tuned to the two defects this task found**, and a claim that copy is
clean rests on the hand pass, not on the grep returning what you expected.

**The two instances were one screen apart and neither review caught the other**, because the
false claim was copied before anyone checked it: `ServiceAccountCreate`'s own doc comment
placed both affordances "on the Clients row", and the subtitle was written from the comment.
Where a comment and the tree disagree, the tree is the fact — the same order §5.1 states for
this document.


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

`src/routes/explore/dataapi/detail/ResponseSection.tsx`. The section's SETTINGS are one
`ConfigTable` with group rows — `Response transform`, then `Output shape` — and two blocks
are not rows of it. The reason those two are out is the durable part of this section:

- **Frozen columns** — a read-only `Name | Type` listing of the *published* contract. There
  is no Value to edit. Forcing it into the shared table would either fabricate a Value column
  or nest a table inside a cell, and would make read-only output look editable.

  **"Nest a table inside a cell" means inside a cell that must ALIGN with its siblings**, and
  that exemption is now load-bearing, because this same tab nests one: `ReturnsEditor`'s grid
  sits inside the transform's `colSpan={3}` row. Nothing aligns across a full-width row — it
  spans every column, so there is no neighbour for its contents to line up with — and the
  objection here is precisely about a listing whose `Name` column would have to agree with
  `Setting` while its `Type` column agreed with `Value`. A nested table in a spanning row is not
  the thing this bullet forbids; a nested table in a `Setting`/`Details`/`Value` cell is.
- **OpenAPI document** — a paragraph, a download button, and a derived base URL. An action,
  not a setting.

**The rule:** the complaint that removed the `ConfigSection`s was about cards framing
near-empty tables (§4.1), not about cards.

#### The premise for `Frozen columns`, measured rather than argued

The two arguments above are about kind. When the tab was re-read against "one table with
group rows" there was a third option nobody had weighed — `Setting | Details | Value` maps
onto `name | why | type` without fabricating anything, because the type genuinely IS the
listing's value — and it loses on geometry, which is checkable:

`ColumnTable` is `table-fixed` with `w-72` on Name, so a type starts **288px** after the name
it describes, at any table width. That number is the whole reason the frame was rewritten:
under auto layout inside `max-w-xl` it was 444px, and "`bigint` sat far from the name it
describes" was the reported complaint. In a `ConfigTable` the Value column sits after
`Setting` (288) **and** `Details` (624 at a 1296px content box), so the type would start
**912px** after the name — three times the distance the current frame exists to produce. A
`Details` cell cannot collapse per row; the column is sized once for the table.

So the conclusion survives, and it now rests on a number instead of only on a kind.

#### The `Response transform` block, which was NOT one of the two and has moved in

Until the detail-tabs pass this section shipped beside `Output shape` as a titled block of its
own: an `h3`, a paragraph, a checkbox, and — only when enabled — a Monaco editor and an
editable returns grid. Its recorded argument was that "the script and its returns table are
the `ParamEditor` shape … not a single `Setting | Details | Value` row", which is true of the
script and the grid and was never true of the **toggle**. The toggle is a checkbox with a
sentence of help beside it, which is the shape this table exists for, and on an endpoint with
no transform the toggle is the *only* part of the block on screen — so the tab was carrying a
titled block, permanently, to house one checkbox.

It is a `ConfigTableGroup` + one `ConfigTableRow` now, **above** `Output shape` because
turning it on removes `Shape` and `Naming` from the rows below it, and a control whose effect
is to delete two rows reads above them. The script and the returns grid take a **full-width
`colSpan={3}` row of the same table** — the device that table already used for the
`SettingsNote` standing in for those two rows — because `valueClassName` is 384px at `lg` and
a code editor is not a 384px thing. The row is not rendered at all without a transform.

**What this does NOT license.** "It is a checkbox, so it is a setting" is the argument, not
"fold everything in". The two blocks above are still out, and the OpenAPI block stayed out on
its own kind while changing shape (§4.2a).

#### 4.2a An action at the end of a settings tab has a shape, and `Config` already had it

`OpenAPI document` is an action and is not a row of the table — unchanged. What changed is the
shape it takes outside the table. It was a heading, then a paragraph, then a button on a line
of its own: read down the page that was a third block layout on a tab that already had a
read-only listing and a settings table, which is the complaint the whole detail-tabs pass
answers ("Config is fine, the other tabs are not").

`Config.tsx`'s `Validate` block is the same KIND of thing — an action at the end of a settings
tab — and it is `space-y-2 border-t border-border pt-4` with the heading and a `size="sm"`
button on ONE line and whatever the action produced beneath. `ResponseSection`'s OpenAPI block
takes that shape, and `Versions`'s `Publish saved draft` takes it too (§5.1). Three call sites,
one shape, on the tab set whose complaint was that it had four.

**Guarded as a shared parent, not as a class**: `EndpointDetail.test.tsx`'s "ends the tab with
the OpenAPI action on its heading line" asserts `button.parentElement === heading.parentElement`
and that that parent holds no `<p>` — the second half is what stops the section root from
satisfying it, since the root does hold one.

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

**Why this is not the drawer mistake.** §11.2 used to record the opposite RULING for `TryIt`'s
warning — that it could not be collapsed at all — on the rule that a reader who must open
something to learn a call is real has already pressed the button. **§11.2 has since reversed
that ruling, and `TryIt.tsx` ships both of its blocks as `Caveat`s.** The RULE is what holds
here, not the ruling it used to support, and the split is what satisfies it — the load-bearing
fact is the summary and is on screen with nothing clicked. What is collapsed is the *why* and the
*where to go instead*, which is what a reader goes looking for once the headline has stopped
them. **Collapsing the headline would be the mistake; collapsing the paragraph under it is
the point.** ("The split" here is the split between summary and body, not §11's layout one.)

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

**Which test governs, because this section states two and they were read as disagreeing.** The
rule above is a KIND test — an explanation of a consequence of acting goes behind a disclosure
headed by that consequence — and the bullet just above it reads as a SIZE test. **The kind test
governs; size decides nothing on its own.** A `Caveat` is a two-part form, a consequence and the
explanation under it, so the question is not "is this long?" but "**is there a second part to
put behind the summary?**" Where there is none there is no caveat to collapse, only a line of
text — which is why the six-word transient above stays plain, and it would stay plain at sixty
words.

Read that way the shipped call sites stop disagreeing with each other. `Access`'s conditional
states — "Revoking may not remove access", "Nothing here to revoke" — each have a consequence
and a mechanism (or a where-to-go-instead), so they are `Caveat`s (`AccessSection.tsx`).
`TryIt`'s published-version note is two sentences of one fact with no separable why, so it is a
`<p>`. Neither is decided by length.

**An action column's header is `sr-only`.** A visible "Actions" heading over one small
button is louder than what it labels; an unnamed column is worse for anyone not reading it
visually. `getAllByRole("columnheader")` still sees the name, which is what the shape tests
assert on.

---

## 5. Width

### 5.1 One width for all six tabs of a detail route

`src/routes/explore/dataapi/EndpointDetail.tsx` — the `<Outlet/>` wrapper carries no
`max-w-*`, **and neither does any of the six sections it hosts**. There is a test per section
saying so, and one in a real browser saying they agree.

**The first defect.** That wrapper used to carry `max-w-3xl`, which left the right third of the
page empty while Config's Details column wrapped to three lines. A single page-level cap was
the wrong instrument — the sections did not all want 768px — so the cap moved out of the route
and into the sections that wanted one. Query gained the most: its SQL editor went from 768px to
1424px (measured during the width pass, not by this document; the same applies to the figures
in §5.4 and §5.5).

**The second defect, which is what this section now records.** Moving the cap into the sections
fixed the page and broke the tab set. Measured on the deployed build at 1600×1000 with seeded
data, all six tabs get the same 1296px content box, and the widest block in each was:

| tab | available | widest block |
|---|---|---|
| Query | 1296 | 1296 |
| Config | 1296 | 1296 |
| Response | 1296 | 1296 |
| Access | 1296 | **1024** |
| Try it | 1296 | **1024** |
| Versions | 1296 | **1024** |

**The content edge moves 272px when you switch tabs** — three times, in a six-item nav, with
nothing on screen to explain it. So: **every section of a detail route leaves width to the
page.** `AccessSection.tsx`, `TryIt.tsx` and `Versions.tsx` gave up their `max-w-5xl`;
`Config`, `Query` and `ResponseSection` already had none.

#### The part of this section that was reversed, and why

Until this task §5.1 said the opposite, and said it as a considered ruling rather than an
oversight. **The criterion was what a section HOLDS:** Config and Query use the page because
they hold the widest controls of any tab and a SQL editor; Access, Try it and Versions cap
themselves at `max-w-5xl` because they hold prose, short-column lists and short controls **that
read worse stretched across a wide monitor**. §5.4a and §5.4b are that criterion being applied
carefully — Response was measured as the second kind, capped, and then moved to the first kind
when what it held changed.

**Nothing in that argument became false.** Each of the three sections still reads better at
1024px than at 1296px taken on its own; §5.4a's measurement of Selects sitting ~1260px from
their labels is still a real complaint, and `Versions` really is five short columns.

**What it lost to is that a reader does not see one section, they see six in sequence.** The
per-section optimum and the whole-route consistency were both in the ruling's scope and only
one of them was measured. A 272px jump between siblings is a defect a reader can see without
knowing anything about the page; a section being 272px wider than its ideal is one they
cannot. When those two conflict, the route wins.

**Two things follow, and they are what keeps this from being a taste argument:**

- **The per-section complaints do not disappear; they move to the instrument that fits them.**
  A section-root cap was answering ALL of them with the one tool that also breaks the tab set;
  each has a narrower instrument. **There are THREE such moves, not two. Two are DONE and the
  third is WITHDRAWN — measured, not asserted, and the difference between those two words is
  load-bearing.** An earlier revision of this bullet named only two and
  reported "one done, one owed", which was the fourth time this document described a rule its
  own tree did not implement; a later one carried three with the row-action move open on
  `Versions`. Both are closed as of the detail-tabs pass — the third by being **withdrawn as
  mis-specified**, not by being fixed: `Roll back` is at x=1470 before and after. Read the bullet,
  not the label. The check that finds any of this is
  the one this section prescribes — **grep the class the rule is about, reconcile every hit,
  then check your count against the list you just wrote**. Run for the row-action move over
  `src/routes/explore/dataapi`, `text-right` returns **19** hits: **two** are action columns and
  both are fixed (`detail/AccessSection.tsx:414` and `detail/Versions.tsx:233`, plus each one's
  matching body cell, 721 and 317); ten are numeric columns in `Activity.tsx` and
  `ActivityCallList.tsx` (Calls, Error rate, p50/p95, Cache-hit rate, Duration, and their
  `tabular-nums` cells); two are `Clients.tsx`, a list page rather than a detail route; **two**
  are prose — `Versions.tsx:113` and `:135`, its own comment explaining why widening a
  `text-right` column moves nothing; and one is a test asserting the header
  (`__tests__/EndpointDetail.test.tsx`).
  2 headers + 2 body cells + 10 numeric + 2 list-page + 2 comment + 1 test = 19.

  **This enumeration had gone stale, and the way it went stale is the argument for the check.**
  It read 17 with `2+2+10+2+1`, and it was correct when it was written at `2fc228d0`. Then
  `fa89ab73` — a fix round of the same task — added a second comment line about `text-right` and
  moved the two cited sites down the file, so by the time the branch was reviewed the doc named
  a count the tree did not return and cited two line numbers (`:224`, `:289`) that no longer
  pointed at anything. Nobody re-ran the check after the last commit that could invalidate it.
  **A prescribed check is only worth the last time it was run**; re-run this one whenever a
  commit touches either file, not only when the rule changes.

  - **Prose: DONE.** "The sentence sets as one very long line" is a PROSE measure and
    `max-w-prose` answers it (§5.2, "cap the prose, not the column"). Every section intro
    carries it. Measured in Chromium: each intro went from filling the content box to 532px
    (Query 419px, which is its text ending before the cap). **This was a regression introduced
    BY the reversal and fixed in the same breath** — three of those six intros were bounded at
    1024 by the caps this section removed, so uncapping the roots un-capped them too, and at
    1920 with the rail collapsed they ran past 1700px.
  - **Column width: DONE.** "The picker is a screen away from the label" is a COLUMN width and
    `ConfigTable`'s `valueClassName` answers it — the resolution §5.4b reached for `Response`,
    which carries `w-52 lg:w-96`. `Try it` kept the `w-52 lg:w-80` it had before the reversal for
    two tasks and got §5.2's trap in full: measured with and without its cap, `Details` — the only
    unsized column — went **408 → 680px, taking all 272 freed pixels**, while `Value` stayed at
    320px, so the picker was no further from its label than before and no closer, and the width
    went to muted help text.

    It now carries `w-52 lg:w-96`, the same as `Config` and `Response`. Measured in Chromium at
    1600×1000, **against the post-reversal row** (§5.1's labelling rule — the baseline is the
    uncapped section, not the capped one):

    | | Setting | Details | Value | table |
    |---|---|---|---|---|
    | `Try it` before, `lg:w-80` | 288 | **680** | 320 | 1288 |
    | `Try it` after, `lg:w-96` | 288 | 616 | **384** | 1288 |
    | `Response` / `Config`, `lg:w-96` | 288 | 624 | 384 | 1296 |

    The 64px moved from `Details` to `Value`; the 8px `Details` still differs by is `Try it`'s own
    `px-1` inset on both of its halves, and `Details` is the unsized remainder that absorbs it.
    The dead space in `Details` — the part its own `max-w-prose` sentence can never occupy, which
    lays out at 532px — went **148px → 84px**.

    **A wider column measures better in isolation and is deliberately not taken, which is the
    point of recording the number.** At `Value` = 448px the caveat's summary in that cell drops
    from two lines to one (the row 85 → 69px) and the dead space in `Details` falls to 20px; at
    512px `Details` (488) is finally narrower than the 532px measure and starts squeezing the
    prose, so 448 is the local optimum and it is real. It loses to this section's own finding:
    three adjacent tabs render one `Setting | Details | Value` grid, a reader sees them in
    sequence, and a fourth width for one grid is the defect §5.1 was reversed to remove in
    miniature. It is also worse where the breakpoint engages — at a 1024px viewport `Details` is
    already down to 40px at `lg:w-96`.

    **Reconciling every hit of the grep, as this section prescribes.** `valueClassName` is passed
    at **seven** call sites — 3 + 3 + 1, and an earlier revision of this paragraph said "six"
    while enumerating seven. The enumeration was complete and the arithmetic was not, which in a
    section whose whole subject is "grep the class and reconcile every hit" is the failure itself
    in miniature: **state the count and then check it against the list you just wrote.** Three are
    the detail tabs this rule governs and they now agree:
    `Config.tsx`, `ResponseSection.tsx`, `TryIt.tsx`, all `w-52 lg:w-96`. Three are
    `ScheduleEditor.tsx` at `w-52 lg:w-80`, which is the Alerts editor and not a detail route —
    out of this rule's scope, named so that a future grep does not read them as violations. The
    remaining hit is the one worth writing down: **`Query.tsx` passes `w-64` and no
    `detailsClassName`.** That is a fourth width and an uncapped Details cell, and it is left
    alone on purpose — it is a one-row strip inside a collapsed `<details>` that already carries
    its own `max-h-[40vh]` scroll box, not a section-level settings grid — but "left alone on
    purpose" is a decision, so it is recorded rather than omitted.
  - **Row action distance: WITHDRAWN on BOTH tabs; what is DONE on `Access` is row height and
    identity legibility — and both withdrawals corrected a claim in this document.** The label
    read "DONE on `Access`" until the live check measured the same metric there and got 643px
    (see below); the distance was never the thing either fix moved. "Withdrawn" and not "done":
    the metric the old bullet named did not move. `Roll back` is at **x=1470 both before and after** this task, and no
    column sizing can move it. What changed is that the complaint was re-measured and found to be
    about the wrong distance, and the defect that IS there — the width piling into `Note` while
    `Who` cut addresses — was fixed with `Access`'s instrument. A reader who greps this list for
    open items should take "withdrawn" as "the record was corrected", never as "the button
    moved". This is the move the earlier revision omitted entirely, and it is the one the
    reversal actively CAUSED rather than merely failed to help. Both tabs put their row action in
    a narrow, right-aligned last column of a `table-fixed` table, so every pixel the section
    gained went to the one unsized column. The instrument is neither of the two above: it is
    **sizing the row table's own columns** so the width lands on the columns that want it.
    `AccessSection.tsx` does that — `Service account` went `w-48` → `w-48 xl:w-96` and `Actions`
    `w-36` → `w-52`, on the measurement that `Invoke`, the unsized column, laid out its widest
    row at 183px while being handed 960.

    `Versions.tsx` now does it too — `Who` went `w-56` → `w-56 xl:w-80` — and getting there
    found that the sentence this bullet used to carry about it was measuring the wrong thing.
    Measured in real Chromium at 1600×1000 against a five-version fixture, **with the removed cap
    put back as a negative control**:

    | | Version | Note | Who | When | Actions | Roll back at |
    |---|---|---|---|---|---|---|
    | capped (1024px), the pre-reversal layout | 128 | 416 | 224 | 128 | 128 | x=1198 |
    | uncapped (1296px), before this task | 128 | **688** | 224 | 128 | 128 | x=1470 |
    | after this task | 128 | 592 | **320** | 128 | 128 | x=1470 |

    The 272px is real and reproduces exactly. **What it is not is a gap the reader crosses**: the
    distance from a row's last TEXT to its action is **83px** in every one of those three states,
    because `When` sits immediately left of the action column and is never empty.

    **And that 83px is structurally forced, not merely observed** — which is the strongest form
    of the claim and the one the first revision of this bullet under-stated. `When` is a fixed
    `w-32` column immediately left of a `text-right` action in a `table-fixed` table, so the gap
    is `158 − (the width of "a month ago")` **at any viewport**: 128px of `When` plus the action
    cell's 8px padding and the button's own inset, none of which depends on how wide the section
    is. The cap could never have improved it and removing the cap could never have worsened it.

    On the same metric `Access` measures **643px** after its own fix. That figure used to be
    marked "indicative — a one-account stub whose `Invoke` cell holds two badges, not a number to
    quote". **It is now a measurement**: taken on the deployed kind build at 1600×1000 against
    the `demo` workspace's EIGHT real service accounts, whose `Invoke` cells hold ONE badge each,
    the distance from a row's last text to the left edge of `Try it` is 643px on all eight rows.
    The stub was right, and it was right for a reason the stub could not show.

    **And it is structurally forced in exactly the way `Versions`' 83px is** — which is what
    settles the comparison rather than merely pointing at its direction. `Invoke` is a sized
    `xl:` column (704px of the 1296px table) immediately left of a `text-right` action, so the
    gap is `704 − (the width of the status badge) + the action cell's inset` at any viewport at
    or above `xl`. Sizing the row's columns moved it — `Invoke` was handed 960px before — but no
    column sizing closes it, because the width `Invoke` keeps is width its OTHER states need: the
    `granted` row carries a caveat disclosure, and the `unchecked` row carries a full sentence.

    **So the label above says "DONE on `Access`" about row HEIGHT and identity legibility, not
    about action distance.** On action distance the complaint is withdrawn on BOTH tabs, for the
    same reason: a left-aligned column beside a right-aligned action produces a gap that is a
    property of the column, not of the section's width, and neither the cap nor its removal was
    ever going to touch it. `Access` differs from `Versions` only in the size of the number
    (643px against 83px), because its neighbouring column is 704px wide and `When` is 128px.

    What the 272px actually did was pile into `Note` — 680px of blank in the MIDDLE of a row
    whose note is empty — while `Who` cut realistic addresses (a 33-character address lays out at
    231px against 224px of column) on two of five rows. That is the same defect `Access` had, in
    the same place, and the same instrument fixes it: the width goes to the column that is
    cutting its content. A note is unbounded prose that no finite width fixes and it keeps its
    `title`; `Note` therefore stays the unsized remainder.

    **Two things measured and deliberately NOT done**, recorded so they are not re-opened:

    - **Widening `Actions` moves nothing.** The cell is `text-right` and the button is 82px, so
      its left edge is `columnRight − 8 − 82` at any column width. `w-32` → `w-52` leaves Roll
      back at x=1470. (On `Access` that same widening DID matter, and for a different reason: it
      has two controls and they were wrapping to two lines.)
    - **Sizing all five columns does not bound `Note`.** Under `table-layout: fixed` a shortfall
      is shared PROPORTIONALLY — measured in Chromium, `128/512/288/128/128` inside a 1296px box
      lays out as `140/553/312/140/140`, so every column including `Actions` grows and `Note`
      still takes the largest share.

    **The residual is not a column-sizing problem and is not owed.** A right-aligned action in a
    full-width table sits at the page's right edge; that is what the shape means, and every
    action column on this route shares it. Left-aligning this one would fix the arithmetic and
    make `Versions` the only tab in the set that aligns its action differently — the defect §5.1
    was reversed to remove, in miniature.

    **State the baseline with the number, because there are two and they disagree by 272.**
    `Access`'s three columns at a 1600px viewport, `Setting | Invoke | Actions`:

    | | identity | Invoke | actions | available |
    |---|---|---|---|---|
    | before the reversal (capped) | 192 | 688 | 144 | 1024 |
    | after the reversal | 192 | **960** | 144 | 1296 |
    | after the row fix | **384** | 704 | **208** | 1296 |

    Both of these are true and they are not the same sentence: measured against the row before it,
    the fix moved **192 to identity, 64 to actions and −256 from Invoke**; measured against the
    pre-reversal layout, the net is **+192 identity, +64 actions, +16 Invoke**, which is the 272
    the reversal freed, finally spent.

    **This is a labelling rule, not a correction — nothing above was ever miscalculated.** Every
    row sums to its own available width and both delta sets are right against their own baseline.
    The rule earns its place anyway: a review of this programme raised the two figures as an
    arithmetic error, checked them, and withdrew it. That is the cost of an unlabelled baseline —
    a correct number that reads as wrong, and reviewer time spent proving it is not, with a real
    chance that someone eventually "fixes" it. **Whenever a width delta appears in §5, it says
    which row it is measured from.**
- **A section may still cap its own CHILDREN** (§5.5), and one does: `ResponseSection`'s
  editable `returns` grid keeps `max-w-xl`, because a row of four form controls stretched wide
  puts the delete button an arm's length from the row it acts on. That is the narrow, still-live
  form of the argument this section reversed. As of this task it is the only non-prose
  `max-w-*` under `detail/`.

**This document has twice shipped a §5 section describing a rule its own reference
implementation violated** — the "Response reads worse stretched" sentence that outlived §5.4b
by four revisions, and §11.2's claim about `Access`'s paragraphs. The check that would have
caught both is mechanical and is the check this task ran: **grep the tree for the class the
rule is about, and reconcile every hit with what the section says.** Where this document and
the tree disagree, the tree is the fact and the document is the bug.

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

**A gate is not always the safe default, and `Access`'s two widened columns gate DIFFERENTLY on
purpose.** The rule above cuts both ways: because `table-fixed` scrolls rather than shrinks, a
column that is wide at every width costs the narrow regime, but a column that is *gated* leaves
the narrow regime with the layout the widening was meant to fix. Which one wins is a
measurement, and the two columns measured opposite:

- **Identity is gated, and at `xl` rather than `lg`.** The widened column has to leave `Invoke`
  enough room at the breakpoint it turns on. Measured available width: at a 1280px viewport
  976px, so Invoke keeps 384px; at 1024px it would be 720px, leaving Invoke 128px against content
  that lays out at 183px. `lg` would have traded a truncated address for a wrapped badge.
- **The actions column is UNCONDITIONAL**, and the asymmetry is the measurement, not an
  oversight. Gated, the action group wraps onto two lines everywhere below `xl`, and two stacked
  32px controls are TALLER than the 32px-plus-16px stack the change removed: seven rows measured
  **651px gated at a 1024px viewport, against 451px ungated and 543px before the change**. So the
  gate would have cost height at exactly the widths where height is scarcest.

`AccessSection.test.tsx` asserts the actions column carries no responsive prefix at all, which is
a deliberate **ban on re-gating it** rather than an incidental class check — somebody tidying the
two columns into a matching `xl:` pair is the specific regression it exists to stop. Recorded
here because a ban whose only rationale lives in a test comment reads as pedantry at the moment
someone has a reason to violate it.

### 5.4 A dead column below `lg`, introduced by this phase

Recorded so nobody "fixes" the widths without knowing about it, and documented in full in
`Config.tsx`'s comment. `ConfigTableGroup` spans `colSpan={3}`, which keeps the table three
columns wide even where Details is `display:none` — so fixed layout hands the leftover to a
phantom third column with no first-row cell, and it lands as dead space to the **right** of
Value. Measured at a 1010px viewport (content box 706px): columns 288/0/208 with 210px dead;
deleting the four group rows from that same live table gives 410/0/296 and no dead space.

**It is not pre-existing, and three places in this document used to say it was.**
`ConfigTableGroup` is new in this phase, and at the time this was written it was the only source
of `colSpan={3}` inside a `ConfigTable` in the tree:
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

**That "only source" is no longer true, and this branch is what falsified it.** The detail-tabs
pass moved `Response transform` into the grid, so `ResponseSection.tsx` now renders TWO group
rows (`:869` `Response transform`, `:955` `Output shape`) **and two hand-written
`<TableCell colSpan={3}>` rows of its own** (`:907`, `:967`) carrying the transform's editors —
plus three more in `AccessSection.tsx` (`:427`, `:437`, `:449`) and one in `TryIt.tsx` (`:477`).
`Config.tsx:109` says "only source" too and is wrong for the same reason. **Both sentences are
left standing above with their date rather than quietly edited**, because the fact they record —
that the phantom column arrived with this phase and is not inherited from `main` — is still
true, and it is the fact §5.4 exists to carry.

What changes for a reader re-tuning widths: the phantom column is produced by **any**
`colSpan={3}` row, not only by `ConfigTableGroup`, so counting group rows undercounts it.
`ResponseSection` has the same phantom column from **two** group rows plus two editor rows, not
from "its one `Output shape` group row" as this section used to say.

### 5.4a The section that fell between two tasks

`ResponseSection` was built before there was a route cap to remove, and the width pass measured
`Config` plus the tabs it rebuilt — Response was in neither set, so it shipped uncapped. Measured
at a ~1470px content box: Setting 288 / Value 208 / Details ~975, with `max-w-prose` stopping the
sentence near 585px — ~390px of empty column, and the Shape and Key-naming Selects ~1260px right
of the labels they answer to, where the old route cap put them at ~560px. That is the same
complaint that earned `Versions` its cap (`Versions` has no cap any more — §5.1's reversal took
all three — but it had one when this was written), so the fix wave gave Response the same `max-w-5xl` root
(~816px), which also reined in the `Frozen columns` and `OpenAPI document` Cards — the frozen
*table* had `max-w-xl`, its Card did not. (Those two Cards are gone as of §4.2's reversal. The
measurement is kept: it is why the section root WAS capped, and "the frame stretched further than
the content it framed" was one input to removing them. **The cap itself is gone as of §5.4b** —
not because this measurement was wrong, but because the thing measured changed: the fix for the
Selects sitting far from their labels is `valueClassName`, which did not exist on `ConfigTable`
when this was written.)

The general rule: **a task that rebuilds a surface and a task that measures widths must agree on
the same list of surfaces**, or the difference between the two lists ships.

### 5.4b The same section, measured again after what it holds changed

The 2026-08-28 Data API UI audit measured `Response` in a browser and reported four content
widths inside one section, with a gutter nothing could use. Reproduced in Chromium at 1600×1000
with the sidebar expanded, against the shipped stylesheet:

| | width | right edge |
|---|---|---|
| content box | 1296 | **1560** (was recorded as 1576 — see below) |
| section root (`max-w-5xl`) | 1024 | 1288 |
| frozen `Name \| Type` table (`max-w-xl`) | 576 | 840 |
| `Setting \| Details \| Value` table | 1024 | 1288 |
| the two block paragraphs (`max-w-prose`) | 532 | 796 |

**One number in that table was wrong, and the arithmetic is what settles it.** The content box's
right edge was recorded as 1576, which puts its left edge at 1576 − 1296 = **280**. Every other
row implies **264**: 1288 − 1024, 840 − 576, 796 − 532. A child box cannot start 16px to the
LEFT of the parent that contains it, so 1576 is the outlier and the true right edge is **1560**.
The gutter is therefore **272px**, not the 288 this section derived from the bad figure — and
272 is also what the deployed-build audit in §5.1 reports (1296 − 1024) and what a re-measurement
in Chromium reproduces. **Nothing downstream moves:** every conclusion in this section was
computed from the WIDTHS, which were right; only the one derived gutter figure was wrong.

**The cap was not wrong when it was applied; the section stopped being the kind of section it was
applied to.** §5.4a capped it as a prose-and-short-lists tab. It has since gained a
`Setting | Details | Value` table (§4.2) and a Monaco editor for the response transform — the two
things §5.1 THEN gave as the reason Config and Query use the page. (Those are now ONE thing: the
transform is a group of that table and its editor is a full-width row of it — §4.2. The four
blocks this section counted are two plus a read-only listing, and only the listing and the
`max-w-prose` sentences are not on the page's measure.) (That what-a-section-HOLDS
criterion is retired: §5.1 now says one width for all six, so there are no groups to move
between. This section is kept because it is still the reason THIS cap came off first, and
because everything below it about `valueClassName`, the frozen table and the `lg` regime is
unaffected by the reversal.) So the cap came off, and:

- **Removing it is half the change** (§5.2). `ConfigTable` is `table-fixed`, so all 272 new pixels
  would have gone to Details, the only unsized column, and the three Selects would have stayed at
  208px. With `valueClassName="w-52 lg:w-96"` — the pair Config already uses — the trigger went
  192 → 368px and Details 528 → 624 rather than to 720.
- **The frozen table's cap was not buying what it was added for.** The rationale for `max-w-xl`
  — that `bigint` sat far from the name it describes — was never in this document; it is in
  `ResponseSection.tsx`'s own comment on that table, which is also where the correction now
  lives. (§5.4a mentions the cap and does not give that reason; §5.1 did too, until this task's
  rewrite dropped every mention of a section-root cap. Citing a section for a
  rationale it does not contain is the §9 failure happening to a cross-reference: the claim
  reads as sourced and is not.) Measured: `w-full` AUTO layout inside that cap gave
  `Name` 443.9px of 576, so the type started 444px in and would drift further with the longest
  name. `table-fixed` with `w-72` on Name puts it 288px in at any width — closer, stable, and the
  same first column the settings table below uses, so the two read as one grid.
- **Prose is the one deliberate exception to a single measure.** The paragraphs still stop at
  `max-w-prose` (§5.2's "cap the prose, not the column"): a frame can span whatever it is given,
  a line of running text cannot. Widening them too is the obvious wrong fix, so the real-browser
  spec asserts they stop short rather than skipping them.
- **Below `lg` nothing changed**, which is the point of §5.3's `lg:`. Measured at 900px: Value is
  208px before and after, and the old cap never bound there either (the content box is 628px).

Two things to carry from this. **A recorded cap binds its measurement, not its conclusion** —
the §4.2 reversal made the same point about a recorded rationale. And the frozen table is the
second case in this document where five rounds of reading classes described a table's geometry
wrongly (§10): the cap everyone cited as fixing the Name/Type gap was leaving 444px of it.

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
test and only that one.

**All six sections now carry that same inverse assertion**, since §5.1's reversal left one rule
instead of two. `Response` moved to it in §5.4b; `Access`, `Try it` and `Versions` moved to it
with the reversal, and each test states the reversal in its own name rather than quietly
changing meaning. `Query` had NO guard at all until then — it was the tab the width pass was
for, so nobody thought it could regress, which is the reasoning that had left `Config`
unguarded. "All six leave width to the page" is not a claim five assertions can make.

The test's own comment is honest about its limits: the class-name assertions are weak and are
meant to be — they exist only so a cap cannot come back, or vanish, silently. **Whether the
width is well spent is a question only a browser can answer.** See §10.

### 5.6 The section guard is one helper, and now one form

`src/routes/explore/dataapi/__tests__/sectionWidth.ts` — `expectSectionLeavesWidthToThePage` and
`expectSectionIntroCapsItsProse`.

Three sections had the walk in §5.5 copy-pasted and **two had no guard at all**: `Try it` and
`Access`. That is how `Try it` shipped a `max-w-5xl` nothing was holding in place — §5.1 listed
it among the sections that cap themselves, and nothing said so in code. The walk, the host
resolution and the regex now live in one module.

**Six call sites, not five, and one exported form, not two.** §5.1's reversal changed both
numbers, and they moved for different reasons:

- **The count went 5 → 6.** `Query` was the only section with no width guard of any kind. It
  was the tab the width pass existed for, so nobody thought it could regress — the same
  reasoning that had left `Config` unguarded until a restored `max-w-3xl` on its own root
  passed 230/230. Under one rule for all six, five assertions cannot state it.
- **`expectSectionCapsItsOwnWidth` was DELETED, not left unused.** After the reversal it had
  zero callers. Keeping it "for a future section" was the alternative and was rejected: an
  exported helper named *caps its own width* is an executable statement that some section is
  supposed to, so a reader would reasonably infer the two-group rule still stands. That is
  precisely the failure §5.1 was reversed to stop — a description outliving the thing it
  described — and it is the third time this document has had to fix one. A section that
  genuinely needs a cap on its own children has §5.5 and `ResponseSection`'s `returns` grid as
  the live precedent; it does not need a dormant helper.

One detail from the deleted form was preserved in the module comment because it is a trap and
not a preference. **The two regexes differed on purpose.** The deleted positive form was
unprefixed (`/(?:^|\s)max-w-/`), because a section capped only at `lg:` is uncapped on exactly
the narrow viewports nobody measured. The surviving negative form includes responsive prefixes,
because `lg:max-w-3xl` wastes the page on exactly the wide ones the pass *did* measure.
Reinstating the positive form as a copy of the negative one's regex would be the bug.

**A third export, and the one case where a class assertion is the RIGHT tool.**
`expectSectionIntroCapsItsProse` pins each section intro's `max-w-prose` (§5.1). Everywhere else
in §5 a class name is a weak stand-in for geometry — but this rule *is* the class: §5.2's "cap
the prose, not the column" says the paragraph opts out of the section's measure, and carrying
`max-w-prose` is the whole of what it must do. jsdom cannot measure the width; it does not need
to.

It exists because for a while **five of the six intro caps were pinned by nothing runnable** —
the only guard was two Playwright specs that had at that point never executed, so deleting the
class from `Query`, `Config`, `Response`, `Try it` or `Versions` left all 5025 tests green.
(Those specs have since been run, §5.7; the helper is still the reason a deleted cap fails in
the suite everyone runs rather than only in the one nobody does.)

**By identity, never by population — this was got wrong twice, in opposite directions.** The
browser spec first left its prose loop unguarded ("not every tab has capped prose"), which is a
`for` over a possibly-empty list. The fix for that was `proseRights.length > 0` — **also wrong**,
because `Response` and `Access` each render THREE unconditional `max-w-prose` paragraphs, so
deleting the intro's cap leaves two behind and the count still passes. Only `Query`, with exactly
one, was ever genuinely guarded, and a guard that holds for one of six is indistinguishable from
one that holds for all six. **A count over a population can be satisfied by any member of it**;
both the jsdom helper and the browser spec now resolve the intro as a specific element — the `<p>`
beside the section's own `h2` — so only the thing the rule is about can satisfy the assertion.

**The host is resolved, not assumed**, so the same helper works under the real `EndpointDetail`
shell (walk stops at the `fieldset`) and under a harness that stubs the context and renders the
section under a bare `Outlet` (`TryIt.test.tsx`), where the section root is a child of the
render container. And the walk is a walk, never `parentElement.parentElement`: overshooting onto
the `fieldset` is a SILENT pass, because the fieldset carries no `max-w-*` whatever the section
does — so `sectionRootOf` hard-fails on it.

### 5.6a Mock state leaks between `describe`s, and the two things `mockReset` finds

`src/routes/explore/dataapi/__tests__/EndpointDetail.test.tsx`, `describe("EndpointDetail — width")`,
and `TryIt.test.tsx`'s file-level `beforeEach`.

`frontend/vitest.config.ts` sets no `clearMocks` and no `mockReset`, so a `mockReturnValue` set in
one block survives into the next. Two blocks were passing on exactly that. Measured on the clean,
all-green tree: the four Data API detail files give **185 passed / 0 failed** unfiltered and
**5 failed** under `-t "width"`, every one a `TypeError` for a hook that returned `undefined`. It
took **six** mocks to make the width block self-contained, each surfacing only after the previous
was fixed.

**No `-t`-filtered count in this suite is a result**, and — the part that bites — this is an
**order dependency**: `.only`, `--shard`, a bisect, or splitting a 1900-line file turns real
guards red for reasons unrelated to what they guard.

#### `clearMocks` is the wrong instrument

It calls `mockClear()`, which resets call history and **leaves the implementation in place** — and
a leaked `mockReturnValue` *is* an implementation. Verified directly rather than reasoned about:
with the local `beforeEach` removed and `clearMocks: true` set, the filtered run fails
**identically**. Flipping it would look like a fix, cost a red test (`monaco.test.ts`), and change
nothing. `mockReset()` is the one that drops implementations.

#### `mockReset: true` finds TWO different things, and the difference is the whole planning problem

Turning it on globally is what surfaced the leaks above. It also breaks tests that are not broken.
**An earlier revision of this section called all twelve failures "the same defect" and described
the follow-up as "12 leaks, fix them". That was wrong**, and wrong in the way §9 warns about: the
error messages looked alike (`reading 'status'`, `reading 'isPending'`, `reading '0'`), and
matching shapes were read as a matching cause without opening the files. Opened, they are two
populations:

| | files | tests | what it is |
|---|---|---|---|
| **Leaks** — fixed | 1 (`TryIt.test.tsx`) | 4 | tests calling `renderTryIt()` with no `mockTry(...)`, running on an implementation left by an earlier test |
| **By design** — not defects | 4 | 8 | module-scope mocks the file deliberately preserves |

The second group cannot be fixed by adding a mock, because nothing is missing:

- `src/lib/__tests__/monaco.test.ts` asserts `configSpy.mock.calls[0][0]` — a call made at
  **module-import time**, which happens once per process. A per-test reset erases history that
  cannot be recreated; there is no mock to add.
- `src/api/__tests__/dashboards.duplicate.test.tsx`, `dashboards.publishdetail.test.tsx` and
  `components/chart/dashboard/__tests__/DashboardViewActionRows.test.tsx` each hold a module-scope
  `vi.fn().mockResolvedValue(...)` and each `beforeEach`s with `mockClear`/`clearAllMocks` —
  **choosing the flag that preserves implementations**. `mockReset` breaks them by design
  conflict, not by defect.

**So the flag is an AUDIT TOOL, not a setting to switch on.** Used as one it earned its keep
immediately: it found the four real leaks, they were fixed, and re-measuring gives **8 failures
across 4 files, all in the by-design group** — which is the useful conclusion, because it says
**the suite has no remaining detectable implementation leaks**. Adopting it permanently is a
different and larger job: restructuring four files that legitimately depend on module-scope mock
state, or giving them a per-file opt-out. Whoever scopes that should budget for restructuring, not
for adding missing mocks.

**What deferring was and was not justified by.** The earlier revision deferred on the grounds of
not owning the affected files, while naming only four areas — 8 of the 12. **The other four were
in `TryIt.test.tsx`, a file this same work was editing.** Those are fixed here. What is deferred
now is only the part genuinely outside this work's reach, and it is a different kind of work than
the number alone suggested.

### 5.7 Six class assertions are not the claim; the browser spec is

`tests/e2e/data-api-detail-tab-widths.spec.ts` (opt-in:
`EGRET_E2E_DATAAPI_WIDTH=1 npx playwright test data-api-detail-tab-widths`).

§5.1's rule is "the six tabs agree on a width". **jsdom cannot see that claim at all** (§10):
it implements no CSS, every box is 0×0, and `expectSectionLeavesWidthToThePage` compares a
string. Six passing string comparisons would also pass on six tabs that disagreed — one section
could reach a different width through a flex parent, a grid, or a child that overflows, none of
which is a `max-w-*` on the root.

So the rule is measured in Chromium: navigate all six tabs at 1600×1000 and assert the widest
block in each equals the available content width, and equals the other five.

**Two ways the sibling width spec went green on a broken page**, both found live and both
guarded against here rather than merely remembered:

- **A counted DOM hop.** `data-api-response-width.spec.ts` first reached the section root with
  `parentElement.parentElement`. One extra wrapper `div` inside the section makes that resolve
  to the *host* instead — at which point `root === available` is tautologically true and the
  spec passes on a section that has regressed. The fix is the same walk `sectionWidth.ts` uses,
  plus a hard stop if the walk lands on the host.
- **A `getByRole` name that matched by substring.** Playwright matches accessible names by
  case-insensitive substring unless told `exact: true`. `name: "Response"` also matched the
  spec's own fixture, `E2E response width`, and the run died on strict mode before measuring
  anything. Every heading lookup here is `exact: true`, and the fixture is named so that a
  substring match against a TAB name would be caught rather than silently satisfied.

A third, general one: **assert the count before iterating.** A `for` over a selector that has
stopped matching asserts nothing and reports green.

**What has actually been run, stated separately from what the spec claims.** At the time §5.1
was reversed this spec had **not** been executed against a live seeded stack — the backend was
not running in the authoring environment — so it was committed as the regression artifact and
"it passes" was not among the claims made for it. **It has since been run against the deployed
kind stack (2026-08-29) and passes**, and the same walk was repeated by hand on a seeded
endpoint: all six tabs measured `available = root right = widest frame` at 1600, 900 and 700,
in light and in dark, with no page-level horizontal overflow at any of them. The geometry had
also been verified before that in headless
Chromium against the Vite dev server with the API stubbed at the network layer: real browser,
real stylesheet, synthetic JSON. All six tabs measured a right edge of 1560 against an available
right edge of 1560, and — the part that makes it evidence — **a negative control put the three
`max-w-5xl`s back and the 272px split reappeared**: Access, Try it and Versions dropped to 1288
while the other three stayed at 1560. Six numbers agreeing prove nothing on their own; six
numbers that agree *and* stop agreeing when the defect is reinstated are a measurement. §8.

**Those are RIGHT EDGES, not widths, and an earlier revision of this paragraph got that wrong** —
it glossed the 1560 as "the harness has no seeded sidebar, hence 1560 rather than 1296".
Re-measured: the content box is L=264, R=1560, **WIDTH 1296** on all six tabs, which is the
deployed figure exactly, and 1288 is where a `max-w-5xl` root ends (264 + 1024). The harness
reproduces the deployed geometry rather than approximating it. The gloss was the weaker claim
AND the false one, which is the usual shape: a number that looks off invites an explanation, and
inventing one is cheaper than re-measuring. The same arithmetic — every box shares the left edge
264 — is what identified §5.4b's 1576 as wrong.

### 5.8 The fourth browser spec, and the helper that would have made it a ReferenceError

`tests/e2e/data-api-detail-one-grid.spec.ts` (opt-in:
`EGRET_E2E_DATAAPI_ONE_GRID=1 npx playwright test data-api-detail-one-grid`).

It carries what jsdom cannot judge about the detail-tabs pass: that `Versions`' widened `Who`
column actually holds a realistic address (laid out in a detached span with the cell's own
computed font — `scrollWidth` reads the same number whether the text is clipped or not once it
FITS, so it would pass vacuously); that `Note` is still the unsized remainder; that Publish
shares the `History` heading's line (a shared vertical CENTRE, not an equal `top` — the button
is taller than the heading, so `top` reports two lines for one); and that `Response` renders
exactly one `Setting | Details | Value` grid whose transform row spans the whole table.

**Two things it got wrong before it ran, both caught by running its own measure functions
against the real page:**

- **A shared helper inside `page.evaluate`.** `page.evaluate` ships the evaluated function's
  own source to the browser and nothing else, so a module-scope helper called from inside one is
  `undefined` there. The section-root walk is written out in BOTH measure functions on purpose;
  the duplication is the price of a spec that can run at all, and a `ReferenceError` at the first
  assertion reads exactly like a missing element.
- **A blanket `querySelectorAll("table")`.** `ReturnsEditor` renders a `max-w-xl` table INSIDE a
  cell of the settings grid, which §5.5 exempts deliberately — so "every frame ends at the
  section's right edge" has to exclude nested frames, and the spec asserts that there IS exactly
  one nested frame so the exclusion cannot go vacuous.

**It has now been run against a live stack (kind, 2026-08-29), and the first run FAILED on a
third bug of the same family — a `page.route` glob that matched nothing.** It intercepted
`**/api/access/users`, which is the name of the frontend module the call comes from
(`src/api/access.ts`), not the route it issues: `listUsers()` reaches
`/api/control-plane/users`. A glob that matches nothing does not throw. The real request went
through, `Who` fell back to `created_by.slice(0, 8)`, and the run died fifteen seconds later on
"element not found" — indistinguishable from the column regression the file exists to catch, and
invisible to a stubbed harness because the harness answers every request itself.

The fix also asserts that the intercept FIRED — and the spec says exactly what that buys, because
re-breaking the glob as a mutation showed the run still dies at the address-visibility wait, not
at the counter. The counter covers the case that wait cannot see: a stack whose real user list
happens to hold the fixture's address, where the wait passes over a dead intercept and every
width below is measured against the backend's data rather than the fixture's.

Before that run, every number in it was measured in headless Chromium against the Vite dev
server with the API stubbed at the network layer, by running the spec's own `measureVersions`
and `measureResponse` — compiled from the spec's source with esbuild rather than retyped — so
what was verified is the code that ships, not a paraphrase of it.

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

## 11. "Configure, run, read the result" is one screen — a split only sometimes

`src/routes/explore/dataapi/detail/Query.tsx` (the first), `detail/TryIt.tsx` (the second).

Two sections of the endpoint editor have the same shape — set something up, run it, read what
came back — and both put the two halves on screen at once rather than down a page that
scrolls.

**The defect.** Stacked, the result sits below the setup and off-screen: run it, scroll down,
read, scroll back up to change a value, scroll down again. `Query`'s preview was below
`ParamEditor`, and `TryIt`'s result below a `Run as` card. In a browser `Query`'s first
version of the split was worse still — the strip of tables took 370px of a 615px section and
left the SQL editor 107px, about four lines, which no test could see.

The parts, all three load-bearing:

- **The setup is tables** (§4) — a `ConfigTable` row for a setting, a plain `Table` for a list.
  `TryIt`'s service-account picker is one `ConfigTableRow`; its per-parameter trial *values*
  are a plain table, one row each.
- **The result is the lower half, and it owns the run control.** The button and the shortcut
  live in the *result*'s header, not with the setup: `PreviewBlock` and `TryIt` both put `Run`
  there, and the header stays put **while the result's own body scrolls**.

  **That last clause used to say "while the body scrolls", full stop, and for `TryIt` it is now
  too strong.** A `Group`'s panes cannot exceed the section, so under the split `Run` was on
  screen at every parameter count. `TryIt`'s column can overflow the shell's scroll box, and
  then the result's header goes below the fold with it — measured, 5px under at 8 declared
  parameters and ~355px under at 16. The header still stays put *within the result*; what it no
  longer does is stay on the *page*. §11.0 records what was put in its place, and what was tried
  and did not work.
- **The shortcut is named beside the button it fires.** `⌘↵`, in the header. An affordance
  nobody can see is not an affordance. `Query` registers it as a Monaco command (`SqlEditor`'s
  `onRun`); `TryIt` has no editor, so it uses a `window` `keydown` listener, the idiom
  `components/sqllab/SqlTab.tsx` already uses for `⌘K`.

### 11.0 The split is the implementation, not the shape — and it fits only one of the two

**This section used to say "both are now a vertical `Group`", and that is what turned out to be
wrong.** A `react-resizable-panels` `Group` divides a definite height by a FRACTION. That is
right when a half must be *handed* a height and cannot state its own, and wrong when a half's
height is decided by its content.

- **`Query` keeps its split.** Its upper half is a Monaco editor: an auto-height editor is 0px
  tall, so something has to hand it a number. `Query.tsx`'s own comment states the rule that
  follows from this and it is the rule that condemns the other case — *"the strip can be
  expanded at any window height, so any fixed split has a window that defeats it."*
- **`TryIt` dropped its split.** Its three parts are *Run as*, *Parameter values* and *Result*,
  and only the last grows without bound; the first two are sized by their content. Measured in
  Chromium at 1600×1000, the `40%` setup pane's `clientHeight` was **230 whatever it held**,
  against a `scrollHeight` of 277 / 297 / 342 / 432 / 612 for 0 / 1 / 2 / 4 / 8 declared
  parameters. It clipped **47px on an endpoint declaring no parameters at all** and 202px on
  four — slicing the `Parameter | Type | Value` header row through the middle so that not one
  parameter row was on screen — while the result pane below it, 389px, held a header and one
  collapsed caveat. There was no fixture for which the fraction was right.

  It is now a flex column: the setup `shrink-0` at its own height with no scroll box of its
  own, the result `flex-1` with a `min-h-64` floor and the section's only `overflow-auto`. When
  an endpoint declares more parameters than the section is tall, the whole section overflows
  the shell's `overflow-auto` fieldset — which scrolls, and is reachable — instead of a pane
  clipping inside a page that does not move. Measured after: nothing in the section hides
  anything before a run, at 0/1/2/4/8/16 parameters.

  **The split could not be repaired in place, which is why the answer is removal rather than a
  better fraction.** `react-resizable-panels` v4 has no content-sizing mode — `defaultSize`
  takes %, px, em, rem, vh or vw — so "size the top pane to its content" means measuring the
  content and calling `panelRef.resize()` on mount and on every parameter change: machinery
  whose only job is to imitate normal flow.

  **What the removal COST, stated rather than netted out.** The split guaranteed `Run` was on
  screen at every parameter count. The column does not: measured, the result's header starts 5px
  below the fold at 8 declared parameters and ~355px below it at 16. That is not cosmetic,
  because §11.1's shortcut is bound to `window` — ⌘↵ fires wherever the page is scrolled, a REAL
  trial runs (a serving-path call, a rate-limit token, an `access_log` row), and **nothing
  visible changes**, so the reader presses it again.

  **`sticky top-0` on the result's header is the obvious answer and it is measurably inert
  here.** The section is `flex-1` in an `h-full` column, so its height is
  `max(256, available − setup)` — never more than the scroll box — and a box that fits can never
  be scrolled past its own top, which is the only condition under which sticky engages. Built,
  then measured at 8 and 16 parameters across four scroll offsets each: the header's top equalled
  the section's top **every time**. It was removed rather than shipped, because a class assertion
  passing over a no-op is the failure §8 exists for.

  Two facts hold instead, and both were measured rather than argued:

  - **`Run` is always reachable**, because that same "never taller than the box" property means
    scrolling to the bottom always brings the result fully into view.
  - **`onRun` scrolls the result into view when a run starts** (`block: "nearest"`, so it is a
    no-op for every run started by clicking a button that was already visible), placed BEHIND the
    `canRun` guard so a refused run does not yank the page. From `scrollTop: 0`, ⌘↵ left `Run`
    inside the visible box at 6, 8 and 16 parameters — where at rest, 8 and 16 had it outside.

- **Sizes are percent STRINGS, wherever a `Group` is still used.** In `react-resizable-panels`
  v4 a bare number means *pixels*, so `defaultSize={55}` is a 55-pixel editor that reads as
  correct in the source. `Query` is tested for the string *type*, not only the value.
  `TryIt.test.tsx` keeps the library's stub after removing the import, precisely so a
  reintroduced split is observable in jsdom rather than silently passing an assertion that no
  longer has anything to match.

**Naming the halves.** With no `Panel` to point at, `TryIt`'s two halves are
`<section aria-labelledby=…>` — real landmarks with the `h3` each already had as its name.
That is what lets a test say "Run is in the *result*" by identity instead of by
`getAllByTestId("panel")[1]`, which is a counted hop wearing a different hat.

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

What did not move is as informative. `TryIt`'s two-line published-version note stays plain
text, and `Access`'s transient "Checking workspace-inherited invoke…" with it, on the same rule
pointing the other way: **there is no second part to put behind a summary**, so a disclosure
would cost a click to reveal nothing.

**A previous revision of this paragraph also claimed `Access`'s CONDITIONAL STATE paragraphs
stay plain text. They do not** — "Revoking may not remove access" and "Nothing here to revoke"
are both `Caveat`s in `AccessSection.tsx`, and have been since §4.4's component landed. The
claim was wrong in the same way twice (see also §7.2's inventory), and wrong in a way this
document is especially prone to: it described the section's REASONING, which is correct, and
then named files as if it had read them. §4.4 now says which test governs so that the two
sections cannot drift again — kind, not size — and under it the two `Caveat`s are right and
this sentence was the error.

### 11.3 What a test can say about any of this

jsdom does no layout (§10), so the drag, the heights and the widths are a browser check. What
is testable is the wiring, and it is where the regressions were: which half each thing is in,
that the sizes are strings wherever a `Group` survives, that the shortcut fires the mutation
with the on-screen values, and that it fires nothing when a guard in §11.1 should stop it.

**Name the half, do not index it.** `Query` still has panels and `within(panels[0])` /
`within(panels[1])` reads them; `TryIt` does not, and its halves are resolved by accessible
name (`getByRole("region", { name: "Result" })`). Prefer the name even where the index works:
an index is a counted hop, and §5.6/§5.7 record twice that a counted hop re-points silently
onto something that satisfies the assertion for the wrong reason.

**`Query.test.tsx` still indexes, and that is recorded rather than quietly left.** Its panels
carry no accessible name — `Query`'s halves are bare `<div>`s inside `<Panel>`, not
`<section aria-labelledby>` — so `getAllByTestId("panel")[1]` is the only handle there is today.
Naming them the way `TryIt` names its halves is the fix and it is **owed**, not done: it is a
change to `Query.tsx`, and the task that rewrote this section owned `TryIt.tsx`. Until then the
index is a known counted hop in a file this section tells you not to write one in.

**And a class chain is the strongest thing jsdom can say about a height.** `TryIt`'s clipping
was 47–382px of hidden content, and no jsdom assertion can see one pixel of it. What its tests
pin instead is the mechanism: walk from the parameter table up to the section root and assert
that no ancestor scrolls vertically or caps its height. Two cautions came out of writing it —
the regex must exclude `overflow-x-*`, because `ui/table.tsx` wraps every `<Table>` in
`overflow-x-auto` by design (§5.3) and catching it would fail a section with nothing wrong;
and the walk must end at the root by IDENTITY, because a chain that silently runs past it
asserts about the shell.

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

- `ListPageShell` — **10** route files: 6 Data API (`Endpoints`, `Clients`, `Activity`,
  `EndpointCreate`, `ServiceAccountCreate`, `ClientCreate`) and 4 Explore list pages
  (`Datasets`, `Charts`, `Dashboards`, `Alerts`). Nine were converted by package U on
  2026-08-22; `Activity.tsx` followed on 2026-08-29. Measured with
  `grep -rl 'from "@/components/explore/ListPageShell"' src --include='*.tsx' | grep -v __tests__ | wc -l`
  from `frontend/`. **`Notebooks.tsx` still hand-writes its header** (735 lines, `aria-pressed`
  = 0). It was named in neither §1.3's list of four nor package U's scope, and is recorded
  here so this section is not read as "every Explore list page is converted".

  **`Activity.tsx` was the last hand-written copy, and the reason it survived is worth more
  than the conversion.** It carried a comment saying the page "owns its own heading and padding
  again", which reads as a decision to decline the shell and was defended as one. It is not:
  the comment was written in #511 on 2026-08-17, one day *before* `ListPageShell` existed
  (#518, 2026-08-18), so it cannot have weighed a component that did not exist. What it
  actually decided was that this route has a heading **at all**, rather than inheriting one
  from the `DataApiLayout` shell that had just been deleted — a question about ownership, not
  about spelling, and the shell is how that ownership is discharged. `dataapi/Clients.tsx`
  carries the same paragraph and already ends it "they just come from the shared shell now".
  The comment also called itself "same as `Charts.tsx`/`Datasets.tsx`" while both of those had
  moved to the shell, so its own claim of parity had gone stale without anything editing it.

  **A comment that records a decision still has to be re-read against the date it was written.**
  Neither "a review called it drift" nor "a comment calls it deliberate" settles anything; the
  commit dates and the sibling that resolved the identical paragraph did.

  The 1h/24h/7d/30d strip stayed in the page body rather than moving to the shell's `toolbar`
  slot. §1.2 classes it a view **switch**, and the consequence of that classification is that
  it is never gated on `hasData` — a rule about *when* it renders, not *where*. It is also not
  a toolbar control in shape: `RangePicker` is a button strip over a `<details>` retention
  caveat, two rows, and moving that into the header's `flex flex-wrap items-center` group is a
  layout change no jsdom test can check (§10).
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
- The `Caveat` disclosure of §4.4 — **5** files, **11** call sites (`AccessSection` 4,
  `TryIt` 2, `Query` 2, `Activity` 2, `PreviewBlock` 1). It was 1 file and 3 call sites for one
  phase, all in `AccessSection`, where it was written for a row-scoped explanation big enough to
  set the row height; the Data API UI audit found the same shape at section scale and it moved
  out of that file into `detail/Caveat.tsx`.

  **This was recorded as 4 files / 9 sites, and the counting method is why.** The previous
  revision counted `from "./Caveat"` — an import SPECIFIER — and `Activity.tsx` writes
  `from "./detail/Caveat"` because it sits one directory up. A specifier search structurally
  cannot see a consumer outside the component's own directory, which is precisely the consumer
  a "does this belong in `components/`?" question is asking about. **Count by resolved module,
  not by import string.** For this file that is:

  ```bash
  grep -rn 'Caveat"' frontend/src --include='*.tsx'   # every specifier that ENDS in Caveat
  grep -rn '<Caveat' frontend/src --include='*.tsx'   # and the call sites, separately
  ```

  The second command is separate on purpose: a file count and a call-site count are different
  questions, and collapsing them is the symbol-family error recorded two bullets up.

  **Does the "deliberately not in `components/`" ruling survive the correction?** Yes, and for a
  reason the wrong count was hiding rather than supporting. The fifth file is `Activity.tsx`,
  which is still Data API — a sibling route, not another feature — so the spread is 5 files
  inside one feature, not 4 inside one directory. The test for promotion is whether the rule has
  been measured somewhere else, and it has not. Had the fifth consumer been outside Data API,
  the specifier count would have hidden exactly the evidence that should have moved it.

  What has NOT adopted it is the rule working rather than an inconsistency — `Access`'s short
  transient states and `TryIt`'s two-line published-version note are plain text, because there
  is no second part to put behind a summary. (This bullet used to name `Access`'s conditional
  state paragraphs here too; they are `Caveat`s, and are two of the four counted above. See
  §11.2.)
- The shape of §11 — **2** sections, both Data API endpoint detail (`Query`, `TryIt`).
  **Only ONE of them is a `react-resizable-panels` split**: `Query`, whose upper half is a
  Monaco editor that must be handed a height. `TryIt` is a flex column (§11.0) — its parts are
  content-sized, and the fraction clipped them at every parameter count measured. The library
  is also used by `components/sqllab/SqlTab.tsx`, which is where the idiom came from.
  `Versions` and `Access` are neither: they are not "configure, run, read the result" — there
  is nothing to run.
- The section-width guard of §5.6 — **6** sections (`Query`, `Config`, `Response`, `Access`,
  `Try it`, `Versions`), one helper, **one form**. It was 3 copies and 2 absences before the
  width phase, then 5 call sites across two forms; §5.1's reversal to one width added `Query`
  — the last unguarded section — and deleted `expectSectionCapsItsOwnWidth`, which the reversal
  left with zero callers. **Sections whose ROOT carries a `max-w-*`: 0.** Sections capping a
  CHILD: 1, `ResponseSection`'s `returns` grid (§5.5), and
  and `grep -rn 'className="[^"]*max-w-' src/routes/explore/dataapi/detail` is what makes that a
  count and not a recollection (§9). **Narrowed to `className=` on purpose:** the bare
  `grep -rn 'max-w-'` this bullet used to quote also returns the `max-w-xl` line itself plus nine
  COMMENT lines that discuss caps without applying any, so re-running it reads as drift that is
  not there — a check nobody trusts gets skipped.
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
