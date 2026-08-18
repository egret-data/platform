# Data API Bookmarks + Folders Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the Data API `Endpoints` page the bookmark and folder capabilities the other four Explore pages have, and fix the shared grouping bug that makes rows disappear on all six.

**Architecture:** One shared-code fix first (`groupByFolder` stops dropping orphan rows), then the bookmark backend (two routes reusing the generic `bookmarks` table, plus a `bookmarked` flag on the list query), then two primitives — `BookmarkToggle` and `FolderToolbar` — wired into `Endpoints`.

**Tech Stack:** Rust — axum 0.8, sqlx 0.9, utoipa. Frontend — React 18, TypeScript, TanStack Query via orval-generated hooks, shadcn/Radix, vitest + Testing Library.

**Spec:** `docs/en/specs/2026-08-18-data-api-frontend-rework-design.md` — this is **phase 3**.

**Scope correction, already applied to the spec.** The spec's table said moving an endpoint to a folder needed a `folder_id` added to `UpdateEndpointInput`. It is already there (`endpoints.rs:254`, `Option<Option<Uuid>>`), the store honours it, and `routes.rs:912` validates workspace containment through `scoped_folder`. **Folder move is frontend-only.** Only bookmarks need backend work.

## Global Constraints

- **`groupByFolder` has six consumers** (`Endpoints`, `Charts`, `Datasets`, `Dashboards`, `Alerts`, `Notebooks`). Measure before changing: `grep -rl "groupByFolder" frontend/src`. Absence of a grep hit is not absence of a consumer — grep the file that would define it.
- **Bookmarks reuse the generic `bookmarks` table** (`entity_type text`, `entity_id uuid`). **No migration.** Follow `crates/egret-bi/src/dataset.rs:230,242` for the two statements and `crates/egret-bi/src/routes.rs:956+` for the route shape.
- **The bookmark routes are gated on `data_api_endpoint:read`**, not `:manage`. Bookmarking is a per-user view preference, not an authoring action, and every sibling gates it on the read permission.
- **A bookmark is per user.** `user_id` comes from `ctx.principal`, never from the request.
- **No primitive owns data fetching.** `BookmarkToggle` and `FolderToolbar` take state and callbacks.
- **The eslint baseline is exactly 0 errors / 178 warnings.** A new warning is a regression.
- **No msw.** Tests `vi.mock` the API module and use `vi.mocked(hook).mockReturnValue(...)`.
- **CSRF is free through the generated client and only through it** — never hand-roll a `fetch`.
- **`openapi.json` at the repo root is a committed artifact with a byte-identity gate in a different crate** (`egret-server`'s `committed_openapi_json_is_current`). Any utoipa change must regenerate it.
- **Backend tests here are DB-gated:** a missing `DATABASE_URL` makes them report `ok` while asserting nothing, distinguishable only by duration. Run against `egret_test` and read the duration. Never point a test at the dev `egret` database.
- **Do not touch `Charts.tsx`, `Datasets.tsx`, `Dashboards.tsx`, `Alerts.tsx`, `Notebooks.tsx`.** Task 1 changes shared code they all consume, which is exactly why it ships with its own tests and nothing else.
- **Every assertion is mutation-checked before being believed, and a mutation that fails to compile proves nothing** — build to zero errors first, confirm that, then read the result. If a prescribed mutation will not compile, re-cut it into one that does and is equivalent in force, and say so.
- **Run the FULL frontend suite before every commit that touches shared code.**

---

## File Structure

| File | Responsibility |
|---|---|
| `frontend/src/components/explore/folders.ts` | **Modify.** `groupByFolder` stops dropping rows whose `folder_id` names an absent folder. Six consumers. |
| `crates/egret-dataapi/src/endpoints.rs` | **Modify.** `add_bookmark` / `remove_bookmark`, and a `bookmarked` flag on the list query. |
| `crates/egret-dataapi/src/routes.rs` | **Modify.** `POST`/`DELETE …/endpoints/{id}/bookmark`, their `#[utoipa::path]`, router lines and `paths(...)` registration. |
| `crates/egret-dataapi-api/src/lib.rs` | **Modify.** `bookmarked: bool` on the endpoint list DTO. |
| `crates/egret-dataapi/tests/bookmarks.rs` | **New.** Route-level tests. |
| `openapi.json` | Regenerated. |
| `frontend/src/components/explore/BookmarkToggle.tsx` | **New.** The star button plus its filter, replacing four copies. |
| `frontend/src/components/explore/FolderToolbar.tsx` | **New.** New-folder button and the folder dialog wiring. |
| `frontend/src/routes/explore/dataapi/Endpoints.tsx` | **Modify.** Wires both, plus move-to-folder in the row menu. |

---

### Task 1: `groupByFolder` stops dropping orphan rows

**Files:**
- Modify: `frontend/src/components/explore/folders.ts:9-32`
- Test: `frontend/src/components/explore/__tests__/folders.test.ts`

**Interfaces:**
- Consumes: nothing new.
- Produces: `groupByFolder` keeps its exact signature. Only its behaviour for orphan rows changes.

**The bug.** Rows are bucketed into `byId` keyed by `folder_id`, but the output loop emits only folders present in the `folders` argument. A row whose `folder_id` names a folder that is not in the list — deleted, not yet loaded, or belonging to another entity type — sits in `byId` under a key nothing reads and **disappears from the page entirely**. It is not merely unselectable; it cannot be seen, opened, or recovered from that list.

This became visible when `Endpoints` gained bulk mode: "Select all" could sweep in rows nobody could see, so "Delete selected" destroyed them. That page now narrows its bulk set defensively, but the row is still invisible there and on five other pages.

- [ ] **Step 1: Write the failing test**

Append to `frontend/src/components/explore/__tests__/folders.test.ts`:

```ts
it("keeps a row whose folder is missing, instead of dropping it", () => {
  // A folder can vanish from `folders` while a row still points at it: it was
  // deleted, or the folders query has not resolved, or it belongs to another
  // entity_type. Losing the row is the worst available outcome — it cannot be
  // seen, opened, or moved out, and nothing on screen says it exists.
  const rows = [{ id: "a", folder_id: "gone" }, { id: "b", folder_id: null }];
  const groups = groupByFolder(rows, [], false);
  const all = groups.flatMap((g) => g.rows.map((r) => r.id));
  expect(all).toContain("a");
  expect(all).toContain("b");
});

it("puts an orphan row in Uncategorized rather than inventing a folder for it", () => {
  const rows = [{ id: "a", folder_id: "gone" }];
  const groups = groupByFolder(rows, [], false);
  // One group, the null-folder one — not a synthesised folder named after an id
  // the user has never seen.
  expect(groups).toHaveLength(1);
  expect(groups[0].folder).toBeNull();
  expect(groups[0].rows.map((r) => r.id)).toEqual(["a"]);
});

it("still hides empty named folders when hideEmpty is set", () => {
  // The orphan fix must not resurrect the empty-folder behaviour hideEmpty
  // exists to suppress during a search.
  const rows = [{ id: "a", folder_id: "f1" }];
  const groups = groupByFolder(rows, [{ id: "f1", name: "One" }, { id: "f2", name: "Two" }] as never, true);
  expect(groups.map((g) => g.folder?.name ?? null)).toEqual(["One"]);
});
```

Match the file's existing import and `Folder` fixture shape — read it first rather than assuming.

- [ ] **Step 2: Run it to verify it fails**

Run: `cd frontend && npx vitest run src/components/explore/__tests__/folders.test.ts`
Expected: FAIL — the first two tests, because row `a` is dropped.

- [ ] **Step 3: Fix `groupByFolder`**

After the named-folder loop and before the Uncategorized push, fold anything left in `byId` into `uncat`:

```ts
  const sorted = [...folders].sort((a, b) => a.name.localeCompare(b.name));
  const out: FolderGroup<T>[] = [];
  const claimed = new Set<string>();
  for (const f of sorted) {
    const fr = byId.get(f.id) ?? [];
    claimed.add(f.id);
    if (hideEmpty && fr.length === 0) continue;
    out.push({ folder: f, rows: fr });
  }
  // Any bucket no folder claimed holds rows whose `folder_id` names a folder
  // that is not in `folders` — deleted, still loading, or another entity type.
  // They used to vanish: bucketed under a key the output loop never read, so the
  // row could not be seen, opened, or moved out of the folder that no longer
  // exists. Uncategorized is where a row with no *usable* folder belongs.
  for (const [id, rows] of byId) {
    if (!claimed.has(id)) uncat.push(...rows);
  }
  if (uncat.length > 0 || !hideEmpty) out.push({ folder: null, rows: uncat });
  return out;
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd frontend && npx vitest run src/components/explore/__tests__/folders.test.ts`
Expected: PASS.

- [ ] **Step 5: Mutation-check**

Delete the orphan-folding loop. Run `npm run typecheck` and confirm **zero errors**, then run the file: the first two tests must FAIL. Restore.

- [ ] **Step 6: Run the FULL suite — six consumers**

Run: `cd frontend && npx vitest run && npm run typecheck && npm run lint`
Expected: all pass; 0 errors / 178 warnings. This is shared code; a narrowly-scoped run is exactly how a regression reached `main` earlier in this project.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/components/explore/folders.ts frontend/src/components/explore/__tests__/folders.test.ts
git commit -m "fix(explore): stop dropping rows whose folder is missing"
```

---

### Task 2: Bookmark routes

**Files:**
- Modify: `crates/egret-dataapi/src/endpoints.rs`
- Modify: `crates/egret-dataapi/src/routes.rs`
- Modify: `crates/egret-dataapi-api/src/lib.rs`
- Create: `crates/egret-dataapi/tests/bookmarks.rs`
- Modify: `openapi.json`

**Interfaces:**
- Consumes: the generic `analytics.bookmarks` table; `egret_core::authz::require`; `EP_READ` (`routes.rs:105`).
- Produces: `POST`/`DELETE /api/ws/{workspace}/data-api/endpoints/{id}/bookmark`, generated hooks `useDataApiBookmarkEndpoint` / `useDataApiUnbookmarkEndpoint` (orval names them from the handler fn names — confirm after regenerating), and `bookmarked: bool` on the endpoint list DTO.

- [ ] **Step 1: Read the sibling implementation**

Read `crates/egret-bi/src/dataset.rs:225-250` (the two statements) and `crates/egret-bi/src/routes.rs:954-1010` (the two handlers). Follow both. `ON CONFLICT DO NOTHING` on the insert is what makes bookmarking idempotent; keep it.

- [ ] **Step 2: Write the failing tests**

Create `crates/egret-dataapi/tests/bookmarks.rs`, following `crates/egret-dataapi/tests/endpoints_routes.rs` for the fixture idiom (that file's header explains the DB gate and the self-skip).

Four tests:

1. **Bookmarking is idempotent.** POST twice, assert 200 both times and exactly one row in `analytics.bookmarks`.
2. **A bookmark is per user.** User A bookmarks; list as user B and assert `bookmarked` is false for that row, then list as A and assert true. This is the test that would catch reading `user_id` from anywhere but the session.
3. **`:read` is enough, `:manage` is not required.** A principal holding only `data_api_endpoint:read` can bookmark. Bookmarking is a view preference.
4. **An endpoint in another workspace is 404, not 403.** Absence and denial must be indistinguishable from outside — the rule this crate's other suites already hold.

Each assertion gets mutation-checked in Step 6.

- [ ] **Step 3: Run to verify they fail**

Run: `DATABASE_URL="postgres://egret:egret@localhost:5432/egret_test" cargo test -p egret-dataapi --test bookmarks`
Expected: FAIL to compile — the routes do not exist. Note the duration on later runs: a sub-100ms "ok" means `DATABASE_URL` never reached the process and nothing was asserted.

- [ ] **Step 4: Implement**

Add to `endpoints.rs`, beside the other store methods:

```rust
    /// Bookmark `endpoint` for `user`. Idempotent: a second call is a no-op
    /// rather than an error, because the UI's star is a toggle and a double
    /// click must not 500.
    pub async fn add_bookmark(&self, user: Uuid, endpoint: Uuid) -> Result<()> {
        sqlx::query(
            "INSERT INTO analytics.bookmarks (user_id, entity_type, entity_id) \
             VALUES ($1,'data_api_endpoint',$2) ON CONFLICT DO NOTHING",
        )
        .bind(user)
        .bind(endpoint)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn remove_bookmark(&self, user: Uuid, endpoint: Uuid) -> Result<()> {
        sqlx::query(
            "DELETE FROM analytics.bookmarks \
             WHERE user_id=$1 AND entity_type='data_api_endpoint' AND entity_id=$2",
        )
        .bind(user)
        .bind(endpoint)
        .execute(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }
```

`entity_type` is the string `'data_api_endpoint'`. It appears in three places (both statements and the list query) — if they disagree, bookmarks write to one bucket and read from another, and the star silently never lights. Consider a `const BOOKMARK_ENTITY: &str = "data_api_endpoint";` so the three cannot drift.

Add `bookmarked` to the list query as an `EXISTS` sub-select keyed on the caller, exactly as `dataset.rs:203` does, and add `pub bookmarked: bool` to the list DTO.

Add the two handlers to `routes.rs` following `ds_bookmark_add`/`ds_bookmark_remove`, gated on `EP_READ`, with the endpoint resolved for workspace containment first so a foreign id is 404.

- [ ] **Step 5: Regenerate `openapi.json`**

```bash
cargo run -p egret-bin -- openapi > openapi.json
cargo test -p egret-server committed_openapi_json_is_current
```
Expected: PASS. This gate lives in a different crate from the one you edited, which is why it gets forgotten.

- [ ] **Step 6: Mutation-check each test**

Four mutations, each built to zero errors **first**:

1. Drop `ON CONFLICT DO NOTHING` → test 1 must fail (or error).
2. Bind a fixed uuid instead of `ctx.principal.user_id` → test 2 must fail.
3. Change the handler's gate from `EP_READ` to `EP_MANAGE` → test 3 must fail.
4. Remove the workspace-containment resolve → test 4 must fail.

If a mutation makes the code fail to compile, re-cut it so it builds, then read the result.

- [ ] **Step 7: Run the crate suite and commit**

```bash
DATABASE_URL="postgres://egret:egret@localhost:5432/egret_test" cargo test -p egret-dataapi
cargo fmt --all
cargo clippy -p egret-dataapi --all-targets -- -D warnings
```

```bash
git add crates/egret-dataapi crates/egret-dataapi-api openapi.json
git commit -m "feat(dataapi): bookmark an endpoint, per user, on the generic bookmarks table"
```

---

### Task 3: `BookmarkToggle`, and wire it into `Endpoints`

**Files:**
- Create: `frontend/src/components/explore/BookmarkToggle.tsx`
- Test: `frontend/src/components/explore/__tests__/BookmarkToggle.test.tsx`
- Modify: `frontend/src/routes/explore/dataapi/Endpoints.tsx`
- Modify: `frontend/src/routes/explore/dataapi/__tests__/Endpoints.test.tsx`
- Modify: `frontend/src/api/dataApi.ts` (the invalidation wrapper)

**Interfaces:**
- Consumes: `ListToolbarButton({ icon, label, active?, filled?, onClick })`; the generated bookmark hooks from Task 2.
- Produces: `BookmarkToggle({ active, onToggle })` — the header filter button; and `BookmarkStar({ bookmarked, onToggle, name })` — the per-row star.

- [ ] **Step 1: Write the failing test**

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TooltipProvider } from "@/components/ui/tooltip";
import { BookmarkToggle, BookmarkStar } from "@/components/explore/BookmarkToggle";

const wrap = (ui: React.ReactNode) => render(<TooltipProvider>{ui}</TooltipProvider>);

describe("BookmarkToggle", () => {
  it("reads as pressed when the bookmarked-only filter is on", () => {
    const { rerender } = wrap(<BookmarkToggle active={false} onToggle={() => {}} />);
    expect(screen.getByRole("button", { name: "Bookmarks" })).toHaveAttribute("aria-pressed", "false");
    rerender(<TooltipProvider><BookmarkToggle active onToggle={() => {}} /></TooltipProvider>);
    expect(screen.getByRole("button", { name: "Bookmarks" })).toHaveAttribute("aria-pressed", "true");
  });
});

describe("BookmarkStar", () => {
  it("names the row it belongs to, so 30 stars are not 30 identical controls", async () => {
    const onToggle = vi.fn();
    wrap(<BookmarkStar bookmarked={false} name="orders by region" onToggle={onToggle} />);
    const btn = screen.getByRole("button", { name: /orders by region/i });
    expect(btn).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(btn);
    expect(onToggle).toHaveBeenCalledTimes(1);
  });

  it("says which way the click goes", () => {
    wrap(<BookmarkStar bookmarked name="orders" onToggle={() => {}} />);
    // A star that is already filled must offer to REMOVE, not to add again.
    expect(screen.getByRole("button", { name: /remove bookmark/i })).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd frontend && npx vitest run src/components/explore/__tests__/BookmarkToggle.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Write the component**

```tsx
import { Star } from "lucide-react";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";

/**
 * The header filter: show only bookmarked rows.
 *
 * Four pages own a copy of this button and its `onlyBookmarked` state; this is
 * the shared one. It holds no state of its own — the page owns the filter,
 * because the page is what applies it to its rows.
 */
export function BookmarkToggle({ active, onToggle }: { active: boolean; onToggle: () => void }) {
  return <ListToolbarButton icon={Star} label="Bookmarks" active={active} filled onClick={onToggle} />;
}

/**
 * The per-row star.
 *
 * `name` is required and reaches the accessible name, because a list of thirty
 * rows otherwise gives a screen-reader user thirty controls called "Bookmark"
 * with nothing to tell them apart — the same defect the row-actions menus had.
 * The label also states the DIRECTION of the click: a filled star that says
 * "Bookmark" is telling the user the opposite of what pressing it does.
 */
export function BookmarkStar({
  bookmarked,
  name,
  onToggle,
}: {
  bookmarked: boolean;
  name: string;
  onToggle: () => void;
}) {
  const label = bookmarked ? `Remove bookmark from ${name}` : `Bookmark ${name}`;
  return <ListToolbarButton icon={Star} label={label} active={bookmarked} filled onClick={onToggle} />;
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd frontend && npx vitest run src/components/explore/__tests__/BookmarkToggle.test.tsx`
Expected: PASS, 3 tests.

- [ ] **Step 5: Wire it into `Endpoints`**

Regenerate the client (`npm run gen:api`) so the Task 2 hooks exist. Then:

- add `BookmarkToggle` to the page toolbar with an `onlyBookmarked` state that filters `rows`;
- add a `BookmarkStar` column to each row, driven by the DTO's new `bookmarked` field;
- **route both mutations through `src/api/dataApi.ts`** with an `onSuccess` that invalidates the endpoint list, reading the workspace off the mutation **variables**. Six mutations in the previous phase toasted success over a stale list because orval hooks carry no `onSuccess` and `queryClient` sets `staleTime: 30000`; that wrapper is the house fix.

- [ ] **Step 6: Test the wiring**

Add to `Endpoints.test.tsx`:

- filtering to bookmarked-only hides an unbookmarked row;
- clicking a row's star calls the add mutation with that endpoint's id, and clicking a filled one calls remove.

- [ ] **Step 7: Mutation-check**

1. Make the `onlyBookmarked` filter a no-op → the filter test must fail.
2. Point the star at a fixed id instead of the row's → the star test must fail.

Both built to zero errors first.

- [ ] **Step 8: Full gates and commit**

```bash
cd frontend && npx vitest run && npm run typecheck && npm run lint && npm run build
```

```bash
git add frontend/src
git commit -m "feat(dataapi): bookmark endpoints from the list"
```

---

### Task 4: `FolderToolbar`, and move-to-folder in the row menu

**Files:**
- Create: `frontend/src/components/explore/FolderToolbar.tsx`
- Test: `frontend/src/components/explore/__tests__/FolderToolbar.test.tsx`
- Modify: `frontend/src/routes/explore/dataapi/Endpoints.tsx`
- Modify: `frontend/src/routes/explore/dataapi/EndpointActionsMenu.tsx`
- Modify: `frontend/src/routes/explore/dataapi/__tests__/Endpoints.test.tsx`

**Interfaces:**
- Consumes: `ListToolbarButton`; `FolderNameDialog` and `MoveToFolderItems` from `@/components/explore/…` (read them for their real props); `useFolders` / `useFolderMutations` from `@/api/folders`; the existing `useUpdateEndpoint` for `folder_id`.
- Produces: `FolderToolbar({ onNewFolder })` — the header button only. The dialog stays owned by the page, because the page already owns the folder query.

**Scope note:** the backend is already complete for this — `UpdateEndpointInput.folder_id` is `Option<Option<Uuid>>`, the store honours it, and `routes.rs:912` validates containment. This task is UI only.

- [ ] **Step 1: Write the failing test for the toolbar button**

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TooltipProvider } from "@/components/ui/tooltip";
import { FolderToolbar } from "@/components/explore/FolderToolbar";

describe("FolderToolbar", () => {
  it("offers a New folder control with an accessible name", async () => {
    const onNewFolder = vi.fn();
    render(<TooltipProvider><FolderToolbar onNewFolder={onNewFolder} /></TooltipProvider>);
    await userEvent.click(screen.getByRole("button", { name: "New folder" }));
    expect(onNewFolder).toHaveBeenCalledTimes(1);
  });
});
```

- [ ] **Step 2: Run to verify it fails, then write the component**

```tsx
import { FolderPlus } from "lucide-react";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";

/**
 * The New-folder control, as a header toolbar button.
 *
 * Deliberately just the button. The dialog it opens needs the folder list and
 * the folder mutations, both of which the page already holds for its grouping —
 * a toolbar that fetched them would own a second copy of the same query, and
 * the two could disagree about which folders exist.
 */
export function FolderToolbar({ onNewFolder }: { onNewFolder: () => void }) {
  return <ListToolbarButton icon={FolderPlus} label="New folder" onClick={onNewFolder} />;
}
```

- [ ] **Step 3: Wire the page**

- `FolderToolbar` in the toolbar, opening the existing `FolderNameDialog` (read its props; `Datasets.tsx` shows the four modes it supports);
- `MoveToFolderItems` in `EndpointActionsMenu`, calling `useUpdateEndpoint` with `{ folder_id: <id or null> }`;
- keep the existing `FolderHeaderRow` grouping as it is.

**`folder_id` is `Option<Option<Uuid>>` on the wire**: sending `null` means "move to Uncategorized", and **omitting the key** means "leave the folder alone". A PATCH that always sends `folder_id` would move every endpoint to Uncategorized on any edit. The existing `diffToUpdateInput` in `EndpointDetail.tsx` shows the house pattern for send-only-what-changed.

- [ ] **Step 4: Test the wiring**

- moving a row to a folder calls the update mutation with `{ folder_id: "f-1" }`;
- moving it to Uncategorized sends `folder_id: null`, **not** an omitted key — assert on the actual payload, since those two are different requests with different meanings;
- creating a folder calls the folder-create mutation and the list re-groups.

- [ ] **Step 5: Mutation-check**

Change the Uncategorized case to omit `folder_id` instead of sending `null`. Build to zero errors, then confirm the second test fails. This is the assertion most likely to be written loosely, because both shapes "look like" moving to no folder.

- [ ] **Step 6: Full gates and commit**

```bash
cd frontend && npx vitest run && npm run typecheck && npm run lint && npm run build
```

```bash
git add frontend/src
git commit -m "feat(dataapi): create folders and move endpoints between them"
```

---

## After the plan: verify it in a browser

Every phase of this project has found a defect in a browser that no test could see. Do not report this complete on a green suite.

1. **Kill any stale `egret` on :8080 first** and confirm the running server is the one you just built — a previous session's binary served an older API and every check would have measured the wrong server.
2. **Bookmark a row, reload, and confirm the star is still filled.** That is the round trip through the database, which no mocked test exercises.
3. **Bookmark as one user; log in as another and confirm their star is empty.** Per-user is the property most likely to be wrong and least likely to be noticed.
4. **Create a folder, move an endpoint into it, then move it back to Uncategorized** and confirm it lands there rather than disappearing.
5. **Point an endpoint's `folder_id` at a folder, delete that folder, and reload** — the row must appear under Uncategorized, not vanish. That is Task 1's fix, in the product.
6. **Filter to bookmarked-only while a folder is collapsed** and confirm the counts on screen agree with what is listed.

---

## Self-Review

**Spec coverage (phase 3):** bookmark backend — Task 2; bookmark UI — Task 3; folder create/move UI — Task 4; the `groupByFolder` fix — Task 1, added because it is the shared-code root cause of the disappearing rows and it belongs with folder work rather than after it.

**Placeholder scan:** Tasks 2 and 4 say "read the sibling and follow it" for the store statements, the route shape, and `FolderNameDialog`/`MoveToFolderItems`' props. Each names the exact file and lines; each is a *match an existing pattern* instruction rather than a decision deferred. Task 2's test bodies are described rather than written out — that is the one place this plan is thinner than it should be, and the four mutations in Step 6 are what keep the tests honest.

**Type consistency:** `BookmarkToggle({ active, onToggle })` and `BookmarkStar({ bookmarked, name, onToggle })` are used with those exact props in Task 3's wiring. `FolderToolbar({ onNewFolder })` likewise in Task 4. `bookmarked: bool` is added to the DTO in Task 2 and read in Task 3. `entity_type` is `'data_api_endpoint'` in all three places it appears.
