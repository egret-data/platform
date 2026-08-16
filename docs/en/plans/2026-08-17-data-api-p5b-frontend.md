# Data API P5b — Frontend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Data API management surface — the area, its three sections, and the endpoint detail page — so an author can create, publish, grant, try and observe an endpoint without leaving the product.

**Architecture:** One shell route under `/explore/:ws/data-api` using the Workspace-Settings section-nav idiom, with nested routes for Endpoints / Clients / Activity and a separate detail route per endpoint. Every server call goes through the orval-generated client, which inherits auth cookies and the CSRF header from `apiFetch`. No new backend work: P5a (PR #510) shipped every route this consumes.

**Tech Stack:** React 18, TypeScript, react-router-dom (`<BrowserRouter>`), TanStack Query via orval-generated hooks, shadcn/Radix, Tailwind v4 with OKLCH tokens, vitest + Testing Library.

Implements `docs/en/specs/2026-08-16-data-api-p5-design.md` §2, §3 (UI half), §4 (UI half), §5.6, §6.2, §6.3, §6.4 — PRs 3–7 of that spec's §9. The backend half is `docs/en/plans/2026-08-16-data-api-p5a-backend.md`.

## Global Constraints

- **This branch must start from P5a.** Branch off `feat/data-api-p5a` (or `main` once #510 merges). The routes this plan consumes — `/activity/summary`, `/activity/calls`, `/openapi`, `/try` — do not exist before it.
- **Regenerate the API client first.** `frontend/src/api/generated/data-api/data-api.ts` predates P5a and has no hooks for the four new routes. Run `npm run gen:api` (which is `orval && node scripts/encode-path-params.mjs`) after the backend is in. Do not hand-write a client module.
- **Never hand-roll `fetch` for a mutating call.** `api/client.ts:41-44` attaches `X-Egret-CSRF` on POST/PUT/PATCH/DELETE, and `api/mutator.ts` routes the generated client through it. `POST …/try` is a mutating call inside the CSRF-layered management group; a bespoke `fetch` would 403 in production while every test passed. This exact trap killed a production POST in the Alerts programme.
- **`apiFetch` throws on non-2xx**, so generated hooks surface failures as react-query `error` (an `ApiError` carrying `.status`), and a successful response arrives as `{ status, data, headers }` — read `data?.data`.
- **Explore idiom, not control-plane idiom.** List pages are `p-4` (not `p-6`), `<h1 className="text-lg font-semibold">` with no subtitle paragraph, and a default-size `+ Endpoint` button with no icon. The control-plane idiom (`space-y-4`, `text-2xl` + subtitle, `size="sm"` + `<Plus/>`) is a different section; copying it is the mismatch users have objected to before.
- **Endpoints list hand-rolls its `<table>` like Charts/Datasets — do NOT use `FilterableTable`.** It cannot model folder grouping, and its styling visibly differs (bold dark headers vs muted, `border-b` vs `border-t`, `hover:bg-muted/50` vs `hover:bg-accent`). Clients IS a plain list, so `FilterableTable` + `SummaryStrip` is right there, as `DatabasesPage` does.
- **`FilterableTable` renders `emptyText` whenever the row set is empty**, so an external facet filtering to zero would show the first-run CTA. Pass a plain string in that case.
- **A route obliges `useUnsavedChangesWarning`** (`frontend/src/lib/useUnsavedChangesWarning.ts`, signature `(when: boolean) => void`). Dirtiness must be a **comparison against the loaded value**, never an `onChange` flag — an `onChange` flag plus Radix hydration raises a phantom "Leave site?" on a page nobody edited.
- **The app uses `<BrowserRouter>`, not a data router, so `useBlocker` is unavailable.** `useUnsavedChangesWarning` covers refresh and close only; in-app navigation cannot be blocked. Do not attempt a data-router migration here.
- **OKLCH tokens: inline styles use `var(--token)` directly, never `hsl(var(--token))`.** `--highlight` (amber) is reserved for a page's single primary emphasis — passive notes use `routes/explore/settings/SettingsNote.tsx`. Standalone amber text is `text-highlight`, never `text-highlight-foreground` (that token is dark and invisible on dark backgrounds).
- **A `<div>` inside a `<button>` is invalid HTML.** `RelativeTime` accepts a `className`, so use `className="block …"`.
- **No prettier in this repo.** Format check is `eslint .` only; never run `npx prettier --write`.
- **Test gotcha for any section nav:** the shell renders two navs (desktop grouped + mobile strip), both present in JSDOM because there is no CSS. Every section link appears twice, so `getByRole` throws — use `getAllByRole(...).some(...)`/`.every(...)`, as `WorkspaceSettings.test.tsx` does.
- Shared components live in `frontend/src/components/explore/`: `FilterableTable`, `SummaryStrip`, `FacetSelect`, `RowActionsMenu`, `ListSkeleton`, `RelativeTime`, `EmptyState`, `ConfirmDeleteDialog`.
- **Every list query must handle `isError` separately from empty.** The systemic bug this codebase already fixed once was a 500/403 rendering as the first-run empty state with a Create CTA.
- **There is no msw in this repo.** Tests mock the API module itself. The pattern, from `routes/control-plane/__tests__/AuditLog.test.tsx:12,60-62`:

```tsx
vi.mock("@/api/generated/data-api/data-api");
// …then per test:
vi.mocked(api.useDataApiListEndpoints).mockReturnValue(
  { data: { status: 200, data: ROWS }, isLoading: false, isError: false } as unknown as
    ReturnType<typeof api.useDataApiListEndpoints>,
);
```

Every test snippet below is written in that style. A snippet using `server.use(rest…)` would not run — if you see one anywhere, it is a mistake, not a second supported style.

## File Structure

| File | Responsibility |
|---|---|
| `frontend/src/routes/explore/dataapi/DataApiLayout.tsx` | Create: shell — header, section nav, `<Outlet/>` |
| `frontend/src/routes/explore/dataapi/Endpoints.tsx` | Create: endpoints list |
| `frontend/src/routes/explore/dataapi/EndpointDetail.tsx` | Create: detail route shell + its own section nav |
| `frontend/src/routes/explore/dataapi/detail/Definition.tsx` | Create: SQL, params, database, cache/limits/rate-limit |
| `frontend/src/routes/explore/dataapi/detail/ResponseSection.tsx` | Create: output shape/naming, frozen columns, OpenAPI download |
| `frontend/src/routes/explore/dataapi/detail/AccessSection.tsx` | Create: per-service-account status checklist |
| `frontend/src/routes/explore/dataapi/detail/TryIt.tsx` | Create: Try-it console |
| `frontend/src/routes/explore/dataapi/detail/Versions.tsx` | Create: version list, publish/rollback |
| `frontend/src/routes/explore/dataapi/Clients.tsx` | Create: service accounts → clients |
| `frontend/src/routes/explore/dataapi/Activity.tsx` | Create: tiles, per-endpoint table, call list |
| `frontend/src/App.tsx` | Modify: mount the routes |
| `frontend/src/components/layout/Sidebar.tsx:39-44` | Modify: give the `Data API` item `to` + `feature` |
| `frontend/src/components/control-plane/WorkspaceFeaturesTab.tsx` | Modify: the §6.4 toggle warning (mounted from `routes/control-plane/WorkspaceDetail.tsx:241`) |

One file per section, mirroring how `routes/explore/settings/` is laid out. `EndpointDetail.tsx` owns the nav and the shared save/dirty state; each section file owns one concern.

---

### Task 1: Regenerate the client, mount the shell

**Files:**
- Modify: `frontend/src/api/generated/**` (regenerated, committed)
- Create: `frontend/src/routes/explore/dataapi/DataApiLayout.tsx`
- Modify: `frontend/src/App.tsx`, `frontend/src/components/layout/Sidebar.tsx:39-44`
- Test: `frontend/src/routes/explore/dataapi/__tests__/DataApiLayout.test.tsx`

**Interfaces:**
- Produces: the route tree `/explore/:ws/data-api/{endpoints,clients,activity}` and `/explore/:ws/data-api/endpoints/:id`; every later task mounts inside it.

- [ ] **Step 1: Regenerate and inspect**

Run: `cd frontend && npm run gen:api`
Then confirm the four new operations exist:
`grep -n "useDataApiActivitySummary\|useDataApiActivityCalls\|useDataApiGetOpenapi\|useDataApiTryEndpoint" src/api/generated/data-api/data-api.ts`
Expected: four matches. The exact generated names come from the backend's `operation_id`s — if they differ, use what orval produced and record the real names in your report; do not rename them.

- [ ] **Step 2: Write the failing test**

```tsx
// frontend/src/routes/explore/dataapi/__tests__/DataApiLayout.test.tsx
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import DataApiLayout from "../DataApiLayout";

function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/explore/:ws/data-api" element={<DataApiLayout />}>
          <Route path="endpoints" element={<p>endpoints pane</p>} />
        </Route>
      </Routes>
    </MemoryRouter>,
  );
}

it("marks the active section and renders the outlet", () => {
  renderAt("/explore/analytics/data-api/endpoints");
  // The shell renders a desktop nav AND a mobile strip; both are in the DOM
  // under JSDOM because there is no CSS, so every link appears twice.
  const links = screen.getAllByRole("link", { name: "Endpoints" });
  expect(links.length).toBeGreaterThan(1);
  expect(links.some((l) => l.getAttribute("aria-current") === "page")).toBe(true);
  expect(screen.getByText("endpoints pane")).toBeInTheDocument();
});
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi`
Expected: FAIL — module not found.

- [ ] **Step 4: Write the shell**

Model it on `frontend/src/routes/explore/settings/WorkspaceSettings.tsx` (228 lines) — read that file first and follow its structure: a `SECTIONS` array, a `NavLink` item component, the desktop grouped nav plus the mobile horizontal strip, and `<Outlet/>` on the right. Two deliberate differences from the settings hub:

```tsx
/**
 * Data API shell.
 *
 * Section nav rather than a tab strip because the three areas are of different
 * natures and more will come (spec §2.2) — the same reason Workspace Settings
 * uses one.
 *
 * NOT collapsible, unlike the settings hub. That hub has nine sections in three
 * groups and earns a rail; three flat sections do not, and a second
 * localStorage-backed collapse key would be a second thing to keep in sync for
 * no gain.
 */
const SECTIONS = [
  { to: "endpoints", label: "Endpoints", icon: Plug },
  { to: "clients", label: "Clients", icon: KeyRound },
  { to: "activity", label: "Activity", icon: Activity },
] as const;
```

Header: `<h1 className="text-2xl font-semibold">Data API</h1>` with the workspace name as subtitle — the shell must outrank each section's own `text-lg` heading, exactly as the settings hub does. Layout `max-w-6xl p-6` with `lg:grid-cols-[13rem_minmax(0,1fr)]`.

- [ ] **Step 5: Mount the routes and wire the sidebar**

`App.tsx` — replace `<Route path="data-api" element={<Placeholder title="Data API" />} />` with:

```tsx
<Route path="data-api" element={<RequireFeature feature="data_api"><DataApiLayout /></RequireFeature>}>
  <Route index element={<Navigate to="endpoints" replace />} />
  <Route path="endpoints" element={<Endpoints />} />
  {/* `new` is not a route here: creating an endpoint asks for a name in a
      dialog and then navigates to the created id, so there is no draft URL
      that can be opened cold. */}
  <Route path="endpoints/:id" element={<EndpointDetail />} />
  <Route path="clients" element={<Clients />} />
  <Route path="activity" element={<Activity />} />
</Route>
```

`Sidebar.tsx:41` — `{ label: "Data API", icon: Plug, to: "data-api", feature: "data_api" }`.

- [ ] **Step 6: Run the tests**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi src/components/layout`
Expected: PASS. `Sidebar.test.tsx` asserts a representative item set — if it now sees a new link, update that assertion rather than weakening it.

- [ ] **Step 7: Verify the test is not vacuous**

Remove `aria-current` from the nav links (or the `end`/active class logic), re-run.
Expected: FAIL on the `aria-current` assertion. Restore.

- [ ] **Step 8: Commit**

```bash
cd frontend && npx tsc --noEmit && npx eslint . && cd ..
git add frontend/src/api/generated frontend/src/routes/explore/dataapi frontend/src/App.tsx frontend/src/components/layout/Sidebar.tsx
git commit -m "feat(data-api): mount the Data API area and regenerate the client"
```

---

### Task 2: Endpoints list

**Files:**
- Create: `frontend/src/routes/explore/dataapi/Endpoints.tsx`
- Test: `frontend/src/routes/explore/dataapi/__tests__/Endpoints.test.tsx`

**Interfaces:**
- Consumes: `useDataApiListEndpoints(workspace)`, `useDataApiCreateEndpoint`, `useDataApiDeleteEndpoint`, `useDataApiDisableEndpoint`, `useDataApiEnableEndpoint`.
- Produces: navigation to `endpoints/:id`.

Columns per spec §2.3: name · method + path · state · version · engine · last edited.

- [ ] **Step 1: Write the failing tests**

```tsx
vi.mock("@/api/generated/data-api/data-api");
import * as api from "@/api/generated/data-api/data-api";

function mockList(over: Partial<{ data: unknown; isError: boolean; isLoading: boolean }>) {
  vi.mocked(api.useDataApiListEndpoints).mockReturnValue(
    { data: undefined, isLoading: false, isError: false, ...over } as unknown as
      ReturnType<typeof api.useDataApiListEndpoints>,
  );
}

it("shows an error state, not the first-run CTA, when the list fails", async () => {
  mockList({ isError: true });
  renderEndpoints();
  expect(await screen.findByText(/could not load/i)).toBeInTheDocument();
  // The CTA copy that belongs to a genuinely EMPTY list must not appear on a failure.
  expect(screen.queryByText(/no endpoints yet/i)).not.toBeInTheDocument();
});

it("groups rows under their folder", async () => {
  mockList({ data: { status: 200, data: [FOLDERED_ROW, LOOSE_ROW] } });
  renderEndpoints();
  expect(await screen.findByText("Reporting")).toBeInTheDocument();   // folder header
  expect(screen.getByText("Uncategorized")).toBeInTheDocument();
});
```

- [ ] **Step 2: Run to verify failure**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi/__tests__/Endpoints.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Build the page on the Charts scaffold**

Read `frontend/src/routes/explore/Charts.tsx:131-250` and copy its shell and table shape. Take: `p-4`, the `text-lg` h1 + `+ Endpoint` button row, the search `Input`, `SummaryStrip`, the `overflow-x-auto` wrapper, `SortableHeader`, `FolderHeaderRow` grouping, and `RowActionsMenu` per row.

**Do not take** bookmarks, trash, bulk select, share or drag. Spec §1 puts them deliberately out of scope: an endpoint is an operational contract, not saved content, and a URL deployed clients depend on has no "move to trash" that means anything.

`SummaryStrip` items double as filter chips, so make them the states an operator actually filters by:

```tsx
<SummaryStrip
  items={[
    { label: "endpoints", value: rows.length },
    { label: "live", value: live.length, onClick: () => setStateFilter("live"), active: stateFilter === "live" },
    { label: "draft", value: draft.length, onClick: () => setStateFilter("draft"), active: stateFilter === "draft" },
    { label: "disabled", value: disabled.length, onClick: () => setStateFilter("disabled"), active: stateFilter === "disabled" },
  ]}
/>
```

The state cell is a `Badge`, not a coloured `<span>` — and `live` is the only one that earns emphasis:

```tsx
const STATE_TONE: Record<string, string> = {
  live: "bg-success/15 text-success",
  draft: "bg-muted text-muted-foreground",
  disabled: "bg-destructive/15 text-destructive",
};
```

- [ ] **Step 4: The workspace-level OpenAPI download**

Spec §5.6 puts a download on the **list** page too, not only on endpoint detail: the aggregate document is the thing you hand a consumer team, and the per-endpoint one is a narrowing of it.

A secondary button beside `+ Endpoint` opens a dialog to choose the service account, then downloads. Reuse the same component Task 4 builds for the detail page — one dialog, called with `endpoint_id` on detail and without it here — so the two cannot drift about which account they asked for or how they render the 409.

Two failures this button must render rather than swallow, both from the backend phase:
- **400 above 200 live endpoints.** The message names `?endpoint_id=` as the way through; show it, because the remedy is inside it.
- **403** when the caller lacks `data_api_endpoint:manage`. The route is gated `:manage`, not `:read` — a viewer can see the list and still not be allowed the grant roster. Render a permission message, not a dead button.

- [ ] **Step 5: Creating an endpoint asks for a name, then navigates**

Per the Alerts lesson (#485) and the Explore convention: the dialog collects the fields the row cannot be created without — name, path, method, database — and on success navigates to `endpoints/:id`. Everything else is edited on the detail route. The dialog must not become a second editor.

- [ ] **Step 6: Run the tests**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi`
Expected: PASS.

- [ ] **Step 7: Verify the error-state test is not vacuous**

Delete the `isError` branch so the component falls through to the empty state, re-run.
Expected: FAIL — the CTA copy appears on a 500. Restore. This is the systemic bug this codebase has already fixed once across four list pages; the test exists to stop it returning.

- [ ] **Step 8: Commit**

```bash
cd frontend && npx tsc --noEmit && npx eslint . && cd ..
git add frontend/src/routes/explore/dataapi
git commit -m "feat(data-api): endpoints list"
```

---

### Task 3: Endpoint detail shell + Definition

**Files:**
- Create: `frontend/src/routes/explore/dataapi/EndpointDetail.tsx`, `detail/Definition.tsx`
- Test: `frontend/src/routes/explore/dataapi/__tests__/EndpointDetail.test.tsx`

**Interfaces:**
- Consumes: `useDataApiGetEndpoint`, `useDataApiUpdateEndpoint`, `useDataApiValidateEndpoint`.
- Produces: the detail nav and the shared dirty/save state every other detail section hangs off.

- [ ] **Step 1: Write the failing tests**

```tsx
it("arms the unsaved-changes warning only after a real change", async () => {
  renderDetail();
  await screen.findByDisplayValue("SELECT 1");
  // Radix hydration and controlled-input echo must not count as a change.
  expect(warnSpy).toHaveBeenLastCalledWith(false);
  await userEvent.type(screen.getByLabelText("SQL"), " -- x");
  expect(warnSpy).toHaveBeenLastCalledWith(true);
});

it("re-baselines after a successful save", async () => {
  renderDetail();
  await userEvent.type(await screen.findByLabelText("SQL"), " -- x");
  await userEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() => expect(warnSpy).toHaveBeenLastCalledWith(false));
});
```

Spy on `useUnsavedChangesWarning` via `vi.mock("@/lib/useUnsavedChangesWarning", …)`.

- [ ] **Step 2: Run to verify failure**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi/__tests__/EndpointDetail.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Build the detail shell**

```tsx
const SECTIONS = [
  { to: "", label: "Definition", end: true },
  { to: "response", label: "Response" },
  { to: "access", label: "Access" },
  { to: "try", label: "Try it" },
  { to: "versions", label: "Versions" },
] as const;
```

Nested routes under `endpoints/:id`, each section its own child route so the section is in the URL and survives a refresh.

**Dirtiness is a comparison, never a flag:**

```tsx
// A serialized snapshot of the loaded value, re-baselined on save. An
// `onChange` boolean would latch on Radix's hydration echo and raise a phantom
// "Leave site?" on a page nobody edited — the exact defect Alerts #486 hit.
const [baseline, setBaseline] = useState<string | null>(null);
const current = useMemo(() => JSON.stringify(form), [form]);
useUnsavedChangesWarning(baseline !== null && current !== baseline);
```

Hydrate **once**, guarded — re-seeding on every data reference clobbers a mid-edit form, a bug this codebase has hit twice:

```tsx
const hydrated = useRef(false);
useEffect(() => {
  if (hydrated.current || !data) return;
  hydrated.current = true;
  setForm(toForm(data));
  setBaseline(JSON.stringify(toForm(data)));
}, [data]);
```

- [ ] **Step 4: Definition section**

One column at `max-w-3xl`. **Do not two-column this form** — tried twice on a comparable editor and wrong both times: a `grid` pads a short section to the height of the tall one beside it, and CSS `columns` fights the flex container and clips the form at the fold, which `tsc` cannot see and only a screenshot shows.

Fields: name, description, method (`GET`/`POST` only), path, database, SQL, parameters, cache, limits, rate limit.

**The parameter editor has no `location` control** (spec §2.6). Every parameter is `query`. `ParamLocation::Body` is refused at publish and `serve_post` has no body extractor, so offering the control and then rejecting one of its values would be a picker that can drift out of sync with the refusal.

Show validation through `useDataApiValidateEndpoint` before publish, surfacing the returned messages inline rather than as a toast that vanishes.

- [ ] **Step 5: Run and mutation-check**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi`
Expected: PASS.

Then replace the comparison with `const dirty = touched` (an `onChange` flag) and re-run.
Expected: FAIL on "only after a real change". Restore.

- [ ] **Step 6: Commit**

```bash
cd frontend && npx tsc --noEmit && npx eslint . && cd ..
git add frontend/src/routes/explore/dataapi
git commit -m "feat(data-api): endpoint detail route and definition editor"
```

---

### Task 4: Response section and the OpenAPI download

**Files:**
- Create: `frontend/src/routes/explore/dataapi/detail/ResponseSection.tsx`
- Test: extend `__tests__/EndpointDetail.test.tsx`

**Interfaces:**
- Consumes: `useDataApiGetOpenapi(workspace, { service_account_id, endpoint_id })`, `useDataApiListServiceAccounts`.

- [ ] **Step 1: Write the failing tests**

```tsx
it("shows the served base URL beside the download", async () => {
  vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
    { data: { status: 200, data: OPENAPI_FIXTURE }, isError: false } as unknown as
      ReturnType<typeof api.useDataApiGetOpenapi>,
  );
  renderResponse();
  // OPENAPI_FIXTURE.servers[0].url — the slug is part of it; see the backend note below.
  expect(await screen.findByText("https://egret.test/data-api/v1/analytics")).toBeInTheDocument();
});

it("names the endpoint and says re-publish when the document is refused", async () => {
  // apiFetch throws on non-2xx, so a 409 arrives as react-query's `error`,
  // an ApiError carrying `.status` — not as `data`.
  vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
    {
      data: undefined,
      isError: true,
      error: Object.assign(new Error("endpoint daily has an output naming collision; re-publish it"), { status: 409 }),
    } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
  );
  renderResponse();
  expect(await screen.findByText(/re-publish/i)).toBeInTheDocument();
  expect(screen.getByText(/daily/)).toBeInTheDocument();
});
```

- [ ] **Step 2: Run to verify failure, then build**

The section shows the frozen `columns` of the live version, the `output.shape`/`naming` controls, and the download.

**The download requires choosing a service account**, because the document is generated per account — it contains exactly the endpoints that account may invoke. Present that as the choice it is, not as a hidden parameter.

**Show the base URL next to the button.** It is derived server-side from `issuer`, and a derived value needs a place to be seen wrong; the human eye on this screen is the check. If the response carries a warning that the request host disagrees with `issuer`, surface it here — that mismatch means `catalog.public_url` is wrong, which breaks token minting long before it breaks a downloaded file.

`409` is the naming-collision refusal: show the endpoint name and "re-publish it", never a generic failure toast.

- [ ] **Step 3: Run, mutation-check the 409 branch, commit**

Remove the 409 handling so it falls to the generic error path, re-run, confirm FAIL, restore.

```bash
git commit -m "feat(data-api): response section with per-service-account OpenAPI download"
```

---

### Task 5: Access section — the real status checklist

**Files:**
- Create: `frontend/src/routes/explore/dataapi/detail/AccessSection.tsx`
- Test: `frontend/src/routes/explore/dataapi/__tests__/AccessSection.test.tsx`

**Interfaces:**
- Consumes: `useDataApiListEndpointGrants`, `useDataApiListServiceAccounts`, `useDataApiGrantEndpointInvoke`, `useDataApiRevokeEndpointInvoke`.

This is spec §6.2 and it is the task most likely to be built wrong in a way that looks right.

- [ ] **Step 1: Write the failing tests**

```tsx
it("distinguishes a grant on this endpoint from one inherited from the workspace", async () => {
  renderAccess();  // fixture: one account granted here, one reaching invoke via ws_admin
  expect(await screen.findByText("Granted on this endpoint")).toBeInTheDocument();
  expect(screen.getByText("Inherited from the workspace")).toBeInTheDocument();
  // The inherited row must NOT offer a revoke that would do nothing.
  const inherited = screen.getByTestId("access-row-inherited");
  expect(within(inherited).queryByRole("button", { name: /revoke/i })).not.toBeInTheDocument();
  expect(within(inherited).getByRole("link", { name: /membership/i })).toBeInTheDocument();
});

