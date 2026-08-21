# Bulk Actions Stop Acting On Rows You Cannot See

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** `MembersPanel` and `UsersPage` obey the contract every other bulk-select surface already obeys — a row that is not on screen is not in the count and not in the action.

**Architecture:** `FilterableTable` reports the ids it drew; the two pages feed those to `useBulkSelect` and take their targets from `selectedIds`.

**Tech Stack:** React 18, TypeScript.

## The defect

`useBulkSelect`'s own doc comment states the contract:

> `selectedIds` re-derives from `rows` on every render rather than just handing back `selected` … a row picked while visible and then hidden (its folder collapsed, a filter applied) drops out of the count and out of the action.

PR #523 made the five grouped Explore pages honour it. These two do not:

- `routes/control-plane/access/UsersPage.tsx:312` — `const selectedUsers = () => filtered.filter((u) => bulk.selected.has(u.id));`
- `components/access/MembersPanel.tsx:110` — `const selectedMembers = () => bulkRows.filter((m) => bulk.selected.has(m.id));`

Both filter the **page's own list** by `bulk.selected`, never touching `bulk.selectedIds`. But `FilterableTable` owns a search box, and on `UsersPage` a `pageSize={25}` pager — it renders only `pageRows` (`components/explore/FilterableTable.tsx`, `shown` → `pageRows`). So a row selected while visible and then searched away, or left behind on another page, **stays selected and is still acted on**.

The actions are not recoverable ones: `UsersPage` offers **Disable** and **Assign role**; `MembersPanel` offers **Remove from workspace** and **Set role**.

**PR #524 fixed the neighbouring half** — the table's own header "Select all" used to toggle every row the search left, across all pages, and now toggles only the page it drew. **Changing the `rows` argument passed to `useBulkSelect` will NOT fix this half**, because these two pages never read `selectedIds`; that was measured during #524's review.

## Global Constraints

- **Follow the existing contract, do not invent a second one.** The semantics are already written down in `useBulkSelect` and already shipped on five pages. This makes two more pages consistent with it.
- **`FilterableTable` has seven-plus consumers.** Anything added to it must be optional and inert for the ones that do not opt in.
- **No effect-driven render loops.** `pageRows` is derived fresh each render, so a naive `useEffect` on the array identity fires forever. Key any effect on a stable value derived from the ids' *contents*.
- eslint baseline **0 errors / 177 warnings**; `tsc --noEmit` clean. No msw.
- Run the frontend suite unpiped. NEVER pipe into `head`/`tail`.
- **A shell hook rewrites `... | grep` into `rtk grep`, which swallows piped stdin** — redirect to a file and grep the file.
- **`cp` is aliased to `cp -i` and will hang waiting for input.** Restore files with `git checkout --` or a shell redirect, never `cp`.
- **Commit before cutting any mutation.** Build to zero errors, CONFIRM the patch landed, then read.

---

### Task 1: The table reports what it drew

**Files:** `frontend/src/components/explore/FilterableTable.tsx`, its tests

- [ ] **Step 1: Add an optional way to observe the drawn rows**

Extend the existing `selection` prop rather than adding a sibling — the two are the same concern, and a consumer that does not select has no use for the ids.

Whatever shape you choose, it must be **inert for consumers that omit it** and must not fire on every render. Say in a comment why the dependency is what it is.

- [ ] **Step 2: Test it**

Assert the reported ids **change when the search narrows the table** and when the page changes — the two mechanisms that create the gap. A test that only checks the callback fires once on mount would pass against an implementation that never updates.

**Use a fixture with more rows than one page** so "reported the page" and "reported everything" are distinguishable. With a single page they are the same list, and the test proves nothing.

- [ ] **Step 3: Commit**

---

### Task 2: The two pages take their targets from what was drawn

**Files:** `frontend/src/routes/control-plane/access/UsersPage.tsx`, `frontend/src/components/access/MembersPanel.tsx`, their tests

- [ ] **Step 1: Feed `useBulkSelect` the drawn rows**

Then `bulk.selectedIds` means what it means everywhere else, and the "{n} selected" strip stops disagreeing with the screen.

- [ ] **Step 2: Take targets from `selectedIds`, not from `selected` over the full list**

This is the half that actually changes behaviour. Both `selectedMembers()` and `selectedUsers()` must go.

- [ ] **Step 3: Test the property, not the plumbing**

For each page: select a row, narrow the search so it is no longer drawn, fire the bulk action, and assert **the mutation is not called with that row's id**. Assert the id passed, not the call count.

**The fixture needs at least two rows with a search term that separates them**, so "acted on the right row" and "acted on row 0" are different claims. That distinction has now been got wrong three times in this programme — a fixture where the correct answer and the wrong answer coincide proves nothing.

- [ ] **Step 4: Mutation-check**

Put the old `filtered.filter(...)` derivation back and confirm the new test fails. Then remove the drawn-rows wiring and confirm it fails again. Both must die.

- [ ] **Step 5: Commit**

---

## After the plan: check it in a browser

The local stack has users in the control plane. Select a user, type a search that hides them, and confirm the count drops and Disable does not touch them. Then the same on a workspace's Members panel.

## Self-Review

**Spec coverage:** Task 1 is the mechanism, Task 2 is the behaviour; the defect is only closed by both. **Placeholder scan:** Task 1 Step 1 deliberately does not prescribe the prop's shape — the constraint (inert when omitted, no render loop) is what matters, and the right shape depends on how the effect is keyed. **Type consistency:** `selectedIds` is the single source of targets on both pages afterwards.
