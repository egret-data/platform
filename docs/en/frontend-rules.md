# egret frontend rules

The rules every change under `frontend/` follows. This file is normative; [`ui-conventions.md`](ui-conventions.md) holds the reasons behind them and the record of reversed decisions.

**How to read it.**
- Each rule is **MUST** or **SHOULD**. Breaking a SHOULD needs a one-line reason in the PR.
- Rule ids (`FE-<GROUP>-<n>`) are stable forever and never reused once retired.
- **Checked by** says what enforces a rule: `lint` rules fail `npm run lint`; `test` and `browser` rules are verified that way; `review` rules are answered in the [reviewer checklist](#reviewer-checklist).
- A rule that names a component names the file it lives in. If the component moves, this file changes in the same PR.
- Code that broke a rule before the rule existed is listed in the [debt register](#debt-register).

## 1. Page frame (FE-PAGE)

#### FE-PAGE-1 — A list page uses `ListPageShell` · MUST
A list page uses `ListPageShell`, and that shell owns the page's single `h1`. Order: title, then the toolbar (`ListToolbarButton`), then `SummaryStrip`, then the table.
**Do this:** `frontend/src/components/explore/ListPageShell.tsx`, `frontend/src/components/explore/ListToolbarButton.tsx`, `frontend/src/components/explore/SummaryStrip.tsx`. **Checked by:** review. **Why:** [ui-conventions §1](ui-conventions.md#1-the-list-page).

#### FE-PAGE-2 — Detail sections are tabs, and shareable state lives in the URL · MUST
A detail page with sections uses `ui/tabs`. The selected tab, and any filter or selection a user would want to share, live in the URL (`?tab=`, `?q=`). The Back button walks them.
**Do this:** `frontend/src/components/ui/tabs.tsx`. **Checked by:** review.

#### FE-PAGE-3 — Create on a page or in a dialog by size · MUST
- Create on a page (`…/new`, declared before `:id`) when the form has several sections, or when its result must be read before leaving (a one-time secret).
- Create in a dialog when it has one to three fields.
- Entry points are links, not imperative pushes.

**Do this:** a page: `frontend/src/routes/explore/dataapi/EndpointCreate.tsx` (declared as `data-api/endpoints/new` in `frontend/src/App.tsx`); a dialog: `frontend/src/components/explore/FolderNameDialog.tsx`. **Checked by:** review. **Why:** [ui-conventions §3](ui-conventions.md#3-create-flows-when-a-page-when-a-dialog).

#### FE-PAGE-4 — Tabular lists use the shared tables · MUST
A tabular list uses `FilterableTable` or `ui/table`. A hand-written `<table>` is allowed only for a specialised grid (pivot, SQL results), and carries a comment saying why.
**Do this:** `frontend/src/components/explore/FilterableTable.tsx`, `frontend/src/components/ui/table.tsx`. **Checked by:** review.

#### FE-PAGE-5 — Settings are `Setting | Details | Value` tables · MUST
Settings use `ConfigTable`/`ConfigTableRow`/`ConfigSection`, in the `Setting | Details | Value` shape. A flat property map is one searchable key/value list, not a bespoke form per field.
**Do this:** `frontend/src/components/config/ConfigTable.tsx` (`ConfigTable`, `ConfigTableRow`), `frontend/src/components/form/ConfigSection.tsx`. **Checked by:** review. **Why:** [ui-conventions §4](ui-conventions.md#4-setting--details--value-tables-and-when-a-card-is-still-right).

## 2. States (FE-STATE)

#### FE-STATE-1 — Loading, empty and error, error first · MUST
Every data view renders loading, empty and error, and checks `isError` before it checks for data. An error never renders as empty.
**Do this:** `ListStates` (`frontend/src/components/explore/ListStates.tsx`), which branches loading, error, empty, data in that order. **Checked by:** review. **Why:** [ui-conventions §2](ui-conventions.md#2-loading-empty-error--three-states-never-two).

#### FE-STATE-2 — Loading content is a skeleton · MUST
Loading page or list content renders a skeleton. A spinner (`Loader2`) is only for a button or an in-place action. The bare text "Loading…" is not a loading state.
**Do this:** `ListStates` (`frontend/src/components/explore/ListStates.tsx`), `ListSkeleton` (`frontend/src/components/explore/ListSkeleton.tsx`), `frontend/src/components/ui/skeleton.tsx`. **Checked by:** review.

#### FE-STATE-3 — Empty is `EmptyState`, and a filter miss says so · MUST
Empty renders `EmptyState`. "Empty" means the unfiltered collection is empty. A filter that matches nothing says so, and offers to clear the filter.
**Do this:** `frontend/src/components/explore/EmptyState.tsx`. **Checked by:** review. **Why:** [ui-conventions §2](ui-conventions.md#2-loading-empty-error--three-states-never-two).

#### FE-STATE-4 — Status indicators compare content · MUST
A status indicator ("edited", "differs from live", "out of date") compares the content it describes. A timestamp is used only when the content is unavailable, and then every writer of that timestamp is listed in a comment.
**Do this:** example: `frontend/src/routes/explore/dataapi/endpointState.tsx` (compares the draft with the served definition). **Checked by:** review.

## 3. Actions and feedback (FE-ACTION)

#### FE-ACTION-1 — Destructive actions use a confirm dialog · MUST
A destructive action is confirmed with `ConfirmDeleteDialog` or `ui/alert-dialog`. Never `window.confirm`.
**Do this:** `frontend/src/components/ui/confirm-delete-dialog.tsx`, `frontend/src/components/ui/alert-dialog.tsx`. **Checked by:** lint (`egret/no-window-confirm`).

#### FE-ACTION-2 — Success invalidates; toast only when staying · MUST
On success, a mutation invalidates the queries it changes. If it navigates to the result, there is no toast. If it stays on the page, a toast confirms it.
**Do this:** example: `frontend/src/routes/explore/dataapi/EndpointCreate.tsx` (invalidates, navigates, no toast); `frontend/src/routes/explore/dataapi/detail/AccessSection.tsx` (invalidates, stays, toasts). **Checked by:** review. **Why:** [ui-conventions §3.6](ui-conventions.md#36-on-success-invalidate-and-no-toast--navigate-unless-the-response-is-the-point).

#### FE-ACTION-3 — Failure shows the server's message and clears nothing · MUST
On failure, a mutation shows the server's message, in a toast or inline, and clears nothing the user entered.
**Do this:** example: `frontend/src/routes/explore/dataapi/EndpointCreate.tsx` (the server's message inline, every field kept). **Checked by:** review. **Why:** [ui-conventions §3.7](ui-conventions.md#37-on-failure-clear-nothing).

#### FE-ACTION-4 — A disabled control says why · MUST
A disabled control says why it is disabled, in a tooltip or adjacent text.
**Do this:** `frontend/src/components/ui/tooltip.tsx`. **Checked by:** review.

#### FE-ACTION-5 — Unsaved edits warn · MUST
A form with unsaved edits uses `useUnsavedChangesWarning`.
**Do this:** `frontend/src/lib/useUnsavedChangesWarning.ts`. **Checked by:** review.

## 4. Colour, type and spacing (FE-VISUAL)

#### FE-VISUAL-1 — Colours come from tokens · MUST
Use the token classes defined in `frontend/src/index.css` (`bg-primary`, `text-muted-foreground`, `bg-highlight`, `text-success`, `text-destructive` and the others defined there). No raw Tailwind palette class (`bg-green-600`). No arbitrary colour (`bg-[#0af]`, `text-[oklch(...)]`).
**Do this:** `className="bg-primary text-primary-foreground"`, with the tokens in `frontend/src/index.css`. **Checked by:** lint (`egret/no-raw-palette`; not applied under `frontend/src/components/chart/echarts/`, see FE-VISUAL-6).

#### FE-VISUAL-2 — Inline styles use `var(--token)` · MUST
In an inline style, a token is `var(--token)`, never `hsl(var(--token))`. A token cannot cross into an SVG data URL, so resolve it first with `getComputedStyle`.
**Do this:** `style={{ color: "var(--primary)" }}`, as in `frontend/src/components/access/EffectBadge.tsx`; for an SVG data URL, `frontend/src/components/chart/dashboard/grid-background.ts`. **Checked by:** review.

#### FE-VISUAL-3 — Font sizes come from the scale · MUST
Use `text-xs`, `text-sm`, `text-base`, `text-lg`, `text-xl` and `text-2xl`. No arbitrary `text-[Npx]`.
**Do this:** `className="text-sm"`; lint's definition is `frontend/eslint-rules/rules/no-arbitrary-text-size.js`. **Checked by:** lint (`egret/no-arbitrary-text-size`).

#### FE-VISUAL-4 — Tabular numbers; correct in light and dark · MUST
A numeric column, count or measurement uses `tabular-nums`. Every screen is correct in light and dark; a colour change is checked on both and against every surface it lands on.
**Do this:** example: `frontend/src/components/chart/pivot/PivotTable.tsx` (`tabular-nums` on numeric cells). **Checked by:** browser.

#### FE-VISUAL-5 — Lucide icons; one highlight per page · SHOULD
Icons come from `lucide-react`, and a page has at most one `--highlight` emphasis. `--accent` is the hover/focus surface and is not used as an emphasis colour.
**Do this:** icons as in `frontend/src/components/explore/RowActionsMenu.tsx`; `--highlight` and `--accent` are defined in `frontend/src/index.css`. **Checked by:** review.

#### FE-VISUAL-6 — Charts keep the Metabase palette and its cues · MUST
Charts keep the Metabase series palette and its colour-independent cues: line styles and marker shapes per series, and the `aria` data description. Decal patterns stay off, as decided in chart 05 (#360); `palette.ts` records why. Literal colours inside chart option builders are allowed; ECharts needs concrete values.
**Do this:** `frontend/src/components/chart/echarts/palette.ts`. **Checked by:** review.

## 5. Forms (FE-FORM)

#### FE-FORM-1 — Hand-written form state · MUST
Forms hold their state in `useState` or a dedicated hook. No form library is added.
**Do this:** `frontend/src/routes/control-plane/useEngineConfigForm.ts`. **Checked by:** review.

#### FE-FORM-2 — Every input has a tied `Label` · MUST
Every input has a `Label` tied by `htmlFor`/`id`.
**Do this:** `<Label htmlFor="name">Name</Label><Input id="name" />` (`frontend/src/components/ui/label.tsx`). **Checked by:** lint (the label side: `jsx-a11y/label-has-associated-control` requires a `Label` to have `htmlFor` or a nested control) and review (every input has a label whose `htmlFor` matches its `id`; lint misses an unlabelled input and a mismatched id).

#### FE-FORM-3 — Known values get a picker · MUST
When the set of valid values is known, the control is a picker or a combobox (pick or type) with help. It is never a bare text input.
**Do this:** `frontend/src/components/ui/select.tsx`, `frontend/src/components/ui/tag-combobox.tsx`; example: `frontend/src/routes/control-plane/config/PropertyPicker.tsx`. **Checked by:** review.

#### FE-FORM-4 — Errors sit beside their field · MUST
A validation error shows beside its field, in the server's words when the server produced it.
**Do this:** example: `frontend/src/routes/control-plane/config/QuantityInput.tsx` (the error directly under its input). **Checked by:** review.

## 6. Data and API (FE-DATA)

#### FE-DATA-1 — Regenerate with `npm run gen:api` · MUST
The API client is regenerated with `npm run gen:api`, never with bare `orval`, which drops path-parameter encoding.
**Do this:** `npm run gen:api` (`frontend/package.json`), which runs `orval` and then `frontend/scripts/encode-path-params.mjs`. **Checked by:** review.

#### FE-DATA-2 — Orval hooks and a key factory per domain · MUST
A new endpoint is consumed through its orval-generated hook. A hand-written query key comes from a per-domain key factory (`queryKeys.ts` in the domain's folder), prefixed with the domain's name.
- **Exception.** A generated orval fetcher (never a hand-written `fetch`) may be called directly when the call needs a cursor across pages, an abort signal per request, or parameters computed at fetch time. Each such call site carries a one-line comment naming its reason.

**Do this:** `frontend/src/routes/control-plane/queryKeys.ts`; an exception: `frontend/src/components/workflows/executions/logs/useLogFollow.ts` (a cursor across pages). **Checked by:** review.

#### FE-DATA-3 — `localStorage` holds preferences only · MUST
Server data is never cached in `localStorage`. `localStorage` holds per-viewer preferences only (theme, a collapsed panel), and every access is wrapped so the page works without it.
**Do this:** example: `frontend/src/components/explore/folders.ts` (a collapse state; every read in `try`). **Checked by:** review.

## 7. Routing (FE-ROUTE)

#### FE-ROUTE-1 — Routes live in `App.tsx`, behind their guard · MUST
Routes are declared in `frontend/src/App.tsx`, or in a lazy area module that `App.tsx` mounts (the area's sub-routes must live there, or `App.tsx` would import its pages eagerly; for example `frontend/src/routes/workflows/WorkflowsRoutes.tsx`). A route is wrapped in the guard its area needs (`RequireFeature`, `RequirePermission`, `RequireGlobalAdmin`). A redirect uses an absolute path.
**Do this:** `frontend/src/auth/guards.tsx`. **Checked by:** review.

#### FE-ROUTE-2 — New feature areas are lazy · MUST
Every new feature area, starting with Workflow, is code-split with `React.lazy` and `Suspense`, with a skeleton fallback.
**Do this:** a route area: `workflows/*` in `frontend/src/App.tsx` (`lazy(() => import("@/routes/workflows/WorkflowsRoutes"))` inside `LazyRouteArea`, `frontend/src/components/layout/LazyRouteArea.tsx`: `Suspense` with a skeleton fallback, under an error boundary so a chunk that fails to load shows a reload instead of a blank app); a component: `frontend/src/components/monaco/MonacoEditor.tsx`. **Checked by:** test: a bundle test per lazy area, `npm run test:bundle:workflows` (`frontend/scripts/assert-workflows-lazy.mjs`) for the Workflows modules it lists, one marker per module, `npm run test:bundle:governance-data` (`frontend/scripts/assert-governance-data-lazy.mjs`) for the Governance Data area, and `npm run test:bundle:monaco` (`frontend/scripts/assert-monaco-lazy.mjs`) for Monaco.

#### FE-ROUTE-3 — Kebab-case, workspace-scoped URLs · MUST
URLs are kebab-case and workspace-scoped under `/explore/:workspace/…`.
**Do this:** `frontend/src/App.tsx` (for example `/explore/:workspace/data-api/endpoints/new`). **Checked by:** review.

## 8. Accessibility (FE-A11Y)

#### FE-A11Y-1 — Pass the jsx-a11y recommended rules · MUST
The code passes the recommended rules of `eslint-plugin-jsx-a11y-x` (the maintained fork of `eslint-plugin-jsx-a11y`; rule ids read `jsx-a11y/…`). `jsx-a11y/no-autofocus` is off; see FE-A11Y-6. `jsx-a11y/no-redundant-roles` allows `role="list"` on `<ul>` and `<ol>`: Safari with VoiceOver drops a list's role once CSS removes its list styling, as the Tailwind reset does. `jsx-a11y/no-noninteractive-tabindex` also allows `tabIndex` on `role="log"`, for FE-A11Y-3's scrollable log. `jsx-a11y/no-noninteractive-element-to-interactive-role` also allows `role="treegrid"` on `<table>`: a treegrid is a table whose rows form a tree, as the namespaces list is.
**Do this:** fix what lint reports; the configuration is `frontend/eslint-rules/plugins.js`. **Checked by:** lint (jsx-a11y).

#### FE-A11Y-2 — The shared focus ring, at 3:1 · MUST
Focus uses the shared ring (`focus-visible:ring-2 focus-visible:ring-ring`). A focus indicator meets 3:1 against every surface it lands on.
**Do this:** the ring as in `frontend/src/components/ui/button.tsx`. **Checked by:** test (`frontend/src/__tests__/focusIndicatorContrast.test.ts`).

#### FE-A11Y-3 — Everything clickable works from the keyboard · MUST
Every clickable element works from the keyboard. A non-button element made clickable gets a role, `tabIndex` and Enter/Space handling. An icon-only button has an `aria-label`. A scrollable `role="log"` region takes `tabIndex={0}`, so the keyboard can scroll it (axe's `scrollable-region-focusable`); a windowed list keeps the row holding focus rendered.
**Do this:** `frontend/src/components/ui/icon-button.tsx` for an icon-only button; `frontend/src/components/workflows/executions/logs/LogView.tsx` for a scrollable log. **Checked by:** review.

#### FE-A11Y-4 — Never colour alone · MUST
Information is never carried by colour alone; charts keep their line styles, markers and `aria` description (FE-VISUAL-6).
**Do this:** `frontend/src/components/chart/echarts/palette.ts` (marker shapes and line styles per series, and the `aria` description). **Checked by:** review.

#### FE-A11Y-5 — Respect reduced motion · MUST
Motion respects `prefers-reduced-motion`.
**Do this:** `frontend/src/index.css` (its `prefers-reduced-motion: reduce` block). **Checked by:** review.

#### FE-A11Y-6 — `autoFocus` only after the user's own action · MUST
`autoFocus` is only for a field that appears because of the user's own action (a dialog's first field, an inline edit), never on page load or route change.
**Do this:** example: `frontend/src/components/ui/confirm-delete-dialog.tsx` (focus on the dialog the user opened). **Checked by:** review. **Why:** `jsx-a11y/no-autofocus` is off (the user's ruling, 2026-10-10), because it cannot tell the two apart.

## 9. Copy (FE-COPY)

#### FE-COPY-1 — English, sentence case, verbs, one name per thing · MUST
UI text is English. Titles, buttons and menu items are sentence case ("Add a chart"). A button starts with a verb. The same thing has the same name everywhere.
**Do this:** example: `frontend/src/routes/explore/dataapi/EndpointCreate.tsx` ("New endpoint", "Create endpoint"). **Checked by:** review.

#### FE-COPY-2 — Errors say what happened and what to do · MUST
An error message says what happened and what the user can do. A raw exception or a stack trace is never shown.
**Do this:** example: `frontend/src/components/monaco/MonacoEditor.tsx` ("The code editor could not be loaded. Reload the page to try again."). **Checked by:** review.

#### FE-COPY-3 — Copy that names a control is tested · MUST
Text that names a control ("click Publish") is a claim that the control exists under that name, and a test checks it.
**Do this:** example: `frontend/src/components/layout/__tests__/Sidebar.test.tsx`. **Checked by:** test. **Why:** [ui-conventions §3.9](ui-conventions.md#39-copy-that-names-a-control-is-a-claim-about-the-tree-and-is-checked-like-one).

#### FE-COPY-4 — Pages write their own plurals · MUST
Plurals are written out by the page (`SummaryStrip`'s `singular`), not built by appending "s" in shared code.
**Do this:** `frontend/src/components/explore/SummaryStrip.tsx`. **Checked by:** review.

## 10. Reference products (FE-REF)

#### FE-REF-1 — egret's visuals, the source product's flow · MUST
The visual system is egret's. Structure and interaction flow copy the area's source product:
- BI (charts, dashboards, tables, settings panels): Metabase, including the settings panel's tab split and row shape;
- Workflow: Kestra.

**Do this:** example: `frontend/src/components/chart/settings/` (the Metabase-shaped settings panel). **Checked by:** review.

#### FE-REF-2 — Compare against a running instance · MUST
The comparison is made against a running instance of the source product, not from memory. Any deliberate difference is recorded in the package's spec with its reason.
**Do this:** `scripts/harness/` seeds the Metabase parity dashboards; record differences in the package's spec under `docs/en/specs/`. **Checked by:** review.

## 11. Verification (FE-TEST)

#### FE-TEST-1 — Tests render the real thing and can fail · MUST
- A test renders the real component and asserts through the DOM, preferring `getByRole`.
- A test does not copy the expression it tests.
- A fixture must be able to tell the right behaviour from a plausible wrong one; for example, it is not pre-sorted when order matters.

**Do this:** example: `frontend/src/components/explore/__tests__/ListStates.test.tsx`. **Checked by:** review. **Why:** [ui-conventions §8](ui-conventions.md#8-an-assertion-that-a-hardcoded-implementation-would-also-pass-pins-nothing).

#### FE-TEST-2 — Painted results are verified in a browser · MUST
A change to layout, width, height, colour or any other painted result is verified in a real browser: Playwright, or measuring the DOM with `getBoundingClientRect` and `getComputedStyle`. jsdom has no layout, and reading code does not settle layout.
**Do this:** Playwright specs in `frontend/tests/e2e/` (`npm run test:e2e`); example: `frontend/tests/e2e/data-api-detail-tab-widths.spec.ts`. **Checked by:** browser. **Why:** [ui-conventions §10](ui-conventions.md#10-reading-the-code-does-not-settle-layout).

#### FE-TEST-3 — Shared components change additively · MUST
A shared component changes additively. Its blast radius is measured by import path, not by symbol name.
**Do this:** no single file; follow `docs/en/ui-conventions.md` §7. **Checked by:** review. **Why:** [ui-conventions §7](ui-conventions.md#7-changing-a-shared-component-additively-and-measure-by-import-path).

#### FE-TEST-4 — The gate · MUST
Every frontend change passes, from `frontend/`:
- `npm run lint` (ESLint, then `npm run lint:legacy`, `frontend/scripts/lint-legacy.mjs`)
- `npx tsc -b`
- `npm run test`
- `npm run build`
- the bundle tests the change touches (`npm run test:bundle`, `test:bundle:xlsx`, `test:bundle:csp`, `test:bundle:monaco`, `test:bundle:workflows`, `test:bundle:governance-data`)

**Do this:** the scripts in `frontend/package.json`. **Checked by:** review: the reviewer confirms the gate ran.

## Reviewer checklist

Every rule a reviewer must confirm is here: either lint cannot check it, or its check is a test or the gate that the change must add or run. Every frontend task review answers each applicable item: **pass**, **fail**, or **not applicable**. For a rule checked by a test, the reviewer also asks: does the test this rule needs exist, and does it cover this change?

- [ ] FE-PAGE-1 — A list page uses `ListPageShell`, which owns the single `h1`; title, toolbar, `SummaryStrip`, table.
- [ ] FE-PAGE-2 — Detail sections use `ui/tabs`; the tab, and any filter or selection a user would want to share, live in the URL.
- [ ] FE-PAGE-3 — Create on a page when several sections or a read-once result, in a dialog for one to three fields; entry points are links.
- [ ] FE-PAGE-4 — Tabular lists use `FilterableTable` or `ui/table`; a hand-written `<table>` is a specialised grid with a comment.
- [ ] FE-PAGE-5 — Settings use `ConfigTable`/`ConfigTableRow`/`ConfigSection` in `Setting | Details | Value` shape.
- [ ] FE-STATE-1 — Loading, empty and error all render; `isError` is checked first; error never renders as empty.
- [ ] FE-STATE-2 — Loading content is a skeleton; spinners only for buttons and in-place actions; no bare "Loading…".
- [ ] FE-STATE-3 — Empty is `EmptyState` for the unfiltered collection; a filter miss says so and offers to clear.
- [ ] FE-STATE-4 — Status indicators compare content; a timestamp fallback lists its writers in a comment.
- [ ] FE-ACTION-2 — Success invalidates the changed queries; no toast when navigating, a toast when staying.
- [ ] FE-ACTION-3 — Failure shows the server's message and clears nothing the user entered.
- [ ] FE-ACTION-4 — A disabled control says why.
- [ ] FE-ACTION-5 — A form with unsaved edits uses `useUnsavedChangesWarning`.
- [ ] FE-VISUAL-2 — Inline styles use `var(--token)`; SVG data URLs get a resolved value.
- [ ] FE-VISUAL-4 — Numbers use `tabular-nums`; colour changes checked in light and dark on every surface.
- [ ] FE-VISUAL-5 — (SHOULD) Icons from `lucide-react`; at most one `--highlight` per page; `--accent` is not emphasis.
- [ ] FE-VISUAL-6 — Charts keep the Metabase palette, line styles, marker shapes and the `aria` description; decals stay off.
- [ ] FE-FORM-1 — Form state in `useState` or a dedicated hook; no form library.
- [ ] FE-FORM-2 — Every input has a `Label` whose `htmlFor` matches its `id` (lint checks only the label side).
- [ ] FE-FORM-3 — Known value sets get a picker or combobox with help.
- [ ] FE-FORM-4 — Validation errors sit beside their field, in the server's words when the server produced it.
- [ ] FE-DATA-1 — The API client was regenerated with `npm run gen:api`.
- [ ] FE-DATA-2 — New endpoints through orval hooks, or a generated fetcher with a one-line comment naming the exception; hand-written keys from the domain's key factory.
- [ ] FE-DATA-3 — No server data in `localStorage`; every access wrapped.
- [ ] FE-ROUTE-1 — Routes in `App.tsx` or in a lazy area module it mounts, behind their area's guard; redirects are absolute.
- [ ] FE-ROUTE-2 — A new feature area is lazy with a skeleton fallback, and a bundle test guards it (W7 adds the first).
- [ ] FE-ROUTE-3 — URLs are kebab-case under `/explore/:workspace/…`.
- [ ] FE-A11Y-2 — Focus uses the shared ring, at 3:1 against every surface it lands on; a new surface token is added to `focusIndicatorContrast.test.ts`.
- [ ] FE-A11Y-3 — Everything clickable works from the keyboard; icon-only buttons have an `aria-label`; a scrollable `role="log"` has `tabIndex={0}`.
- [ ] FE-A11Y-4 — No information carried by colour alone.
- [ ] FE-A11Y-5 — Motion respects `prefers-reduced-motion`.
- [ ] FE-A11Y-6 — `autoFocus` only on a field the user's own action revealed.
- [ ] FE-COPY-1 — English, sentence case, buttons start with a verb, one name per thing.
- [ ] FE-COPY-2 — Errors say what happened and what to do; no raw exception or stack trace.
- [ ] FE-COPY-3 — Copy that names a control has a test that the control exists under that name.
- [ ] FE-COPY-4 — Pages write their own plurals.
- [ ] FE-REF-1 — egret's visual system; the source product's structure and flow (Metabase for BI, Kestra for Workflow).
- [ ] FE-REF-2 — Compared against a running instance; deliberate differences recorded in the spec.
- [ ] FE-TEST-1 — Tests render the real component, assert through the DOM, do not copy the expression, and use fixtures that can fail.
- [ ] FE-TEST-2 — Painted results verified in a real browser.
- [ ] FE-TEST-3 — Shared components changed additively; blast radius measured by import path.
- [ ] FE-TEST-4 — The gate ran: `npm run lint`, `npx tsc -b`, `npm run test`, `npm run build`, and the bundle tests the change touches.

## Debt register

**Policy: fix it when you touch it, and the lists only shrink.**
- A PR that edits a listed file brings that file into line with the rule, or says in its body why it cannot.
- For a lint rule, fixing a file means removing it from `frontend/eslint-rules/legacy.js` in the same commit. `npm run lint:legacy` fails while a fixed file is still listed.
- A file is never added to a legacy list. An `eslint-disable` comment is not a way around a rule.

### Lint-checked debt

Each row is a legacy list in `frontend/eslint-rules/legacy.js`: the files that broke the rule when it was introduced. `eslint.config.js` turns the rule off for exactly those files.

| Rule | Lint rule | Files | Note |
|---|---|---|---|
| FE-VISUAL-1 | `egret/no-raw-palette` | 12 | includes the Button `ai` variant's raw gradient (`frontend/src/components/ui/button.tsx`) |
| FE-VISUAL-3 | `egret/no-arbitrary-text-size` | 16 | |
| FE-ACTION-1 | `egret/no-window-confirm` | 2 | `DatasetDetail.tsx`, `DatabasesPage.tsx` |
| FE-A11Y-1 | `jsx-a11y/aria-role` | 3 | |
| FE-A11Y-1 | `jsx-a11y/click-events-have-key-events` | 4 | |
| FE-A11Y-1 | `jsx-a11y/heading-has-content` | 2 | |
| FE-A11Y-1, FE-FORM-2 | `jsx-a11y/label-has-associated-control` | 35 | |
| FE-A11Y-1 | `jsx-a11y/no-noninteractive-element-interactions` | 3 | |
| FE-A11Y-1 | `jsx-a11y/no-noninteractive-tabindex` | 2 | |
| FE-A11Y-1 | `jsx-a11y/no-static-element-interactions` | 1 | |
| FE-A11Y-1 | `jsx-a11y/role-has-required-aria-props` | 3 | |

### Review-checked debt

Counted on 2026-10-10. These are **grep counts, not an inventory**: each is a rough instrument that over- or under-counts, and its command is given so it can be rerun. Run the commands from `frontend/`; they work in bash and zsh.

| Rule | Debt | Grep count |
|---|---|---|
| FE-PAGE-1 | control-plane and governance list pages hand-roll their frame | (1) 42 route files with their own `<h1>`; (2) 42 of them do not mention `ListPageShell` |
| FE-PAGE-4 | hand-written `<table>` outside the shared tables (some are allowed specialised grids) | (3) 17 lines |
| FE-STATE-2 | "Loading…" texts; spinners used for content (not counted) | (4) 28 lines |
| FE-STATE-3 | hand-written "No X" empty texts, and dashed empty blocks | (5) 6 lines of the form `"No <word> yet\|found`; (6) 12 `border-dashed` lines |
| FE-DATA-2 | the `cp` and `control-plane` key prefixes for one domain | (7) 30 `["cp"` lines; (8) 52 `["control-plane"` lines |
| FE-ROUTE-2 | only the Workflows area in `App.tsx` is lazy; the areas before it are eager | (9) 1 `lazy(` in `App.tsx` (Workflows, W7a) |
| docs | `docs/vi/ui-conventions.md` has drifted from `docs/en/ui-conventions.md` | (10) 664 lines against 2,227 |

```bash
grep -rln "<h1" src/routes --include='*.tsx' | grep -v __tests__ | wc -l                                       # (1)
grep -rLn "ListPageShell" $(grep -rln "<h1" src/routes --include='*.tsx' | grep -v __tests__) | wc -l           # (2)
grep -rn "<table" src --include='*.tsx' | grep -v __tests__ | grep -v "components/ui/table" | grep -v "\.tsx:[0-9]*: *\({/\*\|//\|\*\)" | wc -l   # (3)
grep -rn "Loading…\|Loading\.\.\." src --include='*.tsx' | grep -v __tests__ | wc -l                            # (4)
grep -rn "\"No [a-z]\+ \(yet\|found\)" src --include='*.tsx' | grep -v __tests__ | grep -v "title=\|\.tsx:[0-9]*: *\({/\*\|//\|\*\)" | wc -l   # (5)
grep -rn "border-dashed" src --include='*.tsx' | grep -v __tests__ | wc -l                                      # (6)
grep -rn "\[\"cp\"\|\['cp'" src --include='*.ts*' | grep -v __tests__ | wc -l                                  # (7)
grep -rn "\[\"control-plane\"" src --include='*.ts*' | grep -v __tests__ | wc -l                                # (8)
grep -c "lazy(" src/App.tsx                                                                                    # (9)
wc -l ../docs/vi/ui-conventions.md ../docs/en/ui-conventions.md                                               # (10)
```

**Narrowed greps (2026-10-10, W7a final review).** Greps (3) and (5) used to count compliant code, so W7a's compliant additions read as new debt (FE-PAGE-4 from 19 to 21, FE-STATE-3 from 20 to 22). They now skip comment lines, and (5) skips `title=` lines, which are `EmptyState` titles (the compliant form). On the same narrowed greps the counts before W7a were 16 and 6. The one W7a line still counted by (3) is compliant: the namespace-grouped `<table>` in `src/routes/workflows/FlowsList.tsx`, an allowed specialised grid with its FE-PAGE-4 comment.