it("says egret cannot check Ranger, and offers Try it", async () => {
  renderAccess();
  expect(await screen.findByText(/egret cannot check/i)).toBeInTheDocument();
  expect(screen.getAllByRole("link", { name: /try it/i }).length).toBeGreaterThan(0);
});
```

- [ ] **Step 2: Run to verify failure, then build**

Each service account is one row with **two** conditions.

**Condition 1 — invoke.** Two distinct states, and collapsing them is the defect:

- *Granted on this endpoint* → revocable here, via `useDataApiRevokeEndpointInvoke`.
- *Inherited from the workspace* → say plainly that revoking here does nothing, and link to membership, which is where it can actually change.

The API gives you both halves: `/grants` lists the object-scoped rows, and the generated OpenAPI document (or a Try-it run) reflects the effective answer including the Cedar `ws_admin` cascade. An account that is effectively able to invoke but absent from `/grants` is *inherited*. If you render one ✓ with a Revoke button next to it, the user clicks revoke, the backend deletes a grant that does not exist, the ✓ remains, and the product looks broken while behaving correctly.

**Condition 2 — Ranger.** egret does not know, and the UI says exactly that. No inference from the endpoint's SQL. Next to it, a **"Run Try it"** link into the Try-it section with that account preselected — it is the only thing that can answer the question, because it traverses the real serving router.

**The §5.3 warning lives here too** (spec §6.3): publishing does not grant data access. Use `SettingsNote` (`bg-muted/40 border`, Info icon), **not** `--highlight` — amber is reserved for a page's single primary emphasis, and a passive standing note is not that.

- [ ] **Step 3: Run, mutation-check, commit**

Collapse the two invoke states into one boolean, re-run, confirm the first test FAILS, restore.

```bash
git commit -m "feat(data-api): access section with a real per-account status checklist"
```

---

### Task 6: Versions section

**Files:**
- Create: `frontend/src/routes/explore/dataapi/detail/Versions.tsx`
- Test: extend `__tests__/EndpointDetail.test.tsx`

**Interfaces:**
- Consumes: `useDataApiListEndpointVersions`, `useDataApiPublishEndpoint`, `useDataApiRollbackEndpoint`.

- [ ] **Step 1: Write the failing test**

```tsx
it("warns that publishing does not grant data access before it publishes", async () => {
  renderVersions();
  await userEvent.click(await screen.findByRole("button", { name: /publish/i }));
  expect(await screen.findByText(/does not grant data access/i)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: /publish/i })).toBeInTheDocument();  // still needs confirming
});
```

- [ ] **Step 2: Build**

A table of versions: version · note · who · when · a `live` badge on the one `live_version_id` points at. Publish takes an optional note. Rollback uses `ConfirmDeleteDialog`'s confirming idiom — it changes what every deployed client receives.

The publish dialog carries the §5.3 sentence (spec §6.3, second of its three placements): this is the moment *before* the endpoint goes live, and the one place the warning reaches someone who has not opened the Access section.

- [ ] **Step 3: Run, commit**

```bash
git commit -m "feat(data-api): versions section with publish and rollback"
```

---

### Task 7: Try it

**Files:**
- Create: `frontend/src/routes/explore/dataapi/detail/TryIt.tsx`
- Test: `frontend/src/routes/explore/dataapi/__tests__/TryIt.test.tsx`

**Interfaces:**
- Consumes: `useDataApiTryEndpoint(workspace, id)` — `POST`, body `{ service_account_id, params }`.

- [ ] **Step 1: Write the failing tests**

```tsx
const REFUSED = {
  status: 403,
  body: { error: { code: "not_granted", message: "…" } },
  request: {
    method: "GET",
    url: "https://egret.test/data-api/v1/analytics/probe",
    authorization: "Bearer ••••",
    curl: "curl -H 'Authorization: Bearer $TOKEN' 'https://egret.test/data-api/v1/analytics/probe'",
  },
};

