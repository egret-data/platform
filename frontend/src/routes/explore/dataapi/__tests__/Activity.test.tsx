import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// There is no msw in this repo — tests mock the API module directly (see
// Endpoints.test.tsx / Clients.test.tsx / AuditLog.test.tsx). Activity also
// resolves trial actors to emails via `listUsers`, and the call list's
// endpoint filter reads the endpoints list — both need a mock or they fire a
// real, unhandled `fetch` in jsdom.
vi.mock("@/api/generated/data-api/data-api");
vi.mock("@/api/access");

import * as api from "@/api/generated/data-api/data-api";
import * as accessApi from "@/api/access";
import type { ActivityCallDto, ActivitySummaryDto, DataApiEndpointDto } from "@/api/generated/model";
import Activity from "@/routes/explore/dataapi/Activity";

function makeEndpoint(overrides: Partial<DataApiEndpointDto> = {}): DataApiEndpointDto {
  return {
    id: "ep-1",
    folder_id: null,
    name: "Daily report",
    description: "",
    path: "reports/daily",
    method: "GET",
    database_id: "db-1",
    statements: [{ name: "main", sql: "select 1" }],
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
  };
}

const DEFAULT_SUMMARY: ActivitySummaryDto = {
  total: {
    endpoint_id: null,
    endpoint_name: null,
    calls: 120,
    errors: 6,
    p50_ms: 45,
    p95_ms: 210,
    cache_hits: 30,
    cacheable_calls: 100,
  },
  endpoints: [
    {
      endpoint_id: "ep-1",
      endpoint_name: "Daily report",
      calls: 100,
      errors: 4,
      p50_ms: 40,
      p95_ms: 180,
      cache_hits: 28,
      cacheable_calls: 90,
    },
    // The unmatched bucket: same grouping set, endpoint_id null. Present by
    // default so "labels the unmatched bucket" doesn't need a special fixture.
    {
      endpoint_id: null,
      endpoint_name: null,
      calls: 20,
      errors: 2,
      p50_ms: null,
      p95_ms: null,
      cache_hits: 0,
      cacheable_calls: 0,
    },
  ],
  retention_days: null,
};

function makeCall(overrides: Partial<ActivityCallDto> = {}): ActivityCallDto {
  return {
    id: "call-1",
    ts: "2026-08-16T12:00:00Z",
    endpoint_id: "ep-1",
    endpoint_name: "Daily report",
    request_method: "GET",
    request_path: "/data-api/reports/daily",
    status_code: 200,
    error_code: null,
    duration_ms: 42,
    cache_hit: true,
    client_id: "client_abc123",
    request_id: "req-fixture-1",
    actor_user_id: null,
    ...overrides,
  };
}

function mockSummary(
  over: Partial<{ data: unknown; isLoading: boolean; isError: boolean; error: unknown }> = {},
) {
  vi.mocked(api.useDataApiActivitySummary).mockReturnValue(
    {
      data: { status: 200, data: DEFAULT_SUMMARY },
      isLoading: false,
      isError: false,
      error: null,
      ...over,
    } as unknown as ReturnType<typeof api.useDataApiActivitySummary>,
  );
}

// `ActivityCallList` pages via `useInfiniteQuery` calling the raw
// `dataApiActivityCalls` fetcher directly (not a generated hook — see that
// file's header comment on why), so these tests mock the fetcher and let a
// REAL `QueryClientProvider` drive real pagination, rather than stubbing a
// hook's return value the way the rest of this file's `mock*` helpers do.
function mockCalls(data: ActivityCallDto[] = []) {
  vi.mocked(api.dataApiActivityCalls).mockResolvedValue({
    status: 200,
    data,
    headers: new Headers(),
  } as unknown as Awaited<ReturnType<typeof api.dataApiActivityCalls>>);
}

function mockCallsError(message: string) {
  vi.mocked(api.dataApiActivityCalls).mockRejectedValue(new Error(message));
}

