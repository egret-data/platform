# Frontend rules: one rulebook for egret's UI, enforced where a machine can check it

**Status:** design, 2026-10-10; not yet implemented.
**Applies to:** everything under `frontend/`, starting with W7 (the workflow UI). Existing code follows the rules as it is touched (§6).
**Related:** `docs/{en,vi}/ui-conventions.md` (the reasons behind earlier decisions; it stays and becomes the rationale this rulebook links to).

**Decided with the user, 2026-10-10:**
- **Enforcement: the rulebook plus lint.** Rules that a machine can check reliably become ESLint errors. Code that already breaks them is listed as named debt, and new code cannot add to it. There is no bulk migration before W7.
- **Structure: a new short rulebook, and the old document is kept.** `docs/en/frontend-rules.md` is the normative source. `docs/en/ui-conventions.md` is the rationale and the record of reversed decisions, and stops being a source of rules.
- **UI copy is English, with no i18n.** One copy style (§4.9). The Vietnamese map and region data stays as it is.
- **Reference products: egret's visual system, the source product's UX.** Every area uses egret's tokens, components and page frames. The structure and the interaction flow of an area copy that area's source product, checked against a running instance: BI follows Metabase, Workflow follows Kestra. Charts keep the Metabase series palette.
- **The rule catalogue (§4) as presented in the brainstorm,** with the starred choices accepted:
  - create flows split by page and dialog according to `ui-conventions.md` §3;
  - skeletons for loading content;
  - forms stay hand-written;
  - orval hooks plus a query-key factory per domain;
  - `React.lazy` for every new feature area.

## 1. Why

A survey of `frontend/src` on 2026-10-10 found a sound base but no single standard:
- **Tokens and primitives are sound:** OKLCH tokens, shadcn/Radix primitives, shared list-page parts.
- **Two page frames.** `ListPageShell` serves Explore and the Data API. Control plane and governance hand-roll theirs, with about 49 route-level `<h1>`.
- **Three table styles:** `FilterableTable`/`ui/table`, TanStack Table in `ChartTable`, and about 9 hand-written `<table>`.
- **Three loading styles:** skeleton, the text "Loading…", and spinners.
- **Two empty styles:** `EmptyState` in about 21 places, and about 53 hand-written "No X" texts.
- **Raw colours:** about 35 raw Tailwind palette classes and some arbitrary hex values bypass the tokens.
- **Destructive confirms:** `window.confirm` beside `ConfirmDeleteDialog`.
- **Query keys:** two prefixes (`cp`, `control-plane`) for one domain.
- **No route-level code splitting.**
- **No a11y lint.**

`ui-conventions.md` records why many past decisions were taken, but it is 2,225 lines of case history centred on the Data API. An agent cannot use it as a checklist, and its Vietnamese twin has drifted (662 lines). W7 is about to add a whole feature area, and it should be built to one written standard.

## 2. Deliverables

1. **`docs/en/frontend-rules.md` (canonical) and `docs/vi/frontend-rules.md` (reference).** These hold the rule catalogue of §4 in the format of §3, the reviewer checklist of §5, and the debt register of §6. The English file stays under about 400 lines so an agent reads it whole.
2. **Pointer files**, a few lines each, telling an agent to read the rulebook before changing anything under `frontend/`:
   - `frontend/CLAUDE.md`, which Claude Code loads automatically when it works in that directory;
   - `frontend/AGENTS.md`, for other agents.
3. **`docs/{en,vi}/ui-conventions.md`:** one line at the top of each saying the rulebook is normative and this file is its rationale. Nothing else in the file changes. The Vietnamese twin's drift is recorded as debt.
4. **Lint** (§7):
   - a local ESLint plugin with egret's own rules;
   - `eslint-plugin-jsx-a11y-x` (the maintained fork of `eslint-plugin-jsx-a11y`);
   - a legacy list per rule;
   - a check that keeps every legacy list honest.
5. **Removal of `frontend/src/theme/egret-theme.ts`.** It is a stale orange hex palette, and nothing imports it; the only matches for `egret-theme` are the theme provider's localStorage key.

## 3. The rule format

Every rule has:
- an **id**: `FE-<GROUP>-<n>`, stable forever, and never reused once retired;
- a **level**: **MUST** or **SHOULD**. Breaking a SHOULD needs a one-line reason in the PR;
- the **rule**, in one or two sentences;
- **do this**: the real component, hook or file to use, by path;
- **checked by**: `lint (egret/<rule>)`, `lint (jsx-a11y)`, `test`, `browser` or `review`;
- **why** (optional): a link to the section of `ui-conventions.md` or to the incident behind the rule.