function mockTry(result: unknown) {
  const mutateAsync = vi.fn().mockResolvedValue({ status: 200, data: result });
  vi.mocked(api.useDataApiTryEndpoint).mockReturnValue(
    { mutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiTryEndpoint>,
  );
  return mutateAsync;
}

it("renders a 403 from the endpoint as a result, not as a failure of the trial", async () => {
  mockTry(REFUSED);
  renderTryIt();
  await userEvent.click(screen.getByRole("button", { name: /run/i }));
  expect(await screen.findByText("403")).toBeInTheDocument();
  expect(screen.getByText(/not_granted/)).toBeInTheDocument();
  // The most common first failure names its fix.
  expect(screen.getByRole("link", { name: /access/i })).toBeInTheDocument();
});

it("never shows a token", async () => {
  mockTry(REFUSED);
  renderTryIt();
  await userEvent.click(screen.getByRole("button", { name: /run/i }));
  expect(await screen.findByText(/Bearer ••••/)).toBeInTheDocument();
  expect(document.body.textContent).not.toMatch(/eyJ/);
});
```

- [ ] **Step 2: Run to verify failure, then build**

The console: a service-account picker, one input per declared parameter, a Run button, and a result pane showing the returned `status`, the body, and the echoed request with `Authorization: Bearer ••••` and a copyable `curl` block using `$TOKEN`.

**A `403 not_granted` is a result, not an error.** The route answers `200` whenever the trial ran; the endpoint's own status lives in the body. Render it as the outcome it is, and when it is `not_granted`, name the fix and link to the Access section.

**Three consequences the UI must state** (spec §3.2), because a user cannot discover them:

- A trial **spends rate-limit budget**. State the rule the backend actually implements: the endpoint-wide bucket is spent for real, while the per-client bucket is the trial's own — so a console click cannot throttle a live consumer.
- A trial **writes to the access log**, and appears in Activity's call list under *Include trial runs*.
- On a cached endpoint a trial **can populate the cache real consumers read** — including `per_principal` entries for that same account, not only `shared` ones.

`413` is the "result too large to display" case: say so and point at Activity, do not render a truncated body as if it were the answer.

- [ ] **Step 3: Run, mutation-check, commit**

Remove the `not_granted` special-casing so it renders as a generic error, re-run, confirm FAIL, restore.

```bash
git commit -m "feat(data-api): try-it console"
```

---

### Task 8: Clients

**Files:**
- Create: `frontend/src/routes/explore/dataapi/Clients.tsx`
- Test: `frontend/src/routes/explore/dataapi/__tests__/Clients.test.tsx`

**Interfaces:**
- Consumes: `useDataApiListServiceAccounts`, `useDataApiCreateServiceAccount`, `useDataApiDeleteServiceAccount`, `useDataApiListClients`, `useDataApiIssueClient`, `useDataApiRevokeClient`.

- [ ] **Step 1: Write the failing tests**

```tsx
it("shows the secret once, with a copy button and a warning", async () => {
  renderClients();
  await userEvent.click(await screen.findByRole("button", { name: /new client/i }));
  await userEvent.type(screen.getByLabelText("Name"), "etl");
  await userEvent.click(screen.getByRole("button", { name: /create/i }));
  expect(await screen.findByText(/cannot be shown again/i)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: /copy/i })).toBeInTheDocument();
});

