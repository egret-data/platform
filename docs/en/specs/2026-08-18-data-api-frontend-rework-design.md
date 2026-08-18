# Data API frontend rework, and egret's list-page conventions

Two deliverables that are one piece of work: bring the Data API pages up to what the rest of Explore already does, and — because doing that by copy-paste would be the fifth copy — extract the shared parts first and write down the result as the standard for every Explore page.

## 1. Why

The Data API pages were built page-by-page against their own requirements, and the rest of Explore was built the same way earlier. The result is measurable rather than a matter of taste, so it was measured before this spec was written.

| Capability | Datasets / Charts / Dashboards / Alerts | Endpoints | Clients |
|---|---|---|---|
| Bookmark, and filter to bookmarked | all four | **no** | **no** |
| Bulk select + bulk actions | all four | **no** | **no** |
| Trash / restore / delete permanently | all four | **no**, hard delete only | **no** |
| Folders: create, rename, move, drag-and-drop | all four | **groups by folder but cannot set one** | **no** |
| Row actions menu | dedicated menu per domain | inline, missing move/share/duplicate | **no menu; buttons loose in the row** |
| Facet filters | three of four | search only | search only |
| Share / Tags | three of four | **no** | **no** |
| Summary strip | all four | yes | yes |
| Pagination | none | none | **yes — the only page with it** |

Two findings from that survey change the approach:

- **`components/explore/BulkBar.tsx` and `components/explore/EmptyState.tsx` already exist and are imported by nothing.** All four pages that have bulk-select reimplement the toggle and the action bar inline, and all six pages hand-roll the same dashed-border empty state. Copying either pattern into Data API would be the fifth and sixth copy of something that already exists as a component.
- **Bookmark logic is copy-pasted four times** with no shared component at all.

So the work is not "make Data API look like Datasets". It is "extract what the four pages already share, prove it on the pages that have none of it, and write it down".

### What is not frontend-only

The request was for frontend work. Three of the seven capabilities cannot be delivered that way, and this is stated up front rather than discovered mid-implementation:

| Capability | Backend work | Migration? |
|---|---|---|
| Bulk select, New pages, detail layout, facets, row menu | none | no |
| Bookmark | two routes, `POST`/`DELETE /ws/{workspace}/data-api/endpoints/{id}/bookmark` | **no** — `bookmarks` is generic (`entity_type text`, `entity_id uuid`) |
| Move to folder | `folder_id` on `UpdateEndpointInput` and the store | **no** — `dataapi.endpoints.folder_id` already exists |
| Trash | `deleted_at`, store, routes, **and the serving path must exclude trashed endpoints** | **yes** |

Trash is the one with a dangerous edge, and §6 is about that edge rather than about the UI.

## 2. Shared primitives, extracted first

Data API is the first consumer because it has none of these today: adopting them there cannot regress a working page. The four existing pages migrate one at a time in later PRs, which is deliberately out of this spec's scope.

Each primitive below replaces something that exists in 4-6 near-identical copies. None is a new idea; every one is a name for a shape the codebase already repeats.

| Primitive | Replaces | Notes |
|---|---|---|
| `ListPageShell` | the `p-4` + `flex items-center justify-between` header, hand-written in all six pages | Takes `title`, optional `description`, a toolbar slot, and a primary-action slot. |
| `BookmarkToggle` | four copies of the star button and its `onlyBookmarked` state | Takes the add/remove mutations; owns nothing else. |
| `BulkBar` + `useBulkSelect` | four inline reimplementations | **The component already exists and is unused** — adopt it rather than writing a fifth. |
| `EmptyState` | six copies of the dashed-border block | **Already exists and is unused.** Must cover three distinct states, not two: empty, filtered-to-empty, and **error**. |
| `TrashToggle` | four copies of the trash/restore/permanent-delete triad | |
| `FolderToolbar` | four copies of new-folder + move-to-folder + drag-and-drop | |

