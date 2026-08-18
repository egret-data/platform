# Data API New Pages Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the three Data API create dialogs with dedicated routes, so creating an endpoint, a service account, or a client credential is a page with a URL rather than a modal.

**Architecture:** Three small create pages following `engines/new` → `EngineCreate` — each collects only what a row cannot exist without, creates it, and hands off to the object's real editor. The detail shell is **not** reused; see the correction below.

**Tech Stack:** React 18, TypeScript, react-router-dom, TanStack Query via orval-generated hooks, shadcn/Radix, vitest + Testing Library. Frontend only — every route already exists on the backend.

**Spec:** `docs/en/specs/2026-08-18-data-api-frontend-rework-design.md` §4 — this is **phase 4**.

**The spec was corrected before this plan was written.** §4 originally said to render the same component as the detail route, following `charts/new` and `alerts/new`. Measuring the detail route shows why that is wrong here: **six of its seven components read `id`** — `PreviewBlock` (preview POSTs to `…/endpoints/{id}/preview`), `ResponseSection`, `AccessSection`, `TryIt`, `Versions` and `Config`. Only `Query` does not. Reusing the shell would ship a page whose Preview cannot run and whose four right-hand tabs are meaningless until the first save. That precedent fits a chart or a schedule — a single form with no id-dependent sub-resources. An endpoint has versions, grants and trials.

## Global Constraints

- **Register the literal `new` BEFORE `:id`**, or the router captures it as an id. `App.tsx:167-176` carries that exact comment for `alerts/new`; follow it.
- **A create page collects only what the row cannot exist without.** Everything else belongs in the editor the user lands on. Widening the form is how a create page becomes a second, worse editor.
- **The hand-off is the part to get right:** create, then navigate to the object's editor, without losing what the author typed and without leaving a half-created row if navigation fails.
- **`App.tsx:124-127` carries a comment arguing AGAINST a `new` route for endpoints**, on the grounds that the dialog collects the minimum and hands off immediately. That reasoning is superseded — the dialog and the page do the same thing, and only one of them has a URL. **Replace the comment**; two contradictory rationales in one file are worse than either.
- **Route both create mutations through `frontend/src/api/dataApi.ts`** so the list invalidates on success, reading the workspace off the mutation variables.
- **CSRF is free through the generated client and only through it** — never hand-roll a `fetch`.
- **The eslint baseline is exactly 0 errors / 178 warnings.** A new warning is a regression.
- **No msw.** Tests `vi.mock` the API module and use `vi.mocked(hook).mockReturnValue(...)`.
- **`ListPageShell` owns a page's single `h1`.** A create page that renders its own as well has two.
- **Every assertion is mutation-checked before being believed, and a mutation that fails to compile proves nothing** — build to zero errors first, confirm that, then read the result. If a prescribed mutation will not compile, re-cut it into one that does and is equivalent in force, and say so.
- **Run the FULL frontend suite before every commit, unpiped.** Never pipe a test run into `head` or `tail` — a piped run was SIGPIPE-killed mid-suite twice in this project while still exiting 0.
- **Fixtures must contain the case where correct and broken disagree.** Six defects in the previous phase survived their own mutations because a fixture was too small: one row where two were needed, an empty folders list where a populated one was the only revealing state. Before writing a test, ask what input would make a broken implementation pass it.

---

## File Structure

| File | Responsibility |
|---|---|
| `frontend/src/routes/explore/dataapi/EndpointCreate.tsx` | **New.** Name, path, method, database, SQL. Creates and navigates to the endpoint's editor. |
| `frontend/src/routes/explore/dataapi/ServiceAccountCreate.tsx` | **New.** Name and description. Creates and returns to the Clients list. |
| `frontend/src/routes/explore/dataapi/ClientCreate.tsx` | **New.** Issues a client credential for one service account, and shows the secret **on the same page** in a created state. |
| `frontend/src/routes/explore/dataapi/Endpoints.tsx` | **Modify.** `+ Endpoint` becomes a `Link`; `CreateEndpointDialog` deleted. |
| `frontend/src/routes/explore/dataapi/Clients.tsx` | **Modify.** `+ Service account` becomes a `Link`; both dialogs deleted; the row menu's Issue-client item becomes a `Link`. |
| `frontend/src/App.tsx` | **Modify.** Three routes, each with `new` before `:id`; the superseded comment replaced. |

---

### Task 1: `endpoints/new`