A rule that names a component names the file it lives in. If a component moves, the rulebook changes in the same PR.

## 4. The rule catalogue

**Levels.** Every rule below is MUST unless it says SHOULD.

**Checking.** A rule says how it is checked only when that is lint, a test or the browser; every other rule is checked by review.

### 4.1 Page frame (FE-PAGE)
1. A list page uses `ListPageShell` (`src/components/explore/ListPageShell.tsx`), and that shell owns the page's single `h1`.
   - Order: title, then the toolbar (`ListToolbarButton`), then `SummaryStrip`, then the table (`ui-conventions.md` §1).
2. A detail page with sections uses `ui/tabs`. The selected tab, and any filter or selection a user would want to share, live in the URL (`?tab=`, `?q=`). The Back button walks them.
3. **Creating:**
   - Create on a page (`…/new`, declared before `:id`) when the form has several sections, or when its result must be read before leaving (a one-time secret).
   - Create in a dialog when it has one to three fields.
   - Entry points are links, not imperative pushes (`ui-conventions.md` §3).
4. A tabular list uses `FilterableTable` (`src/components/explore/FilterableTable.tsx`) or `ui/table`.
   - A hand-written `<table>` is allowed only for a specialised grid (pivot, SQL results), and carries a comment saying why.
5. Settings use `ConfigTable`/`ConfigTableRow`/`ConfigSection` (`src/components/config/`), in the `Setting | Details | Value` shape (`ui-conventions.md` §4).
   - A flat property map is one searchable key/value list, not a bespoke form per field.

### 4.2 States (FE-STATE)
1. Every data view renders loading, empty and error, and checks `isError` before it checks for data. An error never renders as empty (`ui-conventions.md` §2).
2. Loading page or list content renders a skeleton (`ListStates`, `ListSkeleton`, `ui/skeleton`).
   - A spinner (`Loader2`) is only for a button or an in-place action.
   - The bare text "Loading…" is not a loading state.
3. Empty renders `EmptyState` (`src/components/explore/EmptyState.tsx`).
   - "Empty" means the unfiltered collection is empty.
   - A filter that matches nothing says so, and offers to clear the filter.
4. A status indicator ("edited", "differs from live", "out of date") compares the content it describes. A timestamp is used only when the content is unavailable, and then every writer of that timestamp is listed in a comment.

### 4.3 Actions and feedback (FE-ACTION)
1. A destructive action is confirmed with `ConfirmDeleteDialog` (`src/components/ui/confirm-delete-dialog.tsx`) or `ui/alert-dialog`. Checked by lint (`egret/no-window-confirm`).
2. On success, a mutation invalidates the queries it changes.
   - If it navigates to the result, there is no toast. If it stays on the page, a toast confirms it (`ui-conventions.md` §3.6).
3. On failure, a mutation shows the server's message, in a toast or inline, and clears nothing the user entered (`ui-conventions.md` §3.7).
4. A disabled control says why it is disabled, in a tooltip or adjacent text.
5. A form with unsaved edits uses `useUnsavedChangesWarning` (`src/lib/useUnsavedChangesWarning.ts`).

### 4.4 Colour, type and spacing (FE-VISUAL)
1. **Colours come from tokens** (`src/index.css`): `bg-primary`, `text-muted-foreground`, `bg-highlight`, `text-success`, `text-destructive` and the others defined there.
   - No raw Tailwind palette class (`bg-green-600`).
   - No arbitrary colour (`bg-[#0af]`, `text-[oklch(...)]`).
   - Checked by lint (`egret/no-raw-palette`).
2. In an inline style, a token is `var(--token)`, never `hsl(var(--token))`. A token cannot cross into an SVG data URL, so resolve it first with `getComputedStyle`.
3. **Font sizes come from the scale:** `text-xs`, `text-sm`, `text-base`, `text-lg`, `text-xl` and `text-2xl`. No arbitrary `text-[Npx]`. Checked by lint (`egret/no-arbitrary-text-size`).
4. **Numbers and themes:**
   - A numeric column, count or measurement uses `tabular-nums`.
   - Every screen is correct in light and dark; a colour change is checked on both and against every surface it lands on.
   - Checked in the browser.
5. **SHOULD:** icons come from `lucide-react`, and a page has at most one `--highlight` emphasis. `--accent` is the hover/focus surface and is not used as an emphasis colour.
6. Charts keep the Metabase series palette (`src/components/chart/echarts/palette.ts`) and its colour-independent cues: line styles, markers and decals. Literal colours inside chart option builders are allowed; ECharts needs concrete values.