**`EmptyState` must keep error separate from empty.** In the previous phase, two account pickers folded `isError` into "empty" and told an operator to create a service account that already existed — a dead end with a false explanation. A shared component that permits that collapse would spread the defect rather than contain it.

**No primitive owns data fetching.** Each takes state and callbacks. A primitive that fetched would make every page's query key its business, and the P5b defect where six mutations toasted success over a stale list came from exactly that kind of coupling being implicit rather than explicit.

## 3. The two list pages

`Endpoints` and `Clients` adopt every primitive above, plus:

- **Facet filters.** Endpoints gains `state` (live / draft / disabled) and `engine`. Today the state filter exists but is reachable only by clicking a `SummaryStrip` chip, which is not discoverable as a filter.
- **A row actions menu.** Both pages get a dedicated `*ActionsMenu` component like the other four domains have, rather than buttons loose in the row. Clients has no menu at all today.
- **Search in the page toolbar, not inside the table.** `Clients` currently relies on `FilterableTable`'s internal search box, so its search sits in a different place from every other Explore page.

**Certification is not added.** Endpoints and clients have no `certified` field, and inventing one to match Datasets would be a data-model change wearing a UI change's clothes.

## 4. Three dedicated New pages

`endpoints/new`, `clients/new`, and the client-credential flow become routes rather than dialogs.

The precedent is `charts/new` and `alerts/new`: both register the literal `new` **before** `:id` (or the literal is captured as an id) and both render **the same component as the detail route**, with no id in the URL yet. Data API follows that exactly rather than inventing a third shape.

**`App.tsx:124-127` currently carries a comment arguing against this**, on the grounds that the create dialog collects only what a row cannot exist without and hands off to the real editor immediately. That reasoning is superseded, and the comment must be replaced rather than left standing beside the opposite decision — two contradictory rationales in one file is worse than either one alone.

The consequence to design for: a `/new` route has **no id**, so anything keyed on one must tolerate its absence. The detail shell currently remounts on `id` change via `key={id}`; with no id there is nothing to key on, and the first save must navigate to the created id without losing what the author typed.

## 5. Detail content: one table per tab

**Cards are the problem, not the table.** The current detail sections stack several `ConfigSection` cards, each framing a table of one or two rows. Read down the page, that is a column of mostly-empty boxes.

Each tab becomes **one full-width table**. Groups (`Endpoint`, `Cache`, `Limits`, `Rate limit`) become **header rows spanning the table**, not separate frames:

```text
Setting          Details                              Value
-----------------------------------------------------------------
ENDPOINT
Name             How this endpoint is listed          [ … ]
Path             The URL consumers call               [ … ]
-----------------------------------------------------------------
CACHE
Enabled          Serve repeat calls from a cache      ( o)
```

- **`max-w-3xl` on the detail `Outlet` is removed.** It is the direct cause of the wasted horizontal space: the `Details` column wraps to three lines while the right third of the page is empty.
- **The `Details` column gets its own max width**, so explanatory prose does not become one unreadable line at 2560px while the `Value` column takes the slack it needs for long inputs (SQL, path).
- `ConfigSection` is retired **from the Data API detail route only**. Nine Control Plane usages keep it; changing those is not this spec's business.
- **`ConfigTableRow.details` remains a plain string with no markdown.** Backticks written into it render literally — a real defect found in a browser in the previous phase. The wider table makes longer copy tempting; the constraint does not change.

The `Query` tab is the exception and keeps its current form: a SQL editor and a result grid are not settings, and forcing them into a `Setting | Details | Value` row would be the same mistake as the cards, inverted.

## 6. Trash, and the edge that matters

Schema, store, and routes are straightforward. One rule is not:

> **A trashed endpoint must stop serving.** `serving/router.rs` resolves an endpoint by path and method and joins `endpoint_versions` on `live_version_id`. If that query does not exclude trashed rows, a soft-deleted endpoint keeps answering consumer traffic — and the deletion is invisible to the person who performed it.