it("does not show the secret anywhere in the list", async () => {
  renderClients();
  await screen.findByText("etl");
  expect(document.body.textContent).not.toContain(SECRET_FIXTURE);
});
```

- [ ] **Step 2: Build**

A plain two-level list, so **`FilterableTable` + `SummaryStrip` is right here** — this is the one page in the plan that uses it, as `DatabasesPage` does. Service account → its clients.

**The secret appears exactly once**, in a dialog after creation, with a copy button and a sentence saying it cannot be shown again. It is never re-fetchable, never in a list cell, never in a toast.

Deleting a service account is destructive beyond its row — it takes its grants and every client with it. Use `ConfirmDeleteDialog` (type-to-confirm) and say what goes with it.

- [ ] **Step 3: Run, commit**

```bash
git commit -m "feat(data-api): clients section"
```

---

### Task 9: Activity

**Files:**
- Create: `frontend/src/routes/explore/dataapi/Activity.tsx`
- Test: `frontend/src/routes/explore/dataapi/__tests__/Activity.test.tsx`

**Interfaces:**
- Consumes: `useDataApiActivitySummary(workspace, { range })`, `useDataApiActivityCalls(workspace, {…})`.

- [ ] **Step 1: Write the failing tests**

```tsx
it("renders a dash, not 0%, when nothing in range was cacheable", async () => {
  // summary fixture: cache_hits 0, cacheable_calls 0
  renderActivity();
  expect(await screen.findByTestId("tile-cache-hit")).toHaveTextContent("—");
});