**Files:**
- Create: `frontend/src/routes/explore/dataapi/EndpointCreate.tsx`
- Test: `frontend/src/routes/explore/dataapi/__tests__/EndpointCreate.test.tsx`
- Modify: `frontend/src/routes/explore/dataapi/Endpoints.tsx`, `frontend/src/App.tsx`

**Interfaces:**
- Consumes: `ListPageShell({ title, description?, toolbar?, action?, children })`; `useCreateEndpoint` from `@/api/dataApi`; `useDatabases` from `@/api/databases`; `useNavigate`.
- Produces: route `/explore/:workspace/data-api/endpoints/new`.

**What the dialog collects today** (`Endpoints.tsx:585+`): name, path, method (GET/POST), database, SQL. Move exactly those. The page has room the dialog did not, so give SQL a taller editor and add the optional description the dialog omitted — but **do not** add cache, limits, rate limit, or parameters: those are the editor's, and adding them here rebuilds the editor in a worse place.

- [ ] **Step 1: Write the failing test**

```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClientProvider, QueryClient } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

vi.mock("@/api/dataApi");
vi.mock("@/api/databases");

import * as dataApi from "@/api/dataApi";
import * as databasesApi from "@/api/databases";
import EndpointCreate from "@/routes/explore/dataapi/EndpointCreate";

const navigate = vi.fn();
vi.mock("react-router-dom", async (orig) => ({
  ...(await orig<typeof import("react-router-dom")>()),
  useNavigate: () => navigate,
}));

function renderCreate() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/ws1/data-api/endpoints/new"]}>
          <Routes>
            <Route path="/explore/:workspace/data-api/endpoints/new" element={<EndpointCreate />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

describe("EndpointCreate", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }, { id: "db-2", name: "lake" }],
    } as never);
  });

  it("sends what the author typed, and only the fields a row needs", async () => {
    const mutateAsync = vi.fn().mockResolvedValue({ status: 201, data: { id: "e-9" } });
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({ mutateAsync, isPending: false } as never);
    renderCreate();

    await userEvent.type(screen.getByLabelText("Name"), "Daily report");
    await userEvent.type(screen.getByLabelText("Path"), "reports/daily");
    await userEvent.type(screen.getByLabelText("SQL"), "SELECT 1");
    // Two databases in the picker, so choosing the SECOND proves the value is
    // read from the choice rather than defaulted to the only option available.
    await userEvent.click(screen.getByRole("combobox", { name: "Database" }));
    await userEvent.click(await screen.findByRole("option", { name: "lake" }));
    await userEvent.click(screen.getByRole("button", { name: /create endpoint/i }));

    expect(mutateAsync).toHaveBeenCalledTimes(1);
    const body = mutateAsync.mock.calls[0][0].data;
    expect(body).toMatchObject({ name: "Daily report", path: "reports/daily", sql: "SELECT 1", database_id: "db-2" });
    // The create page must not invent config the editor owns.
    expect(Object.keys(body)).not.toContain("cache");
    expect(Object.keys(body)).not.toContain("rate_limit");
  });

  it("hands off to the created endpoint's editor, not back to the list", async () => {
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({
      mutateAsync: vi.fn().mockResolvedValue({ status: 201, data: { id: "e-9" } }),
      isPending: false,
    } as never);
    renderCreate();
    await userEvent.type(screen.getByLabelText("Name"), "x");
    await userEvent.type(screen.getByLabelText("Path"), "x");
    await userEvent.type(screen.getByLabelText("SQL"), "SELECT 1");
    await userEvent.click(screen.getByRole("combobox", { name: "Database" }));
    await userEvent.click(await screen.findByRole("option", { name: "warehouse" }));
    await userEvent.click(screen.getByRole("button", { name: /create endpoint/i }));
    expect(navigate).toHaveBeenCalledWith("/explore/ws1/data-api/endpoints/e-9");
  });

  it("keeps what the author typed when creation fails", async () => {
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue(new Error("path already taken")),
      isPending: false,
    } as never);
    renderCreate();
    await userEvent.type(screen.getByLabelText("Name"), "Daily report");
    await userEvent.type(screen.getByLabelText("Path"), "reports/daily");
    await userEvent.type(screen.getByLabelText("SQL"), "SELECT 1");
    await userEvent.click(screen.getByRole("combobox", { name: "Database" }));
    await userEvent.click(await screen.findByRole("option", { name: "warehouse" }));
    await userEvent.click(screen.getByRole("button", { name: /create endpoint/i }));

    // A dialog could be dismissed and retried; a page that clears itself on
    // failure loses work the author cannot get back. The engine's own message
    // must be on screen, and every field must still hold its value.
    expect(await screen.findByText(/path already taken/i)).toBeInTheDocument();
    expect(screen.getByLabelText("Name")).toHaveValue("Daily report");
    expect(screen.getByLabelText("SQL")).toHaveValue("SELECT 1");
    expect(navigate).not.toHaveBeenCalled();
  });

  it("renders exactly one h1", async () => {
    vi.mocked(dataApi.useCreateEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    renderCreate();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi/__tests__/EndpointCreate.test.tsx`