This is not hypothetical. The Alerts programme shipped exactly this defect: a soft-deleted schedule kept firing until `deleted_at IS NULL` was added to the claim query. The same shape, one subsystem over.

**Trash accepts only `draft` and `disabled` endpoints.** A `live` endpoint must be disabled first. Two reasons:

1. It keeps the state honest. A row that says `live` and serves nothing, or says `trashed` and serves traffic, is a lie either way.
2. It avoids a **second** path to "stop serving" running alongside `disable`. Two mechanisms that stop traffic are two places to forget one, and the forgotten one is always the newer.

The refusal must be a real refusal at the API, not a disabled button: a button the UI hides is not a rule the server enforces.

Restore returns an endpoint to `disabled`, never directly to `live`. Coming back from the trash is not a publish, and re-serving traffic should be a deliberate second action.

## 7. The conventions document

`docs/en/ui-conventions.md`, with a Vietnamese mirror, written **from** the primitives in §2 rather than ahead of them — a convention nobody has implemented is a wish.

It covers: the list-page anatomy and the order of its toolbar controls; when a create flow is a page and when a dialog is still right; the `Setting | Details | Value` table and when a card is warranted; the three states every list must distinguish (loading, empty, error) and why error may never fold into empty; and the width rules.

Each rule cites the code that implements it and, where one exists, the defect that motivated it. A rule with neither is an opinion, and opinions belong in review comments rather than in a standard.

## 8. Testing

- **Every primitive gets its own tests**, not only tests of a page that uses it. A primitive tested solely through one consumer is one refactor away from being untested.
- **`EmptyState` must have a test that error does not render as empty**, mutation-checked by folding the two together and watching it fail.
- **Trash: a test that a trashed endpoint stops serving**, driven through the real serving router, not through the store. Mutation-check it by removing the `deleted_at` predicate — that mutation must fail the test, and if it does not, the test is checking the wrong layer.
- **Trash: a test that the API refuses to trash a `live` endpoint**, at the route rather than in the component.
- **A `/new` route test that mounts the route tree the application actually has.** In the previous phase a test mounted `/explore/:ws/data-api`, a tree the app did not have, and stayed green over dead code.
- Assertions are mutation-checked before being believed. A mutation that does not compile proves nothing — build first, confirm zero errors, then read the result. And prefer asserting the specific outcome over `is_err()`: a recent mutation removed a whole validation scan and the input was still refused, by a different rule, with a different error.

## 9. Decomposition

Six phases. Each is independently shippable and reviewable; a later one can be dropped without unpicking an earlier one.

1. **Primitives** — the six components in §2, with their own tests. No page changes.
2. **Data API list pages** — adopt the primitives; facets; row menus. Frontend only.
3. **Bookmark + folder move** — two routes, one input field; then wire the UI.
4. **New pages** — three routes; retire the dialogs; correct the `App.tsx` comment.
5. **Detail layout** — one table per tab; remove the width cap; retire `ConfigSection` here.
6. **Trash** — migration, store, routes, the serving exclusion, and the UI last.

The conventions document is written at the end of phase 5 and updated by phase 6, so it describes what exists.

**Phases 1 and 2 must land close together.** A primitive with no consumer is unreviewable in practice: nothing shows whether its interface is right.

## 10. Deferred, with the reason

- **Migrating Datasets / Charts / Dashboards / Alerts onto the primitives.** The point of doing Data API first is that it cannot regress anything; migrating four working pages is a separate risk with a separate review.
- **Certification for endpoints and clients** — a data-model change, not a UI one.
- **Sharing and Tags for Data API objects.** An endpoint's access model is Cedar plus per-endpoint invoke grants, and `ShareDialog` speaks the analytics object model. Making them meet is its own design.
- **Pagination for the other five list pages.** `Clients` has it because `FilterableTable` offers it; the rest render every row. Worth doing, unrelated to this.
- **Retiring `ConfigSection` in Control Plane.** Nine usages, none of them this work's concern.
