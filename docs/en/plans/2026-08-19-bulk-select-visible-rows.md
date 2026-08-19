# Bulk Select Acts Only On Visible Rows — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Stop five Explore pages from letting a bulk action destroy rows the user cannot see.

**Architecture:** Each page already computes `groups` from `rows` and renders `{!isCollapsed && group.rows.map(renderRow)}`. Each then passes the **pre-grouping** `rows` to `useBulkSelect`. Derive the rendered set and pass that instead — the fix `Endpoints.tsx` already carries.

**Tech Stack:** React 18, TypeScript, vitest + Testing Library.

## The defect

`useBulkSelect`'s own doc comment states the contract and names the five pages that break it:

> **`rows` must be the set actually RENDERED — not the page's filtered/sorted model of it.**

Measured — all five are identical in shape:

| Page | `useBulkSelect` | grouping | render |
|---|---|---|---|
| `Charts.tsx` | `:68` `rows` | `:70` | `:242` |
| `Dashboards.tsx` | `:92` `rows` | `:94` | `:286` |
| `Alerts.tsx` | `:151` `rows` | `:153` | `:574` |
| `Notebooks.tsx` | `:303` `rows` | `:305` | `:593` |
| `Datasets.tsx` | `:88` `rows` | `:90` | `:271` |

**Concretely:** collapse a folder, turn on bulk mode, click "Select all", then Move to Trash. Rows inside the collapsed folder are trashed, having never appeared on screen. `toggleAll` writes their ids; `allSelected`/`someSelected` count them; `selectedIds` reports them.

The second half matters too and is easy to miss: `selectedIds` re-derives from whatever array it was handed, so passing the visible set is also what **unselects a row hidden after it was picked** (folder collapsed, filter applied). Passing `rows` leaves it selected invisibly.

## Global Constraints

- **`Endpoints.tsx:240-254` is the reference implementation.** Match it, including the `showTrash ? rows : …` branch for pages with a flat trash view. Do not invent a different approach.
- The shared key for the folderless bucket is `UNCATEGORIZED_KEY` — import it rather than re-typing `"__uncat__"`. Confirm the export path before using it.
- **Do not change `useBulkSelect` itself.** It cannot detect this, as its comment explains; the fix belongs at each call site. **Do update its closing paragraph** once the five are migrated — it currently says they "still pass their pre-grouping `rows`", which will be false.
- `MembersPanel.tsx:109` and `UsersPage.tsx:310` are **out of scope** — neither groups its rows. Verify that before leaving them alone; if either does group, report it rather than fixing it silently.
- eslint baseline: **0 errors / 177 warnings**. `tsc --noEmit` clean.
- Run the frontend suite unpiped. NEVER pipe into `head`/`tail` — SIGPIPE kills a run mid-suite while exiting 0.
- **Commit before cutting any mutation.** An implementer on the parent branch lost every uncommitted edit to a `git checkout --` restore.

## Task 1: Migrate the five pages

For each page, in this order — write the failing test first, watch it fail, then fix that page.

- [ ] **Step 1: Write the failing test**

In each page's existing test file, with at least one folder collapsed:
1. rows inside a collapsed folder are NOT selected by "Select all", and the bulk action is not called with their ids;
2. a row selected while visible drops out of the selection when its folder is collapsed.

**The fixture must place rows in at least two folders, with at least one row in each**, and collapse only one. A fixture where every row sits in the collapsed folder cannot tell a correct fix from one that selects nothing at all — both give an empty selection.

**Assert the ids passed to the mutation**, not merely that it was called. `expect(mock).toHaveBeenCalled()` passes when the wrong rows are trashed, which is the entire defect.

- [ ] **Step 2: Run it and watch it fail**

Expected: the selection includes the hidden rows, and the bulk action is called with their ids.

- [ ] **Step 3: Apply the fix**

Derive the rendered set exactly as `Endpoints.tsx:240-249` does and pass it to `useBulkSelect`.

- [ ] **Step 4: Run the page's tests**

- [ ] **Step 5: Commit** — one commit per page, so a bisect lands on a single page.

## Task 2: Update the hook's doc comment and mutation-check the lot

- [ ] Rewrite the closing paragraph of `useBulkSelect` to say the contract is now met at every call site, naming the two ungrouped exceptions. **Do not leave a comment asserting a state of the world that the same commit makes false.**
- [ ] Mutation-check each page: revert that page to `useBulkSelect(rows)`, build to zero errors, **confirm the build is clean**, then read the result. Its test must fail. A mutation that fails to compile proves nothing.
- [ ] Full suite unpiped, eslint at baseline, `tsc` clean.

## Self-Review

**Spec coverage:** five pages in Task 1, the now-false comment in Task 2. **Placeholder scan:** the per-page test bodies are described rather than written, because each page's mocking idiom differs; the fixture and assertion requirements are stated instead. **Type consistency:** `visibleRows` is the name throughout, matching `Endpoints.tsx`.
