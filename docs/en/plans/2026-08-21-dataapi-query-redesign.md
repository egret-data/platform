# Data API Query Section Redesign

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans.

**Goal:** Make the endpoint's Query section a place you can actually write SQL in: a real code editor, everything around it in tables, and the preview visible while you type.

**Architecture:** Adopt the layout SQL Lab already uses in this product — a full-height flex column with a resizable vertical split — rather than inventing a third editing idiom.

**Tech Stack:** React 18, TypeScript, Monaco (`@monaco-editor/react`), `react-resizable-panels` v4, Tailwind v4.

## What the user asked for

1. Everything except the SQL becomes a table.
2. The SQL becomes a code editor.
3. The preview at the bottom is hard to see and awkward while editing SQL.

**User-approved layout (asked and answered):** the full-height SQL Lab shape — a collapsible strip of tables on top, then editor above / preview below with a draggable divider.

## Measured starting state

- `frontend/src/routes/explore/dataapi/detail/Query.tsx` is 77 lines: a `Select` for the database, a plain **`<Textarea rows={12}>`** for the SQL, then `ParamEditor`, then `PreviewBlock` — all stacked, so the preview sits below the parameter editor and is off-screen while editing.
- `frontend/src/routes/explore/dataapi/detail/ParamEditor.tsx` renders **a `Card` per parameter** with `grid grid-cols-2` inside. This is the "too many cards" shape the whole programme has been removing.
- **`frontend/src/components/sqllab/SqlEditor.tsx` already exists and is reusable**: `{value, onChange, schemaWords, schema?, onRun?, onSelectionChange?}`. Monaco, egret-themed light/dark, built-in SQL keyword completions.
- **`react-resizable-panels` v4** is already a dependency and `components/sqllab/SqlTab.tsx` is the working example: `import { Group, Panel, Separator as PanelSeparator }`.
- **Full height is available.** `components/layout/AppShell.tsx:39` is `<main className="min-w-0 flex-1 overflow-auto p-6">` inside an `h-screen` flex column, which is why `routes/explore/SqlLab.tsx:20` can be `<div className="flex h-full flex-col">`. No magic viewport constant is needed.

## Global Constraints

- **`react-resizable-panels` v4 takes sizes as STRINGS.** A bare number means **pixels**, not percent — the silent-semantics trap this repo already hit during the v4 upgrade. `SqlTab.tsx` uses `defaultSize="55%"`; copy that form.
- **Test values must NOT move into the parameters table.** `Query.tsx`'s existing comment explains why: `ParamEditor` holds the *declaration*, which is saved; the preview holds a *test value*, which is not. A test value in the declaration row invites the reader to think it is stored. Keep them in the preview panel.
- **The other five sections must not regress.** The shell change below affects all of them.
- Tokens are OKLCH (Tailwind v4): inline styles use `var(--token)`, never `hsl(var(--token))`.
- eslint baseline **0 errors / 177 warnings**; `tsc --noEmit` clean. No msw.
- Run the frontend suite unpiped. NEVER pipe into `head`/`tail` — SIGPIPE kills a run mid-suite while exiting 0.
- **A shell hook rewrites `... | grep` into `rtk grep`, which swallows piped stdin** — redirect to a file and grep the file.
- **Commit before cutting any mutation.** A mutation that fails to compile — or a patch that never lands — proves nothing.
- The endpoint editor is wrapped in `<fieldset disabled={trashed}>`; a trashed endpoint must stay read-only, and the Monaco editor must be read-only too. A `fieldset` does **not** disable a Monaco instance, so this needs an explicit prop.

---

### Task 1: Let one section fill the height without breaking the other five

**Files:** `frontend/src/routes/explore/dataapi/EndpointDetail.tsx`
**Test:** `frontend/src/routes/explore/dataapi/__tests__/EndpointDetail.test.tsx`

The shell is currently a plain growing block: `<div className="p-4">` → header → nav → `<fieldset disabled={trashed} className="min-w-0">` → `<Outlet/>`.

- [ ] **Step 1: Make the shell a bounded flex column**

