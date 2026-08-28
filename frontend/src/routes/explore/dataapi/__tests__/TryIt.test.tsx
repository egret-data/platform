import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type React from "react";
import { MemoryRouter, Outlet, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

/**
 * `react-resizable-panels`, stubbed to the three props `TryIt` passes — the
 * same stub, and for the same reason, as `Query.test.tsx`.
 *
 * **Not for convenience: the real one is unusable here.** With no layout engine
 * every rect in jsdom is 0×0, so the library's `Separator` wins the hit-test for
 * a click anywhere in the group and takes focus. Every `userEvent.type` in this
 * file would then go to the drag handle instead of the field, and the section
 * would silently read as if nothing could be typed into it. That is an artifact
 * of the environment, not of the product — the drag itself is a browser check.
 *
 * The stub echoes the SIZES back as attributes rather than swallowing them,
 * because those are exactly what a reader cannot verify by eye: in v4 a bare
 * number means PIXELS and a string means percent, so `defaultSize={40}` is a
 * 40-pixel setup pane that still looks right in the source.
 */
vi.mock("react-resizable-panels", () => ({
  Group: ({ children, className, orientation }: { children?: React.ReactNode; className?: string; orientation?: string }) => (
    <div data-testid="panel-group" data-orientation={orientation} className={className}>
      {children}
    </div>
  ),
  Panel: ({ children, className, defaultSize, minSize }: { children?: React.ReactNode; className?: string; defaultSize?: number | string; minSize?: number | string }) => (
    <div
      data-testid="panel"
      data-default-size={String(defaultSize)}
      data-default-size-type={typeof defaultSize}
      data-min-size={String(minSize)}
      data-min-size-type={typeof minSize}
      className={className}
    >
      {children}
    </div>
  ),
  Separator: ({ className }: { className?: string }) => (
    <div data-testid="panel-separator" role="separator" className={className} />
  ),
}));

// No msw in this repo — tests mock the API module directly (see
// AccessSection.test.tsx / EndpointDetail.test.tsx).
vi.mock("@/api/generated/data-api/data-api");

import * as api from "@/api/generated/data-api/data-api";
import { ApiError } from "@/api/client";
import { ParamLocation, ParamType, type DataApiEndpointDto, type DataApiServiceAccountDto } from "@/api/generated/model";
import type { EndpointDetailContext } from "@/routes/explore/dataapi/EndpointDetail";
import TryIt from "@/routes/explore/dataapi/detail/TryIt";
import { expectSectionCapsItsOwnWidth } from "./sectionWidth";

function makeEndpoint(overrides: Partial<DataApiEndpointDto> = {}): DataApiEndpointDto {
  return {
    id: "ep-1",
    folder_id: null,
    name: "Daily report",
    description: "",
    path: "reports/daily",
    method: "GET",
    database_id: "db-1",
    statements: [{ name: "main", sql: "SELECT 1" }],
    params: [],
    output: {},
    cache: {},
    limits: {},
    rate_limit: {},
    source_kind: "sql",
    source_ref: null,
    live_version_id: "v-1",
    state: "live",
    created_by: "user-1",
    created_at: "2026-08-01T00:00:00Z",
    updated_at: "2026-08-10T00:00:00Z",
    ...overrides,
  } as unknown as DataApiEndpointDto;
}

function makeAccount(id: string, name: string): DataApiServiceAccountDto {
  return { id, name, description: "", email: `${id}@svc.local`, created_at: "2026-01-01T00:00:00Z" };
}

const SA_1 = makeAccount("sa-1", "reporting-bot");

/** A refused trial: the endpoint's OWN status (403) travels inside the `/try` route's 200 body. */
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

/** A successful trial. */
const OK = {
  status: 200,
  body: { rows: [{ id: 1 }], meta: { row_count: 1 } },
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

function mockTryRejects(error: unknown) {
  const mutateAsync = vi.fn().mockRejectedValue(error);
  vi.mocked(api.useDataApiTryEndpoint).mockReturnValue(
    { mutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiTryEndpoint>,
  );
  return mutateAsync;
}

/**
 * Supplies the outlet context `useEndpointDetail()` reads, WITHOUT mounting
 * the real `EndpointDetail` shell. Mounting the shell would also mount its
 * section nav — which has its own permanent link literally named "Access" —
 * and that collides with `getByRole("link", { name: /access/i })` below the
 * moment this section adds its OWN "go fix it in Access" link, turning a
 * single, expected match into an ambiguous one. `TryIt` only ever reads
 * `ws`/`id`/`endpoint` off the context (per its own contract), so a partial
 * context is a faithful stand-in, not a shortcut that hides real behavior.
 */
function ContextProvider({ ctx }: { ctx: Partial<EndpointDetailContext> }) {
  return <Outlet context={ctx} />;
}

/**
 * `fieldset: true` reproduces the shell's `<fieldset disabled={trashed}>`
 * WITHOUT mounting `EndpointDetail` (whose nav collides with this section's
 * links — see `ContextProvider`). It exists for one class of test only: a
 * `<details>` is exempt from `fieldset[disabled]` and a Radix trigger is not,
 * and that difference is invisible unless a disabled fieldset is really in the
 * tree. Everything else renders WITHOUT one, so `toBeDisabled()` cannot pass by
 * walking up to an ancestor this section did not put there.
 */
function renderTryIt(
  opts: { endpoint?: DataApiEndpointDto; search?: string; trashed?: boolean; fieldset?: boolean } = {},
) {
  const endpoint = opts.endpoint ?? makeEndpoint();
  const search = opts.search ?? `?service_account=${SA_1.id}`;
  const ctx: Partial<EndpointDetailContext> = {
    ws: "eng",
    id: endpoint.id,
    endpoint,
    trashed: opts.trashed ?? false,
  };

  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const tree = (
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/try${search}`]}>
        <Routes>
          <Route path="/try" element={<ContextProvider ctx={ctx} />}>
            <Route index element={<TryIt />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
  const utils = render(opts.fieldset ? <fieldset disabled>{tree}</fieldset> : tree);
  return { ...utils, qc };
}

beforeEach(() => {
  vi.clearAllMocks();

  vi.mocked(api.useDataApiListServiceAccounts).mockReturnValue(
    { data: { status: 200, data: [SA_1] }, isLoading: false, isError: false } as unknown as
      ReturnType<typeof api.useDataApiListServiceAccounts>,
  );
});

describe("TryIt", () => {
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

  it("preselects the service account named by ?service_account=, per the Access section's contract", async () => {
    const mutateAsync = mockTry(OK);
    renderTryIt({ search: `?service_account=${SA_1.id}` });
    // Nothing to pick — the Select already carries the preselected id, so
    // Run fires immediately with it.
    await userEvent.click(screen.getByRole("button", { name: /run/i }));
    expect(mutateAsync).toHaveBeenCalledWith(
      expect.objectContaining({ data: expect.objectContaining({ service_account_id: SA_1.id }) }),
    );
  });

  it("does not preselect when the URL carries no ?service_account=", async () => {
    mockTry(OK);
    renderTryIt({ search: "" });
    // Nothing preselected means Run has nothing to submit for — it stays
    // disabled rather than silently sending an empty account id.
    expect(screen.getByRole("button", { name: /run/i })).toBeDisabled();
  });

  it("renders one field per declared parameter", () => {
    mockTry(OK);
    renderTryIt({
      endpoint: makeEndpoint({
        params: [
          { name: "region", type: ParamType.string, location: ParamLocation.query, required: true },
          { name: "limit", type: ParamType.number, location: ParamLocation.query, required: false },
        ],
      }),
    });
    expect(screen.getByLabelText(/region/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/limit/i)).toBeInTheDocument();
  });

  it("sends a filled parameter as a one-element list, and omits a blank one", async () => {
    const mutateAsync = mockTry(OK);
    renderTryIt({
      endpoint: makeEndpoint({
        params: [
          { name: "region", type: ParamType.string, location: ParamLocation.query, required: false },
          { name: "limit", type: ParamType.number, location: ParamLocation.query, required: false },
        ],
      }),
    });
    await userEvent.type(screen.getByLabelText(/region/i), "EU");
    await userEvent.click(screen.getByRole("button", { name: /run/i }));
    expect(mutateAsync).toHaveBeenCalledWith(
      expect.objectContaining({ data: expect.objectContaining({ params: { region: ["EU"] } }) }),
    );
  });

  it("states the three consequences a trial has: rate limit, access log, and cache", () => {
    mockTry(OK);
    renderTryIt();
    // Behind the consequence that heads them (Task 3), so this opens the
    // disclosure first: finding the text inside a closed `<details>` would
    // prove the sentence is in the DOM, not that a reader can get to it.
    fireEvent.click(screen.getByText("A trial is a real call, not a dry run"));
    expect(screen.getByText(/rate-limit bucket/i)).toBeInTheDocument();
    expect(screen.getByText(/cannot throttle a live consumer/i)).toBeInTheDocument();
    expect(screen.getByText(/access log/i)).toBeInTheDocument();
    expect(screen.getByText(/Include trial runs/i)).toBeInTheDocument();
    expect(screen.getByText(/per_principal/)).toBeInTheDocument();
  });

  it("treats a 413 as 'too large to display', not a truncated answer", async () => {
    mockTryRejects(new ApiError(413, "payload too large"));
    renderTryIt();
    await userEvent.click(screen.getByRole("button", { name: /run/i }));
    expect(await screen.findByText(/too large to display/i)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /activity/i })).toBeInTheDocument();
    // No stale/truncated result pane left over from a previous run.
    expect(screen.queryByText("Body")).not.toBeInTheDocument();
  });

  it("renders a successful trial's status, body, and echoed request", async () => {
    mockTry(OK);
    renderTryIt();
    await userEvent.click(screen.getByRole("button", { name: /run/i }));
    expect(await screen.findByText("200")).toBeInTheDocument();
    expect(screen.getByText(/"row_count": 1/)).toBeInTheDocument();
    expect(screen.getByText(/GET https:\/\/egret\.test/)).toBeInTheDocument();
    expect(screen.getByText(OK.request.curl)).toBeInTheDocument();
  });
});

/**
 * Final-review finding (Important #4): the account picker collapsed `isError`
 * into the empty case. A failed fetch rendered "This workspace has no service
 * accounts yet. Create one…" while Run sat disabled for want of a selection —
 * a dead end with a false explanation, sending the author off to create an
 * account that already exists. Eight other surfaces in this feature separate
 * the two states; this one did not.
 */
describe("TryIt — the service-account picker distinguishes a failure from an empty workspace", () => {
  function mockAccounts(over: Record<string, unknown>) {
    vi.mocked(api.useDataApiListServiceAccounts).mockReturnValue(
      { data: undefined, isLoading: false, isError: false, error: null, ...over } as unknown as
        ReturnType<typeof api.useDataApiListServiceAccounts>,
    );
  }

  it("reports a failed fetch as a failure, never as 'no service accounts yet'", () => {
    mockAccounts({ isError: true, error: new Error("workspace not found") });
    renderTryIt();
    expect(screen.getByText(/could not load this workspace's service accounts/i)).toBeInTheDocument();
    // The empty-state copy is the specific lie this test exists to forbid.
    expect(screen.queryByText(/has no service accounts yet/i)).not.toBeInTheDocument();
  });

  it("surfaces the real error message, not only a generic sentence", () => {
    mockAccounts({ isError: true, error: new Error("workspace not found") });
    renderTryIt();
    expect(screen.getByText(/workspace not found/)).toBeInTheDocument();
  });

  it("still shows the create-one prompt when the workspace is genuinely empty", () => {
    mockAccounts({ data: { status: 200, data: [] } });
    renderTryIt();
    expect(screen.getByText(/has no service accounts yet/i)).toBeInTheDocument();
    expect(screen.queryByText(/could not load/i)).not.toBeInTheDocument();
  });
});

/**
 * A trial traverses the real serving router, which joins `endpoint_versions`
 * on `live_version_id` (`serving/router.rs`) — it runs the PUBLISHED contract.
 * The parameter fields, though, are built from `endpoint.params`, the DRAFT's
 * list. Rather than guard the section on `dirty` the way Publish is guarded
 * (which would take the tool away exactly while an author is iterating), the
 * section says so.
 */
describe("TryIt — says which version a trial runs", () => {
  it("states that draft edits are not part of what runs", () => {
    renderTryIt();
    expect(screen.getByText(/a trial runs the published version/i)).toBeInTheDocument();
  });
});

/** Two parameters, always. One row cannot tell "wrote the row it was typed in"
 *  apart from "wrote row 0" — see `docs/en/ui-conventions.md` §8. */
const TWO_PARAMS = [
  { name: "region", type: ParamType.string, location: ParamLocation.query, required: true },
  { name: "limit", type: ParamType.number, location: ParamLocation.query, required: false },
];

/**
 * What the trial actually POSTs, not merely that something was POSTed.
 *
 * The pre-existing assertions were `expect.objectContaining`, which pins the
 * one field named and frees every other: `service_account_id` was unpinned in
 * the parameter tests and `params` unpinned in the account test, so a body that
 * dropped either would have passed. These deep-equal the WHOLE call.
 */
describe("TryIt — the payload a trial carries", () => {
  it("puts a typed value under the parameter whose row it was typed in", async () => {
    const mutateAsync = mockTry(OK);
    renderTryIt({ endpoint: makeEndpoint({ params: TWO_PARAMS }) });

    // The SECOND row only. An implementation that writes `params[params[0].name]`
    // — or that reads the first input regardless of which one changed — sends
    // `{ region: ["10"] }` here and dies on the deep equality below. With a
    // one-parameter fixture those two implementations are indistinguishable.
    await userEvent.type(screen.getByLabelText(/^limit$/), "10");
    await userEvent.click(screen.getByRole("button", { name: /^run$/i }));

    await waitFor(() => expect(mutateAsync).toHaveBeenCalled());
    expect(mutateAsync).toHaveBeenCalledTimes(1);
    expect(mutateAsync.mock.calls[0][0]).toEqual({
      workspace: "eng",
      id: "ep-1",
      // The untouched `region` is OMITTED, not sent as `[""]` — an unfilled
      // optional parameter must behave like it was never supplied.
      data: { service_account_id: SA_1.id, params: { limit: ["10"] } },
    });
  });

  it("keeps two filled parameters on their own keys rather than crossing them", async () => {
    const mutateAsync = mockTry(OK);
    renderTryIt({ endpoint: makeEndpoint({ params: TWO_PARAMS }) });

    // Distinct values, so a swap is visible. Neither is a placeholder of the
    // field it is typed into (neither parameter declares a default), so no
    // constant in the component can satisfy this.
    await userEvent.type(screen.getByLabelText(/^region/), "EU");
    await userEvent.type(screen.getByLabelText(/^limit$/), "10");
    await userEvent.click(screen.getByRole("button", { name: /^run$/i }));

    await waitFor(() => expect(mutateAsync).toHaveBeenCalled());
    expect(mutateAsync.mock.calls[0][0].data).toEqual({
      service_account_id: SA_1.id,
      params: { region: ["EU"], limit: ["10"] },
    });
  });
});

/**
 * `react-resizable-panels` is stubbed at the top of this file precisely because
 * its real behaviour cannot be exercised in jsdom. What IS testable, and where
 * the regressions live, is the wiring: which panel each half is in, that the
 * sizes are percent STRINGS rather than v4's pixel numbers, and that the
 * shortcut named in the header actually runs the trial.
 */
describe("TryIt — the split, and the shortcut named in the result header", () => {
  it("splits the section vertically — setup above, result below — and sizes it in PERCENT", () => {
    mockTry(OK);
    renderTryIt();

    const group = screen.getByTestId("panel-group");
    // `orientation`, not `direction` — v4 renamed it, and a wrong prop name is
    // silently accepted and silently horizontal.
    expect(group).toHaveAttribute("data-orientation", "vertical");
    // `min-h-0` is what lets the group give height back; without it the flex
    // item refuses to shrink below its content and the panels grow the page.
    expect(group.className).toMatch(/min-h-0/);

    const panels = screen.getAllByTestId("panel");
    expect(panels).toHaveLength(2);
    // The setup is in the FIRST panel and the result in the SECOND. The Run
    // button lives with the RESULT, as Query's Preview button does.
    expect(within(panels[0]).getByLabelText("Service account")).toBeInTheDocument();
    expect(within(panels[1]).getByRole("button", { name: /^run$/i })).toBeInTheDocument();
    expect(within(panels[0]).queryByRole("button", { name: /^run$/i })).toBeNull();

    // And the handle is between them, not before or after both.
    const handle = screen.getByTestId("panel-separator");
    expect(panels[0].compareDocumentPosition(handle) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(panels[1].compareDocumentPosition(handle) & Node.DOCUMENT_POSITION_PRECEDING).toBeTruthy();

    // STRINGS, and percentages. In react-resizable-panels v4 a bare number is
    // PIXELS — `defaultSize={40}` is a 40-pixel setup pane that reads as
    // correct in the source. The type assertion is the load-bearing half.
    for (const panel of panels) {
      expect(panel).toHaveAttribute("data-default-size-type", "string");
      expect(panel).toHaveAttribute("data-min-size-type", "string");
      expect(panel.getAttribute("data-default-size")).toMatch(/^\d+%$/);
      expect(panel.getAttribute("data-min-size")).toMatch(/^\d+%$/);
    }
    expect(panels[0]).toHaveAttribute("data-default-size", "40%");
    expect(panels[1]).toHaveAttribute("data-default-size", "60%");
  });

  it("names the shortcut in the result panel's header, beside the button it fires", () => {
    mockTry(OK);
    renderTryIt();
    const panels = screen.getAllByTestId("panel");
    // An affordance nobody can see is not an affordance, and the only place a
    // reader looks for it is next to Run.
    expect(within(panels[1]).getByText("⌘↵")).toBeInTheDocument();
  });

  it("runs the trial from ⌘↵, carrying the values on screen", async () => {
    const mutateAsync = mockTry(OK);
    renderTryIt({ endpoint: makeEndpoint({ params: TWO_PARAMS }) });

    await userEvent.type(screen.getByLabelText(/^region/), "EU");
    // The listener is on `window`, so this is the event the product receives —
    // no button is clicked here.
    fireEvent.keyDown(window, { key: "Enter", metaKey: true });

    await waitFor(() => expect(mutateAsync).toHaveBeenCalled());
    // The CURRENT form, not the first render's: a `[]`-dep listener closes over
    // the initial `onRun` and posts an empty `params` with an empty account id.
    expect(mutateAsync.mock.calls[0][0].data).toEqual({
      service_account_id: SA_1.id,
      params: { region: ["EU"] },
    });
  });

  it("does not run from ⌘↵ when no service account is chosen", async () => {
    const mutateAsync = mockTry(OK);
    renderTryIt({ search: "" });
    expect(screen.getByRole("button", { name: /^run$/i })).toBeDisabled();

    fireEvent.keyDown(window, { key: "Enter", metaKey: true });

    // The shortcut bypasses the button, so the button's `disabled` is not the
    // guard — without the check in `onRun` this posts `service_account_id: ""`
    // and the serving path answers a bare 422.
    await waitFor(() => expect(mutateAsync).not.toHaveBeenCalled());
  });

  /**
   * The shell wraps every section in `<fieldset disabled={trashed}>`, which
   * greys the Run button — and a window key handler is not a form control, so
   * it walks straight past it. Before this section had a shortcut, `trashed`
   * did not need reading here at all; adding one without this guard would have
   * made a trashed endpoint runnable by keyboard only.
   */
  it("runs nothing for a trashed endpoint, by shortcut or by button", async () => {
    const mutateAsync = mockTry(OK);
    renderTryIt({ trashed: true });

    expect(screen.getByRole("button", { name: /^run$/i })).toBeDisabled();
    fireEvent.keyDown(window, { key: "Enter", metaKey: true });
    await waitFor(() => expect(mutateAsync).not.toHaveBeenCalled());
  });

  it("keeps what a trial costs in the panel with the button that spends it", () => {
    mockTry(OK);
    renderTryIt();
    const panels = screen.getAllByTestId("panel");
    // `PreviewBlock`'s rule: a reader who has to scroll to find out that this
    // spends a rate limit and writes an access-log row has already pressed the
    // button. jsdom cannot judge that the note is on screen — it can judge that
    // it is in the same panel as Run, which is the wiring the rule reduces to.
    //
    // Task 3 turned the note into a `Caveat`, so the assertion is on the
    // headline — the half that is on screen unclicked — plus the mechanism
    // behind it. Asserting the collapsed text alone would say nothing about
    // which panel a reader finds it in.
    const summary = within(panels[1]).getByText("A trial is a real call, not a dry run");
    expect(summary.closest("details")).toHaveTextContent(/rate-limit bucket/i);
    expect(within(panels[0]).queryByText(/rate-limit bucket/i)).toBeNull();
  });
});

/** §4: settings are `Setting | Details | Value` rows; list-shaped content is a
 *  plain table. This section was eight stacked `Card`s and is now neither. */
describe("TryIt — the setup reads as tables, not as cards", () => {
  it("puts the service-account picker in a config table row rather than loose above the form", () => {
    mockTry(OK);
    renderTryIt();

    const trigger = screen.getByLabelText("Service account");
    const cell = trigger.closest("td");
    expect(cell).not.toBeNull();
    const row = cell!.closest("tr")!;
    // The label is the row's Setting cell, not a `<Label>` floating above a card.
    expect(within(row).getByText("Service account")).toBeInTheDocument();
  });

  it("gives each declared parameter one row of a plain table, with its type", () => {
    mockTry(OK);
    renderTryIt({ endpoint: makeEndpoint({ params: TWO_PARAMS }) });

    const regionRow = screen.getByLabelText(/^region/).closest("tr")!;
    const limitRow = screen.getByLabelText(/^limit$/).closest("tr")!;
    // Two rows, and DIFFERENT ones — one row cannot tell a per-row renderer
    // apart from one that renders row 0 twice.
    expect(regionRow).not.toBe(limitRow);
    expect(within(regionRow).getByText("string")).toBeInTheDocument();
    expect(within(limitRow).getByText("number")).toBeInTheDocument();
    // The required marker belongs to the required one, and only to it.
    expect(within(regionRow).getByText("*")).toBeInTheDocument();
    expect(within(limitRow).queryByText("*")).toBeNull();
  });

  it("says so inside the table when the endpoint declares no parameters", () => {
    mockTry(OK);
    renderTryIt();
    const empty = screen.getByText(/declares no parameters/i);
    // In the table's own body, spanning it, rather than a sentence that reads as
    // a value in the Parameter column.
    expect(empty.closest("td")).toHaveAttribute("colspan", "3");
  });
});

/**
 * The one thing this section exists to say. A trial runs as the chosen SERVICE
 * ACCOUNT; `Query`'s preview runs as the AUTHOR. Measured live on kind: the same
 * endpoint and the same SQL answered `who=svc-kind-smoke-bot` from a trial and
 * `who=admin` from a preview. `PreviewBlock` carries the claim from the other
 * side; these are the three places on this side.
 */
describe("TryIt — says whose identity a trial runs as", () => {
  it("says it in the section description, in the picker's help, and beside Run", () => {
    mockTry(OK);
    renderTryIt();

    expect(screen.getByText(/as a chosen service account/i)).toBeInTheDocument();
    // The second and third places are behind their own disclosures now (Task
    // 3), so this OPENS them: text found inside a closed `<details>` proves the
    // sentence is in the DOM, not that a reader can reach it.
    const picker = screen.getByText("A green preview does not prove a consumer can call this endpoint");
    fireEvent.click(picker);
    expect(picker.closest("details")).toHaveAttribute("open");
    expect(within(picker.closest("details")!).getByText(/Preview in Query runs as you/i)).toBeInTheDocument();

    const run = screen.getByText("A trial is a real call, not a dry run");
    fireEvent.click(run);
    expect(run.closest("details")).toHaveAttribute("open");
    expect(run.closest("details")).toHaveTextContent(/runs as the service account above rather than as you/i);
  });
});

/**
 * `Try it` carries `max-w-5xl` and, until this test, nothing held it there.
 * `Versions` and `Response` each had a guard saying they cap themselves and
 * `Config` had the inverse, so this section and `Access` were the two that
 * could lose (or grow) a cap in silence — the same hole
 * `docs/en/ui-conventions.md` §5.5 records for `Config`, where restoring
 * `max-w-3xl` to the section's own root passed 230/230.
 *
 * The walk lives in `sectionWidth.ts` rather than being written a fourth time.
 * It resolves the host, so it works both under the real `EndpointDetail` shell
 * (`AccessSection.test.tsx`, `EndpointDetail.test.tsx`) and under this file's
 * stubbed-context harness, where the section's root is a child of the render
 * container itself.
 *
 * A class name, not a layout: jsdom has no CSS (§10). Whether the split reads
 * well at that width is the browser pass's question — and in #529 the browser
 * was what caught an editor squeezed to 107px that every test had passed.
 */
describe("TryIt — width", () => {
  it("caps its own width, as §5.1 says this section does", () => {
    mockTry(OK);
    const { container } = renderTryIt();
    const heading = screen.getByRole("heading", { name: "Try it", level: 2 });
    expectSectionCapsItsOwnWidth(container, heading);
  });
});

/**
 * Task 3 — prose moves beside the control it describes.
 *
 * The audit measured ~950 characters of standing prose on this screen against
 * one dropdown and one button. Nothing here was deleted: the rule applied is
 * `docs/en/ui-conventions.md` §4.4 — **the consequence in the summary, the
 * reason behind it** — so every sentence is still on the page and every one of
 * them is still reachable without leaving it.
 *
 * These tests assert REACHABILITY, not presence. jsdom does not hide a closed
 * `<details>` (§11.3), so `getByText` finds a sentence a user cannot see; the
 * only observable that distinguishes the two is the `open` attribute plus the
 * activation that sets it.
 */
describe("TryIt — the prose sits beside the control it describes", () => {
  /** The `<details>` that contains `text`, asserted to be genuinely collapsed. */
  function collapsedDisclosureAround(el: HTMLElement): HTMLDetailsElement {
    const details = el.closest("details") as HTMLDetailsElement | null;
    expect(details).not.toBeNull();
    expect(details).not.toHaveAttribute("open");
    return details!;
  }

  it("keeps the Service account Details cell to one sentence and the rest beside the picker", () => {
    mockTry(OK);
    renderTryIt();

    // The Details cell itself. `ConfigTableRow` renders its help text TWICE —
    // once in the Details column at `lg`, once under the label below it — so
    // this counts both rather than asserting a single node.
    const kept = screen.getAllByText("The identity the trial authenticates as.");
    expect(kept.length).toBeGreaterThan(0);
    // The rule §4.4 caps at about two lines. The failing shape is the 314-char
    // literal this replaced, so assert the CELL is short rather than that some
    // node somewhere holds the short sentence.
    for (const node of kept) {
      expect(node.textContent!.length).toBeLessThan(120);
    }

    // …and the two sentences that were cut from it are still here, behind a
    // disclosure whose summary carries the consequence.
    const summary = screen.getByText("A green preview does not prove a consumer can call this endpoint");
    const details = collapsedDisclosureAround(summary);
    const body = within(details).getByText(/60-second token is minted/i);
    expect(body.closest("summary")).toBeNull();
    expect(body).toHaveTextContent(/Preview in Query runs as you instead/i);

    // Reachable: the gesture a user makes opens it.
    fireEvent.click(summary);
    expect(details).toHaveAttribute("open");
  });

  it("puts the trial's consequences behind the consequence, in the panel that holds Run", () => {
    mockTry(OK);
    renderTryIt();

    const resultPanel = screen.getAllByTestId("panel")[1];
    // The load-bearing half, on screen with nothing clicked. `PreviewBlock`'s
    // rule survives the move: a reader who must open something to learn a call
    // is real has already pressed the button, so THAT sentence is the summary.
    const summary = within(resultPanel).getByText("A trial is a real call, not a dry run");
    const details = collapsedDisclosureAround(summary);
    expect(within(resultPanel).getByRole("button", { name: /^run$/i })).toBeInTheDocument();

    // Every consequence still reachable, none of it standing.
    for (const re of [/rate-limit bucket/i, /cannot throttle a live consumer/i, /access log/i, /Include trial runs/i, /per_principal/]) {
      const node = within(details).getByText(re);
      expect(node.closest("summary")).toBeNull();
    }

    fireEvent.click(summary);
    expect(details).toHaveAttribute("open");
  });

  /**
   * §11.2's trap, on this section for the first time. The shell wraps the body
   * in `<fieldset disabled={trashed}>`; a Radix `Collapsible` trigger is a
   * `<button>` and would be dead there, and every text assertion above would
   * still pass because jsdom does not hide a closed disclosure either way.
   *
   * `fireEvent`, not `userEvent`: `userEvent.click` refuses a click under a
   * disabled fieldset even though `fieldset[disabled]` disables only
   * form-associated descendants and `<summary>` is not one. The failure mode is
   * a FALSE NEGATIVE.
   */
  it("keeps both disclosures openable inside a disabled fieldset", () => {
    mockTry(OK);
    renderTryIt({ trashed: true, fieldset: true });

    for (const label of [
      "A green preview does not prove a consumer can call this endpoint",
      "A trial is a real call, not a dry run",
    ]) {
      const summary = screen.getByText(label);
      // Guard the premise: the fieldset really is disabled, so this is a test
      // about surviving it and not one where nothing was disabled.
      const fieldset = summary.closest("fieldset");
      expect(fieldset).not.toBeNull();
      expect(fieldset).toBeDisabled();

      const details = summary.closest("details")!;
      // A `ui/collapsible` renders a `<button>` here, which the fieldset kills.
      expect(within(details).queryByRole("button")).not.toBeInTheDocument();
      expect(details).not.toHaveAttribute("open");
      fireEvent.click(summary);
      expect(details).toHaveAttribute("open");
    }
  });

  /**
   * The counter-rule, and the reason this section did not collapse everything.
   * §11.2: collapse a caveat because it is BIG, not because its neighbour is.
   * Two lines with no explanation to hold costs a click to reveal nothing.
   */
  it("leaves the short published-version note as plain text, not as an empty disclosure", () => {
    mockTry(OK);
    renderTryIt();
    const note = screen.getByText(/a trial runs the published version/i);
    expect(note.closest("details")).toBeNull();
  });
});