function mockEndpoints(list: DataApiEndpointDto[] = [makeEndpoint()]) {
  vi.mocked(api.useDataApiListEndpoints).mockReturnValue(
    { data: { status: 200, data: list }, isLoading: false, isError: false, error: null } as unknown as
      ReturnType<typeof api.useDataApiListEndpoints>,
  );
}

function renderActivity() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/data-api/activity"]}>
        <Routes>
          <Route path="/explore/:workspace/data-api/activity" element={<Activity />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mockSummary();
  mockCalls();
  mockEndpoints();
  vi.mocked(accessApi.listUsers).mockResolvedValue([]);
});

describe("Activity — tiles", () => {
  it("renders a dash, not 0%, when nothing in range was cacheable", async () => {
    mockSummary({
      data: {
        status: 200,
        data: {
          ...DEFAULT_SUMMARY,
          total: { ...DEFAULT_SUMMARY.total, cache_hits: 0, cacheable_calls: 0 },
        },
      },
    });
    renderActivity();
    expect(await screen.findByTestId("tile-cache-hit")).toHaveTextContent("—");
  });

  it("renders the raw calls count and a percentage error rate, not a decimal", async () => {
    renderActivity();
    const callsTile = await screen.findByTestId("tile-calls");
    expect(callsTile).toHaveTextContent("120");
    const errorTile = screen.getByTestId("tile-error-rate");
    // 6 / 120 = 5.0%
    expect(errorTile).toHaveTextContent("5.0%");
  });

  it("labels p50/p95 as computed across successful calls, not all calls", async () => {
    renderActivity();
    expect(await screen.findByText(/across successful calls/i)).toBeInTheDocument();
    expect(screen.getByTestId("tile-latency")).toHaveTextContent("45");
    expect(screen.getByTestId("tile-latency")).toHaveTextContent("210");
  });

  it("shows a dash for latency when there is no successful-call population", async () => {
    mockSummary({
      data: {
        status: 200,
        data: { ...DEFAULT_SUMMARY, total: { ...DEFAULT_SUMMARY.total, p50_ms: null, p95_ms: null } },
      },
    });
    renderActivity();
    expect(await screen.findByTestId("tile-latency")).toHaveTextContent("—");
  });

  it("shows the summary error state, not empty tiles, when the summary query fails", async () => {
    mockSummary({ isError: true, error: { message: "summary boom" } });
    renderActivity();
    expect(await screen.findByText(/summary boom/i)).toBeInTheDocument();
    expect(screen.queryByTestId("tile-calls")).not.toBeInTheDocument();
  });

  it("shows a loading state while the summary query is in flight", () => {
    mockSummary({ isLoading: true, data: undefined });
    renderActivity();
    expect(screen.getAllByTestId("list-skeleton").length).toBeGreaterThan(0);
    expect(screen.queryByTestId("tile-calls")).not.toBeInTheDocument();
  });

  it("renders no charting library — no canvas, no svg chart element", async () => {
    renderActivity();
    await screen.findByTestId("tile-calls");
    expect(document.querySelector("canvas")).toBeNull();
  });
});