Root becomes a flex column that fills `main`; header and nav stay fixed-size; the `fieldset` becomes the flexible remainder that scrolls. `routes/explore/SqlLab.tsx:20` and its `min-h-0 flex-1 overflow-hidden` body are the working reference — **read them before writing**, including why `min-h-0` is needed (a flex child's default `min-height: auto` refuses to shrink below its content).

Keep `min-w-0` on the `fieldset`: its comment records that a `fieldset`'s UA `min-inline-size: min-content` is not in Tailwind's preflight and would otherwise refuse to shrink.

- [ ] **Step 2: Prove the five other sections still lay out**

Add a test that a long section still scrolls rather than being clipped, and that the header and nav stay visible. **Assert something a broken layout would fail** — a test that merely renders a section passes whether or not the height chain works. Reason about what you can actually observe in jsdom (which does no layout) and say so honestly in the report: if the only real check is a browser, say that rather than writing a test that pretends.

- [ ] **Step 3: Commit**

---

### Task 2: Parameters become a table

**Files:** `frontend/src/routes/explore/dataapi/detail/ParamEditor.tsx`
**Test:** its existing test file

- [ ] **Step 1: One row per parameter**

Columns: **Name · Type · Required · Default · Description**, plus a per-row remove control and an "Add parameter" action. Replace the card-per-parameter stack; keep every field that exists today — this is a re-presentation, not a feature cut. Check the current file for fields the list above misses (e.g. allowed values) and keep them; if one does not fit a column, say where you put it and why.

Follow `components/config/ConfigTable.tsx` for the table idiom the programme standardised on, and `docs/en/ui-conventions.md` for the rules it encodes.

- [ ] **Step 2: The validation and empty states survive the reshape**

The current editor surfaces per-parameter problems. Find how, and keep them attached to the row they belong to — a table that loses the error's anchor is worse than the cards.

- [ ] **Step 3: Test, mutation-check, commit**

Assert on the **values written back through `onChange`**, not merely that it fired. A fixture with **two** parameters, so a change that edits the wrong row is distinguishable from one that edits the right one — with one row, both look identical.

---

### Task 3: The editor, the split, and the preview

**Files:** `frontend/src/routes/explore/dataapi/detail/Query.tsx`, `detail/PreviewBlock.tsx`
**Test:** `frontend/src/routes/explore/dataapi/__tests__/` (add)

- [ ] **Step 1: Lay out the section**

```
flex h-full flex-col
├── collapsible strip: Database (a ConfigTable row) + the parameters table
└── Group direction="vertical"  (min-h-0 flex-1)
    ├── Panel defaultSize="55%"  →  SqlEditor
    ├── Separator                →  the drag handle
    └── Panel                    →  preview: test values + results
```

The database picker becomes a `ConfigTable` row rather than a bare `Select`, per the user's "everything except the SQL becomes a table".

- [ ] **Step 2: Wire the editor**

`SqlEditor` takes `schemaWords` — pass `[]`. **Do not invent a schema source.** SQL Lab's completions come from `words`, local state its schema explorer fills through a `SchemaSink`; there is no reusable hook, and wiring one is its own task. Monaco still offers the built-in SQL keyword list. Say in the report that schema-aware completion is deliberately absent.

Pass `onRun` so **⌘↵ runs the preview** — the affordance that makes the split worth having. Show the shortcut in the preview panel's header so it is discoverable.

- [ ] **Step 3: Honour the trashed state**

`<fieldset disabled>` does not reach a Monaco instance. Make the editor read-only when the endpoint is trashed, and **test it** — the existing trashed-endpoint test asserts the editor is read-only, and it must not start passing for the wrong reason once the textarea is gone.

- [ ] **Step 4: Keep the preview's own reasoning intact**

`PreviewBlock` already explains itself: it runs the on-screen draft as the author, saves nothing, spends no rate limit, and does not appear in Activity. Its note must survive the move into the panel — the reader needs it exactly where the button is.

- [ ] **Step 5: Test, mutation-check, commit**

Cover: the editor renders and edits write to the form; ⌘↵ triggers the preview request; the panel sizes are strings; a trashed endpoint's editor is read-only. Mutation-check the ⌘↵ wiring and the read-only guard.

---

## After the plan: check it in a browser

Every phase of this programme found something in a browser that no test saw, and this one is a layout change — the class of change jsdom is least able to judge.

1. The split drags, and both panels survive a window resize.
2. The editor is genuinely full-height and the page does not double-scroll.
3. ⌘↵ runs the preview with the editor focused.
4. The other five sections still scroll normally, with the header and nav in place.
5. A trashed endpoint's editor cannot be typed into.

## Self-Review

**Spec coverage:** the user's three points map to Tasks 2+3 (tables), 3 (editor), and 1+3 (preview placement). **Placeholder scan:** Task 2's column list is explicit but asks the implementer to check for fields it misses rather than pretending the list is complete. **Type consistency:** `SqlEditor`'s existing prop names are used unchanged; panel sizes are strings throughout.
