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

- **`components/explore/BulkBar.tsx` is imported by nothing at all**, while all four pages that have bulk-select reimplement its toggle and action bar inline. **`components/explore/EmptyState.tsx` is used by eight control-plane pages but by none of the six Explore list pages**, which each hand-roll the same dashed-border block instead. Copying either pattern into Data API would be another copy of something that already exists as a component.

  *(Corrected. An earlier version of this line said both were "imported by nothing". That was true of `BulkBar` and false of `EmptyState` — `grep` finds it in `ControlPlaneEngines`, `Warehouses`, `Workspaces`, `ControlPlaneStorage`, `ControlPlaneSecrets`, `UsersPage`, `RolesPage` and `GroupsPage`. The error came from generalising a survey that had only looked at the six Explore pages. It matters because it changes the blast radius: `EmptyState`'s existing props and defaults are load-bearing for eight live pages, so the variant work must leave them untouched rather than merely unbroken-looking.)*
- **Bookmark logic is copy-pasted four times** with no shared component at all.

So the work is not "make Data API look like Datasets". It is "extract what the four pages already share, prove it on the pages that have none of it, and write it down".

### What is not frontend-only

The request was for frontend work. Three of the seven capabilities cannot be delivered that way, and this is stated up front rather than discovered mid-implementation:

| Capability | Backend work | Migration? |
|---|---|---|
| Bulk select, New pages, detail layout, facets, row menu | none | no |
| Bookmark | two routes, `POST`/`DELETE /ws/{workspace}/data-api/endpoints/{id}/bookmark` | **no** — `bookmarks` is generic (`entity_type text`, `entity_id uuid`) |
| Move to folder | **none — already implemented.** `UpdateEndpointInput.folder_id` is `Option<Option<Uuid>>` (`endpoints.rs:254`), the store honours it (`SET folder_id = CASE WHEN $14 …`), and `routes.rs:912` validates workspace containment via `scoped_folder`. Only the UI is missing. | no |
| Trash | `deleted_at`, store, routes, **and the serving path must exclude trashed endpoints** | **yes** |

Trash is the one with a dangerous edge, and §6 is about that edge rather than about the UI.

*(Corrected. An earlier version of this table said moving to a folder needed a `folder_id` field added to `UpdateEndpointInput`. It is already there, complete with containment validation — the claim came from grepping `routes.rs`, where the struct does not live, and reading the empty result as proof of absence. That is the same mistake this spec already had to correct once about `EmptyState`. **Absence of a grep hit is not absence of the feature; grep the file that would define it, or ask the compiler.**)*

## 2. Shared primitives, extracted first

Data API is the first consumer because it has none of these today: adopting them there cannot regress a working page. The four existing pages migrate one at a time in later PRs, which is deliberately out of this spec's scope.

Each primitive below replaces something that exists in 4-6 near-identical copies. None is a new idea; every one is a name for a shape the codebase already repeats.

| Primitive | Replaces | Notes |
|---|---|---|
| `ListPageShell` | the `p-4` + `flex items-center justify-between` header, hand-written in all six pages | Takes `title`, optional `description`, a toolbar slot, and a primary-action slot. |
| `BookmarkToggle` | four copies of the star button and its `onlyBookmarked` state | Takes the add/remove mutations; owns nothing else. |
| `BulkBar` + `useBulkSelect` | four inline reimplementations | **The component already exists and is unused** — adopt it rather than writing a fifth. |
| `EmptyState` | six copies of the dashed-border block | **Already exists and is used by eight control-plane pages** — its current props and defaults must survive unchanged. Must cover three distinct states, not two: empty, filtered-to-empty, and **error**. |
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

**Corrected after measuring.** This section originally said to follow `charts/new` and `alerts/new`, which render **the same component as the detail route** with no id in the URL yet. That is wrong for an endpoint, and the measurement is simple: **six of the seven detail components read `id`** — `PreviewBlock` (preview POSTs to `…/endpoints/{id}/preview`), `ResponseSection`, `AccessSection`, `TryIt`, `Versions`, and `Config`. Only `Query` itself does not. Reusing the detail shell would produce a page whose Preview cannot run and whose Response, Access, Try it and Versions tabs are all meaningless until the first save.

That precedent works for a chart or a schedule because each is a single form with no id-dependent sub-resources. An endpoint has versions, grants and trials, all of which need the row to exist.

The right precedent is already in the repo and is a third shape, not an invention: **`engines/new` → `EngineCreate`**, a dedicated create page that collects what a row cannot exist without, creates it, and navigates to the detail route. That is also what the current dialog does — the change the user asked for is that it stops being a dialog, not that it becomes the whole editor.

The route registers the literal `new` **before** `:id` as a convention for readers, **not as a fix**. An earlier version of this line said the literal would otherwise be captured as an id. That was measured false: react-router-dom **7.18.2** ranks routes by segment specificity, not declaration order, so with `:id` declared first `/endpoints/new` still matches the static route — verified twice with throwaway `matchRoutes` specs, once while implementing and once in the final review, after the same false claim was found in two pre-existing `App.tsx` comments and corrected there.

*(This line is worth reading as a lesson about this document rather than about the router. The commit that corrected §4 fixed the claim in the code comments and left it standing here, in the same branch — which is exactly the "two contradictory rationales are worse than either alone" failure the correction was about. The final whole-branch review caught it. Correcting a belief means correcting every copy of it, and the copies are rarely all in one file.)*

**`App.tsx:124-127` currently carries a comment arguing against this**, on the grounds that the create dialog collects only what a row cannot exist without and hands off to the real editor immediately. That reasoning is superseded, and the comment must be replaced rather than left standing beside the opposite decision — two contradictory rationales in one file is worse than either one alone.

The consequence to design for is therefore smaller than it first appeared: the create page owns its own small form and never mounts the detail shell, so `key={id}` is not in play. What it must get right is the hand-off — creating the row and navigating to `…/endpoints/{id}` without losing what the author typed, and without leaving a half-created row if the navigation fails.

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
- `ConfigSection` is retired **from the Data API detail route only** — two files there. **Measured by import, not by substring — and re-measured at `HEAD` for this line: 13 files import `ConfigSection`, and 11 production files import `ConfigTable`, outside this route** (`ConfigTable`'s full import count is 14: 13 production files plus its own test). Changing them is not this spec's business, and any change to `ConfigTable` itself must be purely additive.

  *(Twice corrected, and the second correction is the instructive one. The first version guessed "nine Control Plane usages". The second replaced it with 16 and 15 from `grep -rl "ConfigSection"` — a **substring** search, which also matches `StarRocksConfigTab.tsx`'s own local `FeConfigSection` and `CnConfigSection`. So the fix for an unmeasured claim was itself mismeasured, in a way that looked like diligence because it cited a command. The numbers above come from grepping the **import path**. When a count matters, grep what would actually create the dependency.)*
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
