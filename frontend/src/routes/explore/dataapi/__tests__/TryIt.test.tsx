import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Outlet, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// No msw in this repo — tests mock the API module directly (see
// AccessSection.test.tsx / EndpointDetail.test.tsx).
vi.mock("@/api/generated/data-api/data-api");

import * as api from "@/api/generated/data-api/data-api";
import { ApiError } from "@/api/client";
import { ParamLocation, ParamType, type DataApiEndpointDto, type DataApiServiceAccountDto } from "@/api/generated/model";
import type { EndpointDetailContext } from "@/routes/explore/dataapi/EndpointDetail";
import TryIt from "@/routes/explore/dataapi/detail/TryIt";

function makeEndpoint(overrides: Partial<DataApiEndpointDto> = {}): DataApiEndpointDto {
  return {
    id: "ep-1",
    folder_id: null,
    name: "Daily report",
    description: "",
    path: "reports/daily",
    method: "GET",
    database_id: "db-1",
    sql: "SELECT 1",
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

function renderTryIt(opts: { endpoint?: DataApiEndpointDto; search?: string } = {}) {
  const endpoint = opts.endpoint ?? makeEndpoint();
  const search = opts.search ?? `?service_account=${SA_1.id}`;
  const ctx: Partial<EndpointDetailContext> = { ws: "eng", id: endpoint.id, endpoint };

  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const utils = render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/try${search}`]}>
        <Routes>
          <Route path="/try" element={<ContextProvider ctx={ctx} />}>
            <Route index element={<TryIt />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
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