Expected: FAIL — module not found.

- [ ] **Step 3: Write the page**

Use `ListPageShell` for the frame so the page has the same heading and padding as every other Explore route, with a "← Endpoints" link back. Move the five fields out of `CreateEndpointDialog` (`Endpoints.tsx:585+`) verbatim, changing the `id`/`htmlFor` prefix from `ep-` to `new-ep-`, give SQL `rows={12}`, and add an optional Description.

**On failure the form must keep its values and show the server's message inline** — a page that clears itself loses work a dialog would have kept. **On success navigate to `/explore/${ws}/data-api/endpoints/${created.id}`.**

- [ ] **Step 4: Run to verify it passes**

Run: `cd frontend && npx vitest run src/routes/explore/dataapi/__tests__/EndpointCreate.test.tsx`
Expected: PASS, 4 tests.

- [ ] **Step 5: Wire the route and retire the dialog**

In `App.tsx`, add `data-api/endpoints/new` **before** the `data-api/endpoints/:id` route, and **replace** the comment at `:124-127` that argues against this route with one saying why the page won. In `Endpoints.tsx`, turn `+ Endpoint` and the empty-state CTA into `Link`s and delete `CreateEndpointDialog` and its state.

- [ ] **Step 6: Mutation-check**

1. Navigate to the list instead of the created endpoint → the hand-off test must fail.
2. Clear the form in the `catch` → the failure test must fail.
3. Default `database_id` to the first database instead of the chosen one → the first test must fail. *(This is the one the two-database fixture exists for; with one database it would pass.)*

Each built to zero errors first.

- [ ] **Step 7: Full gates and commit**

```bash
cd frontend && npx vitest run && npm run typecheck && npm run lint && npm run build
```

```bash
git add frontend/src
git commit -m "feat(dataapi): create an endpoint on a page with a URL, not in a dialog"
```

---

### Task 2: `clients/new` — the service account

**Files:**
- Create: `frontend/src/routes/explore/dataapi/ServiceAccountCreate.tsx`
- Test: `frontend/src/routes/explore/dataapi/__tests__/ServiceAccountCreate.test.tsx`
- Modify: `frontend/src/routes/explore/dataapi/Clients.tsx`, `frontend/src/App.tsx`

**Interfaces:**
- Consumes: `ListPageShell`; the create mutation from `@/api/dataApi`; `useNavigate`.
- Produces: route `/explore/:workspace/data-api/clients/new`.

Read `CreateServiceAccountDialog` at the bottom of `Clients.tsx` for the fields it collects; move exactly those.

**A service account has no editor to hand off to** — it is a row in a list with a menu. So on success this page returns to `/explore/${ws}/data-api/clients`, unlike Task 1. Say that in a comment: the difference is not an inconsistency, it is that one object has a detail route and the other does not.

- [ ] **Step 1: Write the failing test**

Four tests, mirroring Task 1's shape but with the different hand-off:

1. sends the fields the dialog collected, and nothing more;
2. **returns to the Clients list on success** (assert the exact path);
3. keeps what was typed and shows the server's message when creation fails;
4. renders exactly one `h1`.

Write them out fully rather than referring to Task 1 — the implementer may read this task alone.

- [ ] **Step 2-4: Run, write the page, run again**

Same cycle as Task 1: verify the tests fail for the right reason, write the page with `ListPageShell` and a "← Clients" link, then verify they pass.

- [ ] **Step 5: Wire the route and retire the dialog**

`clients/new` **before** any `clients/:id` route in `App.tsx`; `+ Service account` and the empty-state CTA become `Link`s; `CreateServiceAccountDialog` and its state deleted.

- [ ] **Step 6: Mutation-check**

1. Navigate to the endpoints list instead of the clients list → the hand-off test must fail.
2. Clear the form on failure → the failure test must fail.

- [ ] **Step 7: Full gates and commit**