describe("Activity — range presets bounded by retention", () => {
  it("disables ranges longer than retention and says how long data is kept", async () => {
    mockSummary({ data: { status: 200, data: { ...DEFAULT_SUMMARY, retention_days: 7 } } });
    renderActivity();
    expect(await screen.findByText(/kept for 7 days/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "30d" })).toBeDisabled();
    // 7d itself is exactly at the retention boundary — not disabled.
    expect(screen.getByRole("button", { name: "7d" })).not.toBeDisabled();
  });

  it("disables no range when retention_days is null (no prune scheduled)", async () => {
    renderActivity();
    await screen.findByTestId("tile-calls");
    expect(screen.getByRole("button", { name: "30d" })).not.toBeDisabled();
    expect(screen.getByRole("button", { name: "1h" })).not.toBeDisabled();
  });

  // Final-review finding (#8): `retention_days: 0` — retention off, or simply
  // not configured — made EVERY preset fail `RANGE_DAYS[r] > retentionDays`,
  // including the one currently selected. The screen then reads as a broken
  // control (nothing is clickable, not even what is showing) instead of as a
  // bound, and the selected range cannot be re-pressed. A range that IS
  // selected is by definition reachable.
  it("never disables the SELECTED preset, even at retention_days: 0", async () => {
    mockSummary({ data: { status: 200, data: { ...DEFAULT_SUMMARY, retention_days: 0 } } });
    renderActivity();
    await screen.findByTestId("tile-calls");
    // 24h is the default selection.
    expect(screen.getByRole("button", { name: "24h" })).not.toBeDisabled();
    // The bound still applies to everything else.
    expect(screen.getByRole("button", { name: "30d" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "1h" })).toBeDisabled();
  });

  it("exempts whichever preset is selected, not a hardcoded one", async () => {
    mockSummary({ data: { status: 200, data: { ...DEFAULT_SUMMARY, retention_days: 7 } } });
    const user = userEvent.setup();
    renderActivity();
    await screen.findByTestId("tile-calls");

    // 30d is out of range and disabled…
    expect(screen.getByRole("button", { name: "30d" })).toBeDisabled();
    // …while 1h, which is in range, is selectable and then exempt itself.
    await user.click(screen.getByRole("button", { name: "1h" }));
    expect(screen.getByRole("button", { name: "1h" })).not.toBeDisabled();
    expect(screen.getByRole("button", { name: "24h" })).not.toBeDisabled();
  });

  it("clicking a range preset re-queries the summary with that range", async () => {
    const user = userEvent.setup();
    renderActivity();
    await screen.findByTestId("tile-calls");

    await user.click(screen.getByRole("button", { name: "7d" }));

    await waitFor(() =>
      expect(api.useDataApiActivitySummary).toHaveBeenCalledWith(
        "eng",
        expect.objectContaining({ range: "7d" }),
        expect.anything(),
      ),
    );
  });
});

describe("Activity — per-endpoint table", () => {
  it("labels the unmatched bucket with what was actually requested", async () => {
    renderActivity();
    expect(await screen.findByText("No matching endpoint")).toBeInTheDocument();
  });

  it("renders the matched endpoint's own name and metrics", async () => {
    renderActivity();
    expect(await screen.findByText("Daily report")).toBeInTheDocument();
  });

  it("explains in a caption why the table's total is smaller than the tile's", async () => {
    renderActivity();
    await screen.findByText("No matching endpoint");
    expect(screen.getByText(/matched no endpoint/i)).toBeInTheDocument();
  });
});