### 4.5 Forms (FE-FORM)
1. Forms hold their state in `useState` or a dedicated hook (as `useEngineConfigForm`). No form library is added.
2. Every input has a `Label` tied by `htmlFor`/`id`. Checked by lint (`jsx-a11y`).
3. When the set of valid values is known, the control is a picker or a combobox (pick or type) with help. It is never a bare text input.
4. A validation error shows beside its field, in the server's words when the server produced it.

### 4.6 Data and API (FE-DATA)
1. The API client is regenerated with `npm run gen:api`, never with bare `orval`, which drops path-parameter encoding.
2. **Endpoints and query keys:**
   - A new endpoint is consumed through its orval-generated hook.
   - A hand-written query key comes from a per-domain key factory (`queryKeys.ts` in the domain's folder, as `src/routes/control-plane/queryKeys.ts`), prefixed with the domain's name.
3. Server data is never cached in `localStorage`. `localStorage` holds per-viewer preferences only (theme, a collapsed panel), and every access is wrapped so the page works without it.

### 4.7 Routing (FE-ROUTE)
1. Routes are declared in `src/App.tsx`. A route is wrapped in the guard its area needs (`RequireFeature`, `RequirePermission`, `RequireGlobalAdmin`, from `src/auth/guards.tsx`). A redirect uses an absolute path.
2. Every new feature area, starting with Workflow, is code-split with `React.lazy` and `Suspense`, with a skeleton fallback. Checked by a bundle test like `test:bundle:monaco`.
3. URLs are kebab-case and workspace-scoped under `/explore/:workspace/…`.

### 4.8 Accessibility (FE-A11Y)
1. The code passes `eslint-plugin-jsx-a11y`'s recommended rules. Checked by lint.
2. Focus uses the shared ring (`focus-visible:ring-2 focus-visible:ring-ring`). A focus indicator meets 3:1 against every surface it lands on, which `src/__tests__/focusIndicatorContrast.test.ts` guards.
3. Every clickable element works from the keyboard. A non-button element made clickable gets a role, `tabIndex` and Enter/Space handling. An icon-only button has an `aria-label`.
4. Information is never carried by colour alone; charts keep their decals and markers (FE-VISUAL-6).
5. Motion respects `prefers-reduced-motion`.

### 4.9 Copy (FE-COPY)
1. UI text is English.
   - Titles, buttons and menu items are sentence case ("Add a chart").
   - A button starts with a verb.
   - The same thing has the same name everywhere.
2. An error message says what happened and what the user can do. A raw exception or a stack trace is never shown.
3. Text that names a control ("click Publish") is a claim that the control exists under that name, and a test checks it (`ui-conventions.md` §3.9).
4. Plurals are written out by the page (`SummaryStrip`'s `singular`), not built by appending "s" in shared code.

### 4.10 Reference products (FE-REF)
1. **The visual system is egret's.** Structure and interaction flow copy the area's source product:
   - BI (charts, dashboards, tables, settings panels): Metabase, including the settings panel's tab split and row shape;
   - Workflow: Kestra.
2. The comparison is made against a running instance of the source product, not from memory. Any deliberate difference is recorded in the package's spec with its reason.

### 4.11 Verification (FE-TEST)
1. **Tests:**
   - A test renders the real component and asserts through the DOM, preferring `getByRole`.
   - A test does not copy the expression it tests.
   - A fixture must be able to tell the right behaviour from a plausible wrong one; for example, it is not pre-sorted when order matters.
2. A change to layout, width, height, colour or any other painted result is verified in a real browser: Playwright, or measuring the DOM with `getBoundingClientRect` and `getComputedStyle`. jsdom has no layout, and reading code does not settle layout (`ui-conventions.md` §10).
3. A shared component changes additively. Its blast radius is measured by import path, not by symbol name (`ui-conventions.md` §7).
4. The gate for every frontend change:
   - `npm run lint`
   - `npx tsc -b`
   - `npm run test`
   - `npm run build`
   - the bundle tests the change touches

## 5. The reviewer checklist

The rulebook ends with a checklist of the rules that lint cannot check:
- FE-PAGE-1 to 5
- FE-STATE-1 to 4
- FE-ACTION-2 to 5
- FE-VISUAL-2 and 4 to 6
- FE-FORM-1, 3 and 4
- FE-DATA-1 to 3
- FE-ROUTE-1 and 3
- FE-A11Y-3 to 5
- FE-COPY-1, 2 and 4
- FE-REF-1 and 2
- FE-TEST-1 to 3

Every frontend task review answers each applicable item: pass, fail, or not applicable.

## 6. The debt register

The rulebook's last section lists every known place that breaks a rule. Each entry gives the rule id, the files and a count measured when the rulebook is written. The policy for every entry is the same: **fix it when you touch it**. A PR that edits a listed file brings that file into line, or says in its body why it cannot. The entries known now:

| Rule | Debt (survey of 2026-10-10; recounted in the plan's first task) |
|---|---|
| FE-PAGE-1 | control-plane and governance list pages hand-roll their frame (~49 route-level `<h1>`) |
| FE-PAGE-4 | ~9 hand-written `<table>` outside the specialised grids |
| FE-STATE-2 | ~28 "Loading…" texts; spinners used for content |
| FE-STATE-3 | ~53 hand-written "No X" empty texts, ~12 dashed blocks |
| FE-ACTION-1 | `window.confirm` in `DatasetDetail.tsx` and `DatabasesPage.tsx` |
| FE-VISUAL-1 | ~35 raw palette classes in ~12 files; the Button `ai` variant's raw gradient |
| FE-VISUAL-3 | ~33 arbitrary `text-[Npx]` |
| FE-DATA-2 | the `cp` and `control-plane` key prefixes for one domain |
| FE-ROUTE-2 | no route in `App.tsx` is lazy |
| FE-A11Y-1 | whatever `jsx-a11y` reports on its first run (counted in the plan) |
| docs | `docs/vi/ui-conventions.md` has drifted from the English file |

A lint-checked entry is the rule's legacy list (§7). A review-checked entry is a row in this table.

## 7. Lint

1. **A local plugin, `frontend/eslint-rules/`**, registered as `egret` in `eslint.config.js`. Each rule has `RuleTester` tests in vitest:
   - **`egret/no-raw-palette`** reports a string or template literal containing:
     - a Tailwind colour utility with a palette colour, `(bg|text|border|ring|outline|fill|stroke|from|via|to|divide|placeholder|caret|accent|decoration|shadow)-(slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-(50|[1-9]00|950)`, with an optional variant prefix and opacity suffix;
     - or an arbitrary colour value, `-[#…]`, `-[rgb(…)]`, `-[hsl(…)]` or `-[oklch(…)]`.
     It does not apply under `src/components/chart/echarts/` (FE-VISUAL-6).
   - **`egret/no-arbitrary-text-size`** reports `text-[<number><unit>]`.
   - **`egret/no-window-confirm`** reports `window.confirm(…)` and a bare `confirm(…)` that resolves to the global.
2. **`eslint-plugin-jsx-a11y-x`**, with its `recommended` config, at error. *(Amended 2026-10-10, plan: the maintained fork of `eslint-plugin-jsx-a11y`, with the same rules. Upstream 6.10.2, last published 2024, declares ESLint up to 9, and egret runs ESLint 10. The fork's `jsx-a11y` settings map egret's primitives (`Button`, `Input`, `Label`, `Textarea`) to their elements.)*
3. **Legacy lists.**
   - Each rule has its own list of files that broke it when it was introduced: `frontend/eslint-rules/legacy.js`, one entry per rule id, `egret/*` and `jsx-a11y/*` alike.
   - `eslint.config.js` turns that one rule off for exactly those files.
   - A file is never added to a list after the list is created.
4. **The lists stay honest.** A script, `npm run lint:legacy`, runs each rule with no legacy list over its listed files. It fails when a listed file no longer breaks its rule, so a fixed file must leave its list. It runs in the gate: `npm run lint` runs ESLint and then this script (FE-TEST-4).
5. All of these rules are errors. The React Compiler rules that `eslint.config.js` sets to warn stay as they are; they are outside this package.

## 8. Testing

- **`RuleTester` cases for each egret rule:**
  - valid: token classes, chart builder literals in the exempt path, `text-sm`, `confirm` as a local identifier;
  - invalid: each pattern of §7.1, including a variant prefix (`dark:bg-red-500`), an opacity suffix (`bg-sky-500/20`), and a template literal.
- **`lint:legacy`:** a test for the script itself, with a fixture file that violates and one that does not.
- **`npm run lint` is green on the whole tree** with the legacy lists in place, and red when a violation is added to a file not on a list. The plan checks this by temporarily adding one violation to a file outside the lists.
- **The pointer files:** a reader in `frontend/` reaches the rulebook in one hop; the plan checks each link path exists.
- **The deletion of `egret-theme.ts`:** `npm run build` and `tsc` are green.

## 9. Out of scope

- Migrating existing code to the rules, beyond deleting `egret-theme.ts`. Debt is paid when a file is touched (§6).
- i18n, prettier, and the React Compiler warning backlog.
- A data-router migration (needed for in-app navigation blocking).
- Rewriting `ui-conventions.md` or re-syncing its Vietnamese twin.
- Visual regression (screenshot diff) tooling.