```bash
cd frontend && npx vitest run && npm run typecheck && npm run lint && npm run build
git add frontend/src && git commit -m "feat(dataapi): create a service account on a page"
```

---

### Task 3: `clients/:accountId/new-client` — issuing a credential

**Files:**
- Create: `frontend/src/routes/explore/dataapi/ClientCreate.tsx`
- Test: `frontend/src/routes/explore/dataapi/__tests__/ClientCreate.test.tsx`
- Modify: `frontend/src/routes/explore/dataapi/Clients.tsx`, `frontend/src/routes/explore/dataapi/ServiceAccountActionsMenu.tsx`, `frontend/src/App.tsx`

**Interfaces:**
- Consumes: `ListPageShell`; the issue mutation and `useDataApiListServiceAccounts` from the generated client; `useParams` for `accountId`.
- Produces: route `/explore/:workspace/data-api/clients/:accountId/new-client`.

**This is the one with a real design question, and the answer is the reason it is worth doing.** Issuing a client returns a secret shown **once**. As a dialog that was a modal opening on top of another modal; the previous phase found that the issue dialog is unmounted while open when the secret dialog opens, which is the classic source of a stuck `pointer-events: none`.

As a page, the secret has a better home: **the same route, in a created state.** After a successful issue the page replaces its form with the credential, a copy button, and a "Done" link back to the list. That cannot be dismissed by clicking outside, cannot be closed by an accidental Escape, and survives a refresh with an honest empty state rather than a blank modal.

**The secret must never be refetchable.** If the page is reloaded after issuing, it must say the secret is gone and cannot be shown again — not silently render an empty field.

- [ ] **Step 1: Write the failing test**

Five tests:

1. **the account is named on the page** — it is issuing a credential *for* something, and the URL carries only an id;
2. **issuing shows the secret and a copy control**, and the form is gone;
3. **the secret is shown once**: after issuing, remounting the route (a reload) must show a "cannot be shown again" state rather than an empty field or a stale secret;
4. **a failed issue keeps the form and shows the server's message**;
5. **exactly one `h1`**.

Test 3 is the one that matters and the one a smaller fixture would miss: assert on a **fresh mount** of the route, not on the same component instance.

- [ ] **Step 2-4: Run, write the page, run again**

- [ ] **Step 5: Wire the route and retire the dialogs**

`clients/:accountId/new-client` in `App.tsx`. `ServiceAccountActionsMenu`'s "Issue client…" item becomes a `Link` to it. Delete `IssueClientDialog` and `NewClientSecretDialog` from `Clients.tsx`, along with the `pendingIssue`/`newClient` state that the previous phase's review flagged as unmounting an open Radix modal.

- [ ] **Step 6: Mutation-check**

1. Render the secret from a value that survives a remount → test 3 must fail.
2. Clear the form on failure → test 4 must fail.
3. Show the account's id instead of its name → test 1 must fail.

- [ ] **Step 7: Full gates and commit**

```bash
cd frontend && npx vitest run && npm run typecheck && npm run lint && npm run build
git add frontend/src && git commit -m "feat(dataapi): issue a client credential on a page, with the secret shown in place"
```

---

## After the plan: verify it in a browser

Every phase of this project has found a defect in a browser that no test could see.

1. **Confirm the running server is the binary you just built** before anything else — a stale process on :8080 has already cost this project one wasted measurement round.
2. **Create an endpoint from the new page** and confirm you land in its editor with the SQL you typed.
3. **Submit a path that already exists** and confirm the form keeps every value and shows the server's message.
4. **Issue a client**, copy the secret, then **reload the page** — it must say the secret cannot be shown again, not render an empty field.
5. **After issuing and returning to the list, click anything** — the previous phase's stuck-`pointer-events` risk came from dialogs; confirm the page route has no such residue.
6. **Use the browser Back button** from each create page; a URL that cannot be left is worse than a dialog.

---

## Self-Review

**Spec coverage:** §4's three routes are Tasks 1-3; the superseded `App.tsx` comment is Task 1 Step 5; the corrected precedent (`EngineCreate`, not the detail shell) is in this plan's header and the spec itself.

**Placeholder scan:** Task 2's test bodies are described rather than written out, with their shape given and the one difference from Task 1 named. That is the thinnest point; the two mutations in its Step 6 are what keep those tests honest.

**Type consistency:** all three pages take no props and read the workspace from `useParams`; all three use `ListPageShell` for the frame and own the page's single `h1` through it; the two hand-off destinations differ deliberately and the reason is recorded in Task 2.