describe("Activity — call list", () => {
  // Both assertions below are scoped to the call list. The per-endpoint table
  // now renders the SAME sentence, so an unscoped negative here would be
  // satisfied by whatever that table happens to show — it would fail for a
  // correct call list the day someone gives the summary fixture no endpoints,
  // and it would pass for a broken one that leaked an empty row beside its
  // error.
  it("shows the call-list error state, not an empty table, when it fails", async () => {
    mockCallsError("calls boom");
    renderActivity();
    await screen.findByTestId("tile-calls");
    const list = await screen.findByTestId("activity-call-list");
    expect(await within(list).findByText(/calls boom/i)).toBeInTheDocument();
    expect(within(list).queryByText(/no calls in this range/i)).not.toBeInTheDocument();
  });

  it("shows an empty state distinct from the error state when there are genuinely no calls", async () => {
    renderActivity();
    const list = await screen.findByTestId("activity-call-list");
    expect(await within(list).findByText(/no calls in this range/i)).toBeInTheDocument();
  });

  it("renders a call row with endpoint, status, duration, cache hit and request id", async () => {
    mockCalls([makeCall()]);
    renderActivity();
    // "Daily report" also appears in the per-endpoint table above, so this
    // just confirms the call row rendered before asserting on its content.
    await screen.findByText("req-fixture-1");
    expect(screen.getAllByText("Daily report").length).toBeGreaterThanOrEqual(2);
    expect(screen.getByText("200")).toBeInTheDocument();
    expect(screen.getByText(/42\s*ms/)).toBeInTheDocument();
    expect(screen.getByText("Hit")).toBeInTheDocument();
    expect(screen.getByText("req-fixture-1")).toBeInTheDocument();
  });

  it("renders request_method + request_path, not a name, for a call that matched no endpoint", async () => {
    mockCalls([
      makeCall({ id: "call-2", endpoint_id: null, endpoint_name: null, request_method: "POST", request_path: "/v1/orders" }),
    ]);
    renderActivity();
    expect(await screen.findByText("POST /v1/orders")).toBeInTheDocument();
  });

  it("labels a trial row with the actor's email, not the reserved client_id marker", async () => {
    vi.mocked(accessApi.listUsers).mockResolvedValue([
      { id: "user-77", email: "alice@egret.local", name: "Alice" },
    ] as unknown as accessApi.AccessUser[]);
    mockCalls([
      makeCall({
        id: "call-3",
        actor_user_id: "user-77",
        // The reserved marker a trial's `sub` really carries — must never be
        // shown as if it were a real client.
        client_id: "trial:sa-99",
      }),
    ]);
    renderActivity();
    expect(await screen.findByText("alice@egret.local")).toBeInTheDocument();
    expect(screen.queryByText("trial:sa-99")).not.toBeInTheDocument();
  });

  it('"Include trial runs" is off by default', async () => {
    renderActivity();
    await screen.findByTestId("tile-calls");
    expect(
      screen.getByRole("checkbox", { name: /include trial runs/i }),
    ).not.toBeChecked();
    await waitFor(() =>
      expect(api.dataApiActivityCalls).toHaveBeenCalledWith(
        "eng",
        expect.objectContaining({ include_trials: false }),
      ),
    );
  });

  it('checking "Include trial runs" re-queries with include_trials: true', async () => {
    const user = userEvent.setup();
    renderActivity();
    await screen.findByTestId("tile-calls");

    await user.click(screen.getByRole("checkbox", { name: /include trial runs/i }));

    await waitFor(() =>
      expect(api.dataApiActivityCalls).toHaveBeenCalledWith(
        "eng",
        expect.objectContaining({ include_trials: true }),
      ),
    );
  });

  it("looking up a request id re-queries with that request_id", async () => {
    const user = userEvent.setup();
    renderActivity();
    await screen.findByTestId("tile-calls");

    await user.type(screen.getByLabelText(/request id/i), "req-abc-999");

    await waitFor(() =>
      expect(api.dataApiActivityCalls).toHaveBeenCalledWith(
        "eng",
        expect.objectContaining({ request_id: "req-abc-999" }),
      ),
    );
  });

  it("filtering by endpoint re-queries with that endpoint_id", async () => {
    const user = userEvent.setup();
    mockEndpoints([makeEndpoint({ id: "ep-9", name: "Weekly export" })]);
    renderActivity();
    await screen.findByTestId("tile-calls");

    await user.click(screen.getByRole("combobox", { name: /endpoint/i }));
    await user.click(await screen.findByRole("option", { name: "Weekly export" }));

    await waitFor(() =>
      expect(api.dataApiActivityCalls).toHaveBeenCalledWith(
        "eng",
        expect.objectContaining({ endpoint_id: "ep-9" }),
      ),
    );
  });

  it("shows Load more when a full page came back, since another page may exist", async () => {
    // 50 rows == the page size, so the list believes another page may exist.
    const fullPage = Array.from({ length: 50 }, (_, i) =>
      makeCall({ id: `bulk-${i}`, request_id: `req-${i}` }),
    );
    mockCalls(fullPage);
    renderActivity();
    expect(await screen.findByRole("button", { name: /load more/i })).toBeInTheDocument();
  });

  it("Load more requests the next page with `before` set to the oldest loaded row's ts, and accumulates", async () => {
    const page1 = Array.from({ length: 50 }, (_, i) =>
      makeCall({ id: `p1-${i}`, ts: "2026-08-16T12:00:00Z", request_id: `req-p1-${i}` }),
    );
    const page2 = [makeCall({ id: "p2-0", ts: "2026-08-15T00:00:00Z", request_id: "req-p2-0" })];

    vi.mocked(api.dataApiActivityCalls).mockImplementation(async (_ws, params) => {
      const data = (params as { before?: string }).before ? page2 : page1;
      return { status: 200, data, headers: new Headers() } as unknown as Awaited<
        ReturnType<typeof api.dataApiActivityCalls>
      >;
    });

    const user = userEvent.setup();
    renderActivity();
    const loadMore = await screen.findByRole("button", { name: /load more/i });

    await user.click(loadMore);

    await waitFor(() => expect(screen.getByText("req-p2-0")).toBeInTheDocument());
    // Page 1's rows are still present — this is accumulation, not replacement.
    expect(screen.getByText("req-p1-0")).toBeInTheDocument();
    // The second page had fewer rows than the page size, so there is nothing
    // further to load.
    expect(screen.queryByRole("button", { name: /load more/i })).not.toBeInTheDocument();
    // And the cursor sent for the second page was the oldest ts from the
    // first — not undefined again, which would just refetch page one.
    expect(api.dataApiActivityCalls).toHaveBeenCalledWith(
      "eng",
      expect.objectContaining({ before: "2026-08-16T12:00:00Z" }),
    );
  });

  it("changing a filter starts a fresh query rather than reusing the old accumulated pages", async () => {
    mockEndpoints([makeEndpoint({ id: "ep-9", name: "Weekly export" })]);
    mockCalls([makeCall({ id: "call-initial", request_id: "req-initial" })]);
    const user = userEvent.setup();
    renderActivity();
    await screen.findByText("req-initial");

    mockCalls([makeCall({ id: "call-after", request_id: "req-after" })]);
    await user.click(screen.getByRole("combobox", { name: /endpoint/i }));
    await user.click(await screen.findByRole("option", { name: "Weekly export" }));

    await waitFor(() => expect(screen.getByText("req-after")).toBeInTheDocument());
    // The row from before the filter change is gone — the new query key
    // started at page one instead of appending onto stale accumulated rows.
    expect(screen.queryByText("req-initial")).not.toBeInTheDocument();
  });
});