it("disables ranges longer than retention and says how long data is kept", async () => {
  // summary fixture: retention_days 7
  renderActivity();
  expect(await screen.findByText(/kept for 7 days/i)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "30d" })).toBeDisabled();
});

it("labels the unmatched bucket with what was actually requested", async () => {
  renderActivity();
  expect(await screen.findByText("No matching endpoint")).toBeInTheDocument();
});
```

- [ ] **Step 2: Build the three tiers**

**Tiles.** Calls · error rate · p50/p95 · cache-hit rate. The API sends raw counts, not rates, deliberately — a rate whose denominator is invisible cannot be checked. Divide here, and label honestly:

- p50/p95 must say **"across successful calls"**. The number is computed over 2xx rows only, because a burst of fast 401s otherwise drags p50 down and makes latency look *better* at exactly the moment the system is failing. An unlabelled percentile is a number nobody can act on.
- Cache-hit's denominator is `cacheable_calls`. When it is `0`, render `—`, never `0%`: zero of zero is not zero.

**Per-endpoint table.** The same four metrics per row, plus the final **"No matching endpoint"** row for `endpoint_id: null`. Its total will be smaller than the tile's and that is correct; those rows resolved no endpoint. Say so in a caption rather than letting a reader think the screen is inconsistent.

> **Correction (final review, 2026-08-17).** This step used to say "render its `request_method` + `request_path` instead of a name", and spec §4.4 implied the same. **That is not buildable and it was never right — do not re-add it.** This row is one `GROUPING SETS` bucket over *every* unmatched request in range, so it aggregates many different 404 URLs; there is no single method/path for it to name. `ActivityRow` (this table's row type) carries no such field, only `ActivityCallDto` — the call list's per-call shape — does, documented there as "the ONLY identification a row carries when `endpoint_id` is null". Labelling the row and reading the URLs one at a time in the call list below (§4.6) is the correct build, and is what shipped. Spec §4.4 has been corrected in both languages.

**Call list.** `ts` · endpoint · status + `error_code` · duration · cache hit · `client_id` · `request_id`, with filters for endpoint / status / error code and a direct `request_id` lookup — support's entry point when a caller quotes it out of an error body. Cursor pagination via `before`, accumulating like `AuditLog.tsx`'s "Load more".

**"Include trial runs" is off by default.** When on, trial rows are labelled with the person who ran them.

**Range presets** `1h` / `24h` / `7d` / `30d`, with anything longer than `retention_days` disabled and a line saying how long data is kept — otherwise someone selects `30d`, watches data thin out at the old end, and concludes traffic fell.

No time-series chart (spec §4.8) — this page pulls in no charting stack.

- [ ] **Step 3: Run, mutation-check, commit**

Change the cache-hit tile to render `0%` when the denominator is zero, re-run, confirm the first test FAILS, restore.

```bash
git commit -m "feat(data-api): activity statistics and call list"
```

---

### Task 10: The feature-toggle warning

**Files:**
- Modify: `frontend/src/components/control-plane/WorkspaceFeaturesTab.tsx` — the component rendering the entitlement switches, mounted from `frontend/src/routes/control-plane/WorkspaceDetail.tsx:241-243`
- Test: `frontend/src/routes/control-plane/__tests__/WorkspaceDetail.test.tsx`

Note the section: this is a **control-plane** page, so it follows the control-plane idiom, not the Explore one. Do not carry `p-4`/`text-lg` conventions into it.

- [ ] **Step 1: Write the failing test**

```tsx
it("warns that disabling Data API does not stop serving", async () => {
  renderWorkspaceDetail();
  await userEvent.click(screen.getByRole("tab", { name: "Features" }));
  expect(await screen.findByText(/live endpoints keep serving/i)).toBeInTheDocument();
});
```

- [ ] **Step 3: Add the sentence at the toggle**

Spec §6.4. Disabling the `data_api` feature hides the management surface but **live endpoints keep serving** — an operator reaching for that switch to stop an API would otherwise believe they had.

**It must live at the toggle.** Saying it on the Data API page is useless: disabling the feature hides that page, exactly when the sentence is needed.

- [ ] **Step 4: Run, commit**

```bash
git commit -m "feat(data-api): warn that disabling the feature does not stop serving"
```

---

## Final verification

- [ ] `cd frontend && npx tsc --noEmit` — 0 errors.
- [ ] `cd frontend && npx eslint .` — clean. There are 3 pre-existing warnings in untouched chart/CP files; do not "fix" those.
- [ ] `cd frontend && npx vitest run` — the whole suite. It was ~939 tests at the last full gate; nothing in this plan should reduce that.
- [ ] `cd frontend && npm run build` — succeeds. `tsc` alone does not catch every bundling problem.
- [ ] **Live check in a browser, and treat it as required rather than optional.** Start the stack, log in, and drive: create an endpoint → publish it → grant a service account → run Try it → download the OpenAPI → open Activity. Every defect that mattered in this codebase's chart programme was found here and not by a passing suite.
  - **The one thing a test cannot see:** `POST …/try` traverses the CSRF layer only in a real browser. Confirm it succeeds there. A router-level test passes with or without the header.
  - Check light **and** dark, and desktop **and** mobile widths — the shell renders two navs and only CSS decides which is visible.

## Notes carried from the backend phase

- `ranger_denied` no longer exists. Engine-side denial is `data_access_denied`, and it covers both a Ranger policy denial and a bad or expired engine credential — so the message must not name only Ranger.
- `GET …/data-api/openapi` is gated `data_api_endpoint:manage`, not `:read`. A viewer-level principal gets 403; the Response section's download must render that as a permission message, not a broken button.
- The OpenAPI generator refuses above 200 live endpoints with a 400 naming `?endpoint_id=`. Surface that message rather than a generic failure — the remedy is in it.
- A trial's `sub` is the reserved marker `trial:<service_account_id>`, so trial rows in the call list carry that as `client_id` rather than a real client. Label them from `actor_user_id`, not from `client_id`.