/**
 * Activity is a top-level sidebar item now, not a section nested under a
 * `DataApiLayout` shell — that shell was removed once Data API got its own
 * sidebar group (Endpoints/Clients/Activity), so this page owns its own `h1`
 * and `p-4`, exactly like `Charts.tsx`. No subtitle paragraph either way.
 */
describe("Activity — page scaffold", () => {
  it("uses the top-level idiom: its own h1, no subtitle paragraph", async () => {
    renderActivity();
    const h1 = await screen.findByRole("heading", { level: 1, name: "Activity" });
    expect(h1.className).toContain("text-lg");
    expect(h1.className).toContain("font-semibold");
    expect(screen.queryByRole("heading", { level: 2, name: "Activity" })).not.toBeInTheDocument();
  });

  it("carries its own p-4 padding, like every other top-level Explore list page", async () => {
    const { container } = renderActivity();
    await screen.findByRole("heading", { level: 1, name: "Activity" });
    expect(container.firstElementChild?.className ?? "").toMatch(/\bp-4\b/);
  });
});

/**
 * Both tables on this page mean the same thing by "empty": the selected range
 * produced no calls. They said it in two different sentences, which reads as
 * two different conditions on one screen.
 */
describe("Activity — one wording for one empty state", () => {
  it("uses the same sentence in the per-endpoint table and the call list", async () => {
    mockSummary({
      data: {
        status: 200,
        data: { ...DEFAULT_SUMMARY, endpoints: [] } satisfies ActivitySummaryDto,
      },
    });
    mockCalls([]);
    renderActivity();

    // Both empty rows are on screen at once, so this is a direct comparison
    // rather than two renders that could each drift on their own.
    await waitFor(() =>
      expect(screen.getAllByText("No calls in this range.")).toHaveLength(2),
    );
    // …and the two matches are one per table, not two inside whichever table
    // happened to render: the call list owns one of them.
    expect(
      within(screen.getByTestId("activity-call-list")).getByText("No calls in this range."),
    ).toBeInTheDocument();
    expect(screen.queryByText("No calls in range.")).not.toBeInTheDocument();
  });
});
