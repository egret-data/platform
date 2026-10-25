import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// There is no msw in this repo — tests mock the API module directly (see
// Endpoints.test.tsx / Clients.test.tsx / AuditLog.test.tsx). The call list's
// endpoint filter reads the endpoints list — it needs a mock or it fires a
// real, unhandled `fetch` in jsdom.
vi.mock("@/api/generated/data-api/data-api");

import * as api from "@/api/generated/data-api/data-api";
import type {
  ActivityCallDto,
  ActivityRow,
  ActivitySummaryDto,
  DataApiEndpointDto,
} from "@/api/generated/model";
import Activity from "@/routes/explore/dataapi/Activity";
import { expectChipTone } from "./expectChipTone";

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

/**
 * A range that produced nothing. Not `DEFAULT_SUMMARY` with `endpoints: []` —
 * that fixture keeps `total.calls: 120`, which is a state the backend cannot
 * produce (the grand total and the per-endpoint rows come out of ONE
 * `GROUPING SETS` query) and which would let a page that only ever consults
 * `endpoints` pass for one that consults both.
 */
const EMPTY_SUMMARY: ActivitySummaryDto = {
  total: {
    endpoint_id: null,
    endpoint_name: null,
    calls: 0,
    errors: 0,
    p50_ms: null,
    p95_ms: null,
    cache_hits: 0,
    cacheable_calls: 0,
  },
  endpoints: [],
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
function mockEmptySummary(over: Partial<ActivitySummaryDto> = {}) {
  mockSummary({ data: { status: 200, data: { ...EMPTY_SUMMARY, ...over } } });
}

function mockCalls(data: ActivityCallDto[] = []) {
  vi.mocked(api.dataApiActivityCalls).mockResolvedValue({
    status: 200,
    data,
    headers: new Headers(),
  } as unknown as Awaited<ReturnType<typeof api.dataApiActivityCalls>>);
}

/** Rows only when the caller asked for trials. A mock that returns the same
 * array either way cannot tell "the toggle reached the server" from "the row
 * was always going to render". */
function mockTrialOnlyCalls(trials: ActivityCallDto[]) {
  vi.mocked(api.dataApiActivityCalls).mockImplementation(async (_ws, params) => {
    const asked = (params as { include_trials?: boolean }).include_trials === true;
    return { status: 200, data: asked ? trials : [], headers: new Headers() } as unknown as Awaited<
      ReturnType<typeof api.dataApiActivityCalls>
    >;
  });
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

  // KI-296: the actor is named by the server, so a user who may not list the platform's users
  // sees a name, not a shortened id.
  it("labels a trial row with the actor's name, not the reserved client_id marker (KI-296)", async () => {
    mockCalls([
      makeCall({
        id: "call-3",
        actor_user_id: "user-77",
        actor_name: "alice@egret.local",
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
 * Activity is a top-level sidebar item, not a section nested under a
 * `DataApiLayout` shell — that shell was removed once Data API got its own
 * sidebar group (Endpoints/Clients/Activity), so the `h1` and the `p-4` belong
 * to this route. They come from `ListPageShell` (§1.1), which is how that
 * ownership is discharged rather than a departure from it; these assertions
 * are unchanged from when the page hand-wrote them, because the invariant they
 * pin was never about which file emits the element. No subtitle paragraph.
 */
describe("Activity — page scaffold", () => {
  it("uses the top-level idiom: exactly one h1, no subtitle paragraph", async () => {
    renderActivity();
    const h1 = await screen.findByRole("heading", { level: 1, name: "Activity" });
    expect(h1.className).toContain("text-xl"); // PageTitle, FE-PAGE-7
    expect(h1.className).toContain("font-semibold");
    // §1.1 is that the shell owns THE SINGLE `h1`, so counting is the actual
    // rule: a page that kept a hand-written heading beside the shell's would
    // satisfy `findByRole` and every class assertion above it.
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    expect(screen.queryByRole("heading", { level: 2, name: "Activity" })).not.toBeInTheDocument();
  });

  it("carries its own p-4 padding, like every other top-level Explore list page", async () => {
    const { container } = renderActivity();
    await screen.findByRole("heading", { level: 1, name: "Activity" });
    expect(container.firstElementChild?.className ?? "").toMatch(/\bp-4\b/);
  });
});

/**
 * The five steps §11.3 requires of a reachability assertion, in one place.
 *
 * `fireEvent.click` followed by `getByText` is a presence test with ceremony in
 * front of it — jsdom does no layout, so the sentence is in the document
 * whether or not a reader can reach it. What says something is: the element is
 * a native `<details>` (a Radix collapsible has no `<summary>` to find and no
 * `open` to read), it is CLOSED on arrival, the explanation is not inside the
 * summary, and the click is what changes `open`.
 */
function openCaveat(summaryText: HTMLElement, bodyPattern: RegExp): HTMLDetailsElement {
  const summaryEl = summaryText.closest("summary");
  expect(summaryEl).not.toBeNull();
  const details = summaryEl!.closest("details") as HTMLDetailsElement | null;
  expect(details).not.toBeNull();
  expect(details!.tagName).toBe("DETAILS");

  expect(details!.open).toBe(false);
  const body = screen.getByText(bodyPattern);
  // The explanation is the BODY, not a second line of the headline.
  expect(summaryEl!.contains(body)).toBe(false);

  fireEvent.click(summaryEl!);

  expect(details!.open).toBe(true);
  expect(details!.contains(body)).toBe(true);
  return details!;
}

describe("Activity — the two footnotes sit behind an info affordance", () => {
  it("keeps the retention FACT on screen and its reason one click away, beside the range strip", async () => {
    mockSummary({ data: { status: 200, data: { ...DEFAULT_SUMMARY, retention_days: 7 } } });
    renderActivity();

    const headline = await screen.findByText(/kept for 7 days/i);
    const details = openCaveat(headline, /thinning tail/i);

    // "Beside the control it describes": inside the range picker, not floating
    // full-width under the page.
    expect(screen.getByTestId("activity-range").contains(details)).toBe(true);
  });

  it("keeps the per-endpoint caveat beside that table, headed by the consequence", async () => {
    renderActivity();
    await screen.findByText("No matching endpoint");

    const headline = screen.getByText(/sum to less than the total above/i);
    const details = openCaveat(headline, /matched no endpoint/i);

    expect(screen.getByTestId("activity-per-endpoint").contains(details)).toBe(true);
  });

  it("opens neither disclosure on arrival — the page is not two paragraphs of prose", async () => {
    mockSummary({ data: { status: 200, data: { ...DEFAULT_SUMMARY, retention_days: 7 } } });
    const { container } = renderActivity();
    await screen.findByText("No matching endpoint");

    const all = Array.from(container.querySelectorAll("details"));
    expect(all).toHaveLength(2);
    expect(all.every((d) => !(d as HTMLDetailsElement).open)).toBe(true);
  });
});

/**
 * A range with no calls used to be four metric cards each showing an em dash,
 * a per-endpoint table showing its own empty text, a call list showing
 * DIFFERENT empty text, and every filter rendered over nothing.
 */
describe("Activity — one honest empty state over a range with no calls", () => {
  it("replaces the four metric tiles with a single empty state", async () => {
    mockEmptySummary();
    renderActivity();

    expect(await screen.findByText("No calls in this range")).toBeInTheDocument();
    for (const id of ["tile-calls", "tile-error-rate", "tile-latency", "tile-cache-hit"]) {
      expect(screen.queryByTestId(id)).not.toBeInTheDocument();
    }
  });

  it("renders no per-endpoint table when there is no breakdown to put in one", async () => {
    // `hasCalls` is true on the grand total alone, so the table can be reached
    // with an empty `endpoints`. A header row over nothing is not a better
    // answer than no table, and it is how the second empty sentence crept back.
    mockSummary({
      data: {
        status: 200,
        data: { ...EMPTY_SUMMARY, total: { ...EMPTY_SUMMARY.total, calls: 5 } },
      },
    });
    renderActivity();

    expect(await screen.findByTestId("tile-calls")).toHaveTextContent("5");
    expect(screen.queryByTestId("activity-per-endpoint")).not.toBeInTheDocument();
    // The call list settles a tick after the summary, so wait for its answer
    // before counting — a synchronous count here would pass on an absence that
    // has not happened yet.
    await screen.findByText("No calls in this range");
    expect(screen.getAllByText(/no calls in this range/i)).toHaveLength(1);
  });

  it("says it once, not once per table", async () => {
    mockEmptySummary();
    mockCalls([]);
    renderActivity();
    await screen.findByText("No calls in this range");

    expect(screen.queryByTestId("activity-per-endpoint")).not.toBeInTheDocument();
    expect(screen.queryAllByRole("table")).toHaveLength(0);
    expect(screen.getAllByText(/no calls in this range/i)).toHaveLength(1);
  });

  it("gates the filter row on the empty range — but never the range strip, a view switch", async () => {
    // The call list is mounted here (it is mounted in every state), so this
    // sees the gate itself and not an unmounted subtree.
    mockEmptySummary();
    renderActivity();
    await screen.findByText("No calls in this range");

    expect(screen.queryByRole("combobox", { name: /endpoint/i })).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/status code/i)).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/error code/i)).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/request id/i)).not.toBeInTheDocument();

    // §1.2: a view SWITCH is not gated. All four presets are still there, and
    // the one that is showing is still pressable.
    for (const r of ["1h", "24h", "7d", "30d"]) {
      expect(screen.getByRole("button", { name: r })).toBeInTheDocument();
    }
    expect(screen.getByRole("button", { name: "24h" })).not.toBeDisabled();
  });

  it("keeps the one way to reach trial runs, which the summary never counts", async () => {
    // `activity.rs`: "Trials (`actor_user_id IS NOT NULL`) are excluded here
    // and shown only in the call list." So a workspace whose only traffic was
    // Try-it runs reads as an empty range, and gating the trial toggle away
    // would remove the control exactly when it is the only one that can act.
    mockEmptySummary();
    mockTrialOnlyCalls([
      makeCall({ id: "trial-1", request_id: "req-trial-1", actor_user_id: "user-77", client_id: "trial:sa-9" }),
    ]);
    const user = userEvent.setup();
    renderActivity();
    await screen.findByText("No calls in this range");
    // The default view asked for consumer traffic and got none — the trial row
    // exists and is deliberately not in it.
    await waitFor(() =>
      expect(api.dataApiActivityCalls).toHaveBeenCalledWith(
        "eng",
        expect.objectContaining({ include_trials: false }),
      ),
    );
    expect(screen.queryByText("req-trial-1")).not.toBeInTheDocument();

    await user.click(screen.getByRole("checkbox", { name: /include trial runs/i }));

    expect(await screen.findByText("req-trial-1")).toBeInTheDocument();
    await waitFor(() =>
      expect(api.dataApiActivityCalls).toHaveBeenCalledWith(
        "eng",
        expect.objectContaining({ include_trials: true }),
      ),
    );
    // …and the way back out is on screen, checked.
    expect(screen.getByRole("checkbox", { name: /include trial runs/i })).toBeChecked();
    // There are rows now, so the filters can act on them and are back — the
    // gate is on what is on screen, not on the summary alone.
    expect(screen.getByLabelText(/status code/i)).toBeInTheDocument();
  });

  it("can be turned back off from a trial view that found nothing", async () => {
    // The round trip is the point. The toggle sets a mode; if it lived in the
    // gated filter row, a trial view with no trials in it would hide the only
    // way back, and the reader would be left holding a view they cannot leave.
    mockEmptySummary();
    mockCalls([]);
    const user = userEvent.setup();
    renderActivity();
    await screen.findByText("No calls in this range");

    const toggle = screen.getByRole("checkbox", { name: /include trial runs/i });
    await user.click(toggle);
    expect(toggle).toBeChecked();
    // Still the honest sentence: a view switch with nothing behind it is an
    // empty RANGE, not a filter mismatch — nothing is engaged to mismatch.
    expect(await screen.findByText("No calls in this range")).toBeInTheDocument();
    expect(screen.queryByText(/no calls match/i)).not.toBeInTheDocument();
    // The call list IS mounted here, and this is the only state in which its
    // own gate is observable: no consumer traffic, no trial rows, no filter
    // engaged. Every other empty case hides the filters by unmounting the
    // whole list, which would pass with the gate deleted.
    expect(screen.queryByLabelText(/status code/i)).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/error code/i)).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/request id/i)).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: /endpoint/i })).not.toBeInTheDocument();

    await user.click(toggle);

    expect(await screen.findByText("No calls in this range")).toBeInTheDocument();
  });

  it("keeps a filter that misses over trial rows, where the range itself is empty", async () => {
    // The gate has three disjuncts and the obvious fixture only exercises one:
    // with `hasCalls` true the filter row is on screen whatever else is true,
    // so a test written over a range that HAS calls cannot tell the way-out
    // clause from a constant. This is the state where it is the only thing
    // holding the input up: no consumer traffic, one trial row, then a status
    // code that matches nothing.
    mockEmptySummary();
    mockTrialOnlyCalls([makeCall({ id: "trial-1", request_id: "req-trial-1", actor_user_id: "user-77" })]);
    const user = userEvent.setup();
    renderActivity();
    await screen.findByText("No calls in this range");

    await user.click(screen.getByRole("checkbox", { name: /include trial runs/i }));
    await screen.findByText("req-trial-1");

    mockCalls([]);
    await user.type(screen.getByLabelText(/status code/i), "404");
    await waitFor(() => expect(screen.queryByText("req-trial-1")).not.toBeInTheDocument());

    // Nothing on screen, `hasCalls` false — and the input still there to undo.
    expect(screen.getByLabelText(/status code/i)).toHaveValue("404");
    expect(screen.getByRole("combobox", { name: /endpoint/i })).toBeInTheDocument();
  });

  it("offers the next action rather than a dead end", async () => {
    mockEmptySummary();
    renderActivity();
    await screen.findByText("No calls in this range");

    expect(screen.getByRole("link", { name: /view endpoints/i })).toHaveAttribute(
      "href",
      "/explore/eng/data-api/endpoints",
    );
  });

  it("does not fold a failed fetch into the empty state, on either query", async () => {
    // Both fail, which is what a backend that is down looks like. Neither
    // region may answer with "no calls" — that is a claim about the world,
    // made from two requests that returned nothing about it.
    mockSummary({ isError: true, error: { message: "summary boom" }, data: undefined });
    mockCallsError("calls boom");
    renderActivity();

    expect(await screen.findByText(/summary boom/i)).toBeInTheDocument();
    expect(
      await within(screen.getByTestId("activity-call-list")).findByText(/calls boom/i),
    ).toBeInTheDocument();
    expect(screen.queryByText(/no calls in this range/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/no calls match/i)).not.toBeInTheDocument();
    expect(screen.queryByTestId("tile-calls")).not.toBeInTheDocument();
  });

  it("does not fold a settled-but-empty-handed query into the empty state either", async () => {
    // Not loading, not an error, and no payload: react-query clears `isLoading`
    // on ANY settle. Claiming "no calls in this range" here would be a sentence
    // about the world, asserted from a result nobody has.
    mockSummary({ data: undefined, isLoading: false, isError: false });
    renderActivity();

    expect(screen.getAllByTestId("list-skeleton").length).toBeGreaterThan(0);
    expect(screen.queryByText(/no calls in this range/i)).not.toBeInTheDocument();
  });
});

/**
 * The call list is mounted in every state of the summary query, and these are
 * the two things that buys. Both were lost when it lived inside the summary's
 * `ListStates` children, and neither could be seen by a test whose mocked hook
 * never flips `isLoading`.
 */
describe("Activity — the call list does not depend on the summary", () => {
  it("keeps the filters the user typed across a range switch that reloads the summary", async () => {
    // `range` is in the summary's query key, so every switch puts that query
    // back into `isLoading`. Rendered inside the branch, the whole list
    // unmounts and its four `useState` filters reset — paste a request id,
    // find the row in 24h, widen to 7d, and the field is blank.
    vi.mocked(api.useDataApiActivitySummary).mockImplementation(
      ((_ws: string, params: { range?: string }) => {
        const inFlight = params.range === "7d";
        return {
          data: inFlight ? undefined : { status: 200, data: DEFAULT_SUMMARY },
          isLoading: inFlight,
          isError: false,
          error: null,
        };
      }) as unknown as typeof api.useDataApiActivitySummary,
    );
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

    await user.click(screen.getByRole("button", { name: "7d" }));

    // The summary is a skeleton now…
    expect(screen.getAllByTestId("list-skeleton").length).toBeGreaterThan(0);
    // …and the filter the user typed is still on screen, still applied.
    expect(screen.getByLabelText(/request id/i)).toHaveValue("req-abc-999");
    await waitFor(() =>
      expect(api.dataApiActivityCalls).toHaveBeenCalledWith(
        "eng",
        expect.objectContaining({ range: "7d", request_id: "req-abc-999" }),
      ),
    );
  });

  it("does not blink the filter row out while a range switch is in flight", async () => {
    // "Not measured yet" is not "measured and empty". Gated on `!hasCalls` —
    // which is false for the whole of every reload — the filter row would
    // vanish and come back on each range switch, with nothing typed to keep
    // it. Nothing is typed here on purpose: an engaged filter holds the row up
    // by itself and would hide the bug.
    vi.mocked(api.useDataApiActivitySummary).mockImplementation(
      ((_ws: string, params: { range?: string }) => {
        const inFlight = params.range === "7d";
        return {
          data: inFlight ? undefined : { status: 200, data: DEFAULT_SUMMARY },
          isLoading: inFlight,
          isError: false,
          error: null,
        };
      }) as unknown as typeof api.useDataApiActivitySummary,
    );
    const user = userEvent.setup();
    renderActivity();
    await screen.findByTestId("tile-calls");
    expect(screen.getByLabelText(/status code/i)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "7d" }));

    expect(screen.getAllByTestId("list-skeleton").length).toBeGreaterThan(0);
    expect(screen.getByLabelText(/status code/i)).toBeInTheDocument();
  });

  it("still shows the calls when the summary is the query that failed", async () => {
    mockSummary({ isError: true, error: { message: "summary boom" }, data: undefined });
    mockCalls([makeCall()]);
    renderActivity();

    expect(await screen.findByText(/summary boom/i)).toBeInTheDocument();
    // Two queries, two answers. Hiding a list that loaded because a different
    // request failed tells the operator less than the screen knows.
    expect(await screen.findByText("req-fixture-1")).toBeInTheDocument();
  });
});

describe("Activity — the filter row when there IS something to filter", () => {
  it("renders all four filters over a range that produced calls", async () => {
    renderActivity();
    await screen.findByTestId("tile-calls");

    expect(screen.getByRole("combobox", { name: /endpoint/i })).toBeInTheDocument();
    expect(screen.getByLabelText(/status code/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/error code/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/request id/i)).toBeInTheDocument();
  });

  it("keeps an ENGAGED filter on screen when it matches nothing — the way out", async () => {
    // Gated on the rows actually returned, a filter miss would remove the input
    // holding the filter, leaving nothing to un-set (§2.3).
    mockCalls([makeCall()]);
    const user = userEvent.setup();
    renderActivity();
    await screen.findByText("req-fixture-1");

    mockCalls([]);
    await user.type(screen.getByLabelText(/status code/i), "404");

    await waitFor(() => expect(screen.queryByText("req-fixture-1")).not.toBeInTheDocument());
    expect(screen.getByLabelText(/status code/i)).toHaveValue("404");
    // A miss is a different situation from an empty range, and says so.
    const list = screen.getByTestId("activity-call-list");
    expect(within(list).getByText(/no calls match the current filters/i)).toBeInTheDocument();
    expect(within(list).queryByText(/no calls in this range/i)).not.toBeInTheDocument();
  });
});

// ---------------------------------------------------------------------------
// The monitoring loop: read a failing endpoint's name, then see its calls.
// ---------------------------------------------------------------------------

/** A summary whose breakdown is exactly `rows`, with a grand total summed from
 * them. Not `DEFAULT_SUMMARY` with `endpoints` swapped: that keeps
 * `total.calls: 120` beside a breakdown that no longer adds up to it, a state
 * one `GROUPING SETS` query cannot produce — and a degenerate fixture cannot
 * tell a page that reads the rows from one that reads the total. */
function mockSummaryRows(rows: ActivityRow[]) {
  const sum = (pick: (r: ActivityRow) => number) => rows.reduce((n, r) => n + pick(r), 0);
  mockSummary({
    data: {
      status: 200,
      data: {
        total: {
          endpoint_id: null,
          endpoint_name: null,
          calls: sum((r) => r.calls),
          errors: sum((r) => r.errors),
          p50_ms: 40,
          p95_ms: 180,
          cache_hits: sum((r) => r.cache_hits),
          cacheable_calls: sum((r) => r.cacheable_calls),
        },
        endpoints: rows,
        retention_days: null,
      } satisfies ActivitySummaryDto,
    },
  });
}

function makeRow(over: Partial<ActivityRow> = {}): ActivityRow {
  return {
    endpoint_id: "ep-1",
    endpoint_name: "Daily report",
    calls: 100,
    errors: 4,
    p50_ms: 40,
    p95_ms: 180,
    cache_hits: 28,
    cacheable_calls: 90,
    ...over,
  };
}

/** The two-endpoint world the loop tests navigate: the call fetcher HONOURS
 * `endpoint_id`, so "the filter reached the server" is observable as a row that
 * is no longer there — by identity, not by a count over a population that never
 * empties. */
function mockCallsPerEndpoint() {
  const all = [
    makeCall({ id: "c-daily", endpoint_id: "ep-1", endpoint_name: "Daily report", request_id: "req-daily" }),
    makeCall({ id: "c-weekly", endpoint_id: "ep-9", endpoint_name: "Weekly export", request_id: "req-weekly" }),
  ];
  vi.mocked(api.dataApiActivityCalls).mockImplementation(async (_ws, params) => {
    const id = (params as { endpoint_id?: string }).endpoint_id;
    return {
      status: 200,
      data: id ? all.filter((c) => c.endpoint_id === id) : all,
      headers: new Headers(),
    } as unknown as Awaited<ReturnType<typeof api.dataApiActivityCalls>>;
  });
}

describe("Activity — a per-endpoint row reaches its own calls", () => {
  beforeEach(() => {
    mockSummaryRows([
      makeRow({ endpoint_id: "ep-1", endpoint_name: "Daily report", errors: 20 }),
      makeRow({ endpoint_id: "ep-9", endpoint_name: "Weekly export", errors: 0 }),
    ]);
    mockEndpoints([makeEndpoint(), makeEndpoint({ id: "ep-9", name: "Weekly export" })]);
    mockCallsPerEndpoint();
  });

  it("filtering from the table leaves only that endpoint's calls on screen", async () => {
    const user = userEvent.setup();
    renderActivity();
    // Both endpoints' calls are listed before anything is clicked — otherwise
    // the assertion below would pass over a list that was never going to show
    // the other row.
    await screen.findByText("req-weekly");
    expect(screen.getByText("req-daily")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Show calls for Daily report" }));

    await waitFor(() => expect(screen.queryByText("req-weekly")).not.toBeInTheDocument());
    expect(screen.getByText("req-daily")).toBeInTheDocument();
  });

  it("reaches those calls from the keyboard as well as the mouse", async () => {
    // The row is a real `<button>` (EngineMonitor's idiom for the same job), so
    // Enter and Space are the browser's, not a hand-rolled `onKeyDown` that can
    // handle one and forget the other. What this pins is the choice made HERE:
    // the endpoint name is a control, not the inert `<td>` text it used to be —
    // measured as `cursor: auto`, no role, nothing focusable in the row.
    const user = userEvent.setup();
    renderActivity();
    await screen.findByText("req-weekly");

    const control = screen.getByRole("button", { name: "Show calls for Daily report" });
    control.focus();
    expect(control).toHaveFocus();
    await user.keyboard("{Enter}");

    await waitFor(() => expect(screen.queryByText("req-weekly")).not.toBeInTheDocument());
  });

  it("Space works too, not only Enter", async () => {
    const user = userEvent.setup();
    renderActivity();
    await screen.findByText("req-weekly");

    screen.getByRole("button", { name: "Show calls for Weekly export" }).focus();
    await user.keyboard("[Space]");

    await waitFor(() => expect(screen.queryByText("req-daily")).not.toBeInTheDocument());
    expect(screen.getByText("req-weekly")).toBeInTheDocument();
  });

  it("moves focus to the Endpoint filter, which is where the change happened", async () => {
    // The table is above the fold and the call list is below it. Without this,
    // a keyboard user presses Enter and nothing near them changes — and a
    // pointer user gets no scroll either, since focus is what browsers scroll
    // to. The filter is also the control that UNDOES the choice.
    const user = userEvent.setup();
    renderActivity();
    await screen.findByText("req-weekly");

    await user.click(screen.getByRole("button", { name: "Show calls for Daily report" }));

    const filter = screen.getByRole("combobox", { name: /endpoint/i });
    expect(filter).toHaveFocus();
    // And it SAYS which endpoint, rather than silently narrowing the list.
    expect(filter).toHaveTextContent("Daily report");
  });

  it("clears a typed request id, which cannot coexist with browsing an endpoint", async () => {
    // A request id names exactly one call, so intersecting it with "show me
    // this endpoint's calls" yields that one call or nothing — a control
    // captioned `Show calls for X` landing on an empty list. Cross-cutting
    // filters are a different case and survive: see the next test.
    const user = userEvent.setup();
    renderActivity();
    await screen.findByText("req-weekly");

    // Exact label, not `/request id/i`: every call row now carries a
    // "Copy request id …" button, so the loose matcher this file uses
    // elsewhere finds the input AND every button beside one.
    await user.type(screen.getByLabelText("Request ID"), "req-from-an-error-body");
    await waitFor(() =>
      expect(api.dataApiActivityCalls).toHaveBeenLastCalledWith(
        "eng",
        expect.objectContaining({ request_id: "req-from-an-error-body" }),
      ),
    );

    await user.click(screen.getByRole("button", { name: "Show calls for Daily report" }));

    expect(screen.getByLabelText("Request ID")).toHaveValue("");
    await waitFor(() =>
      expect(api.dataApiActivityCalls).toHaveBeenLastCalledWith(
        "eng",
        expect.objectContaining({ endpoint_id: "ep-1", request_id: undefined }),
      ),
    );
    // And the endpoint's calls are actually on screen, which is what the
    // button promised — asserted by identity, not by the request that was sent.
    expect(await screen.findByText("req-daily")).toBeInTheDocument();
  });

  it("keeps a status-code filter, which an operator may be carrying INTO the endpoint", async () => {
    // "Show me the 500s → now just this endpoint" is a real path, and clearing
    // everything would break it. The rule is not "a jump resets the form"; it
    // is that a point lookup and a browse are different modes.
    const user = userEvent.setup();
    renderActivity();
    await screen.findByText("req-weekly");

    await user.type(screen.getByLabelText(/status code/i), "500");
    await user.click(screen.getByRole("button", { name: "Show calls for Daily report" }));

    expect(screen.getByLabelText(/status code/i)).toHaveValue("500");
    await waitFor(() =>
      expect(api.dataApiActivityCalls).toHaveBeenLastCalledWith(
        "eng",
        expect.objectContaining({ endpoint_id: "ep-1", status_code: 500 }),
      ),
    );
  });

  it("does not offer the unmatched bucket as a filter — the dropdown cannot express it", async () => {
    // `endpoint_id: null` is every request that resolved no endpoint. There is
    // no id to filter by, so the row stays inert rather than becoming a control
    // that cannot do what it looks like it does.
    mockSummaryRows([
      makeRow(),
      makeRow({ endpoint_id: null, endpoint_name: null, errors: 2 }),
    ]);
    renderActivity();

    const row = (await screen.findByText("No matching endpoint")).closest("tr")!;
    expect(within(row).queryByRole("button")).not.toBeInTheDocument();
    // The matched row above it IS a control, so this is a difference between
    // rows and not a page with no controls at all.
    expect(screen.getByRole("button", { name: "Show calls for Daily report" })).toBeInTheDocument();
  });

  it("names the endpoint it will filter to, so eight of them are eight controls", async () => {
    renderActivity();
    await screen.findByText("req-weekly");
    // Real accessible-name computation, not a `title`/text sniff.
    expect(screen.getByRole("button", { name: "Show calls for Weekly export" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show calls for Daily report" })).toBeInTheDocument();
  });

  it("still shows the chosen endpoint when it is no longer in the endpoints list", async () => {
    // A deleted endpoint keeps its `access_log` rows, and the summary's LEFT
    // JOIN then yields `endpoint_name: null` — an id the live endpoints list
    // does not contain. Without a fallback option the filter would read blank
    // while the list below it was narrowed: a filter in effect that the screen
    // does not admit to.
    const user = userEvent.setup();
    mockSummaryRows([makeRow({ endpoint_id: "ep-gone", endpoint_name: null })]);
    mockEndpoints([]);
    renderActivity();
    await screen.findByTestId("tile-calls");

    await user.click(screen.getByRole("button", { name: "Show calls for ep-gone" }));

    expect(screen.getByRole("combobox", { name: /endpoint/i })).toHaveTextContent("ep-gone");
    await waitFor(() =>
      expect(api.dataApiActivityCalls).toHaveBeenLastCalledWith(
        "eng",
        expect.objectContaining({ endpoint_id: "ep-gone" }),
      ),
    );
  });
});

// ---------------------------------------------------------------------------
// Severity: an error rate that says how bad it is.
// ---------------------------------------------------------------------------

/** The cell under a named COLUMN, in a named row.
 *
 * Both halves are looked up rather than counted. `getByText("20.0%")` alone
 * would match the same figure in another column of another row, and the
 * `[2]` this used to be was a counted hop: insert a column and every severity
 * assertion silently starts measuring p50/p95 while staying green. The index
 * now comes from the header the product renders, so it moves when the table
 * does. */
function cellUnder(columnName: string, endpointName: string): HTMLElement {
  const table = screen.getByTestId("activity-per-endpoint");
  const headers = within(table).getAllByRole("columnheader");
  const column = headers.findIndex((h) => h.textContent?.trim() === columnName);
  expect(column, `no column headed "${columnName}"`).toBeGreaterThanOrEqual(0);
  const row = within(table).getByText(endpointName).closest("tr")!;
  return within(row).getAllByRole("cell")[column];
}

function errorRateOf(endpointName: string): HTMLElement {
  return cellUnder("Error rate", endpointName);
}

describe("Activity — an error rate carries its severity", () => {
  it("marks a failing endpoint in form as well as colour, and a healthy one not at all", async () => {
    mockSummaryRows([
      makeRow({ endpoint_id: "ep-bad", endpoint_name: "Spend pacing", calls: 100, errors: 20 }),
      makeRow({ endpoint_id: "ep-ok", endpoint_name: "Quiet export", calls: 100, errors: 0 }),
    ]);
    renderActivity();
    await screen.findByTestId("activity-per-endpoint");

    const bad = errorRateOf("Spend pacing");
    const ok = errorRateOf("Quiet export");

    // FORM: an icon the failing reading has and the healthy one does not. This
    // is the half that survives a monochrome display, and it is structural —
    // an inert class could not produce it.
    expect(bad.querySelector("svg")).not.toBeNull();
    expect(ok.querySelector("svg")).toBeNull();
    // WORD: what a screen reader gets, where colour reaches nobody.
    expect(within(bad).getByText(/failing/i)).toBeInTheDocument();
    expect(within(ok).getByText(/no errors/i)).toBeInTheDocument();
    // COLOUR: the class only. What it PAINTS is asserted in
    // tests/e2e/activity-error-rate-severity.spec.ts — jsdom resolves no
    // `oklch()` and would give the same answer with the tokens swapped (§10).
    expect(bad.querySelector(".text-destructive")).not.toBeNull();
    expect(ok.querySelector(".text-success")).not.toBeNull();
  });

  it("renders 0.0% as good, not as the dash that means unmeasured", async () => {
    // The Cache-hit column on this very table already uses `—` for "not
    // configured", so an empty-looking cell means something else here. A
    // zero error rate is a MEASUREMENT, and the best one.
    mockSummaryRows([makeRow({ endpoint_name: "Quiet export", calls: 100, errors: 0, cacheable_calls: 0, cache_hits: 0 })]);
    renderActivity();
    await screen.findByTestId("activity-per-endpoint");

    const cell = errorRateOf("Quiet export");
    expect(within(cell).getByText("0.0%")).toBeInTheDocument();
    expect(cell).not.toHaveTextContent("—");
    // And the cell that IS unmeasured, on the same row, still says so.
    expect(cellUnder("Cache-hit rate", "Quiet export")).toHaveTextContent("—");
  });

  it("draws the alert line at 5% of calls: 5.0% is failing, 4.9% is not", async () => {
    // 5% is the error-budget convention (a 99% target spends a month's budget
    // in about seven hours at this rate), and it sits far above the routine
    // 4xx traffic that `errors` also counts. The seeded distribution — 0.0%
    // against 18.8% and 20.0% — fixes only that the line lies between them.
    mockSummaryRows([
      makeRow({ endpoint_id: "ep-at", endpoint_name: "At the line", calls: 20, errors: 1 }),
      makeRow({ endpoint_id: "ep-under", endpoint_name: "Under the line", calls: 1000, errors: 49 }),
    ]);
    renderActivity();
    await screen.findByTestId("activity-per-endpoint");

    const at = errorRateOf("At the line");
    const under = errorRateOf("Under the line");
    expect(within(at).getByText("5.0%")).toBeInTheDocument();
    expect(within(under).getByText("4.9%")).toBeInTheDocument();
    expect(within(at).getByText(/failing/i)).toBeInTheDocument();
    expect(within(under).queryByText(/failing/i)).not.toBeInTheDocument();
  });

  it("marks a trickle differently from a flood, rather than collapsing to two states", async () => {
    // The middle tier separates FEW from MANY, and nothing else. It is not
    // "4xx versus 5xx" — a threshold on a rate cannot see the difference (see
    // `errorRateTier`). What it buys is that a trickle is neither invisible nor
    // alarming, which is the operator's third distinct next action.
    mockSummaryRows([
      makeRow({ endpoint_id: "ep-mid", endpoint_name: "Some 4xx", calls: 1000, errors: 20 }),
      makeRow({ endpoint_id: "ep-ok", endpoint_name: "Quiet export", calls: 100, errors: 0 }),
      makeRow({ endpoint_id: "ep-bad", endpoint_name: "Spend pacing", calls: 100, errors: 20 }),
    ]);
    renderActivity();
    await screen.findByTestId("activity-per-endpoint");

    const mid = errorRateOf("Some 4xx");
    expect(within(mid).getByText(/elevated/i)).toBeInTheDocument();
    expect(mid.querySelector("svg")).not.toBeNull();
    // Three readings, three different treatments — not one marked state and two
    // spellings of the same one. The middle tier wears the amber on its BORDER
    // rather than as ink: `--highlight` measures 2.56:1 as text on this
    // surface, which the browser spec caught and no class assertion could (see
    // `ERROR_RATE_INK`).
    expect(mid.querySelector('[class*="highlight"]')).not.toBeNull();
    expect(mid.querySelector(".text-destructive")).toBeNull();
    expect(errorRateOf("Spend pacing").querySelector(".text-destructive")).not.toBeNull();
    expect(errorRateOf("Spend pacing").querySelector('[class*="highlight"]')).toBeNull();
    expect(errorRateOf("Quiet export").querySelector('[class*="highlight"]')).toBeNull();
  });

  it("separates elevated from failing by the ICON, not by the colour of one icon", async () => {
    // The only channel left for a reader who has neither colour nor a screen
    // reader: a greyscale print, a monochrome display, a glance down a column.
    // One glyph in two hues would collapse to one glyph.
    mockSummaryRows([
      makeRow({ endpoint_id: "ep-mid", endpoint_name: "Some 4xx", calls: 1000, errors: 20 }),
      makeRow({ endpoint_id: "ep-bad", endpoint_name: "Spend pacing", calls: 100, errors: 20 }),
    ]);
    renderActivity();
    await screen.findByTestId("activity-per-endpoint");

    const midIcon = errorRateOf("Some 4xx").querySelector("svg")!;
    const badIcon = errorRateOf("Spend pacing").querySelector("svg")!;
    expect(midIcon).not.toBeNull();
    expect(badIcon).not.toBeNull();
    // lucide stamps the glyph name onto the class list, so this compares the
    // SHAPES rather than two references to the same element.
    expect(badIcon.getAttribute("class")).not.toBe(midIcon.getAttribute("class"));
    expect(midIcon.getAttribute("class")).toMatch(/triangle/i);
    expect(badIcon.getAttribute("class")).toMatch(/octagon/i);
  });

  it("gives the Error rate tile the same severity as the table", async () => {
    mockSummaryRows([makeRow({ endpoint_name: "Spend pacing", calls: 100, errors: 20 })]);
    renderActivity();

    const tile = await screen.findByTestId("tile-error-rate");
    expect(tile).toHaveTextContent("20.0%");
    expect(within(tile).getByText(/failing/i)).toBeInTheDocument();
    expect(tile.querySelector("svg")).not.toBeNull();
  });

  it("gives the elevated TILE a second channel, not one lonely triangle", async () => {
    // The tile has no pill, and `ERROR_RATE_INK.elevated` is the tile's own
    // ink — so without a visible word this reading would be an ordinary number
    // plus a small triangle, one channel where the other two tiers have two.
    // Review found that; the tile tests covered only `failing` and `none`,
    // where the ink differs, so nothing here could have.
    mockSummaryRows([makeRow({ calls: 1000, errors: 20 })]);
    renderActivity();

    const tile = await screen.findByTestId("tile-error-rate");
    expect(tile).toHaveTextContent("2.0%");
    expect(tile.querySelector("svg"), "the icon channel").not.toBeNull();
    const word = within(tile).getByText("Elevated");
    // VISIBLE, not `sr-only` — an sr-only word is not a second channel for a
    // reader who has the screen and not a screen reader, which is the case
    // this assertion exists for. jsdom applies no stylesheet, so the class
    // that does the hiding is the only thing that can be asked.
    expect(word.className).not.toMatch(/\bsr-only\b/);
    // And it is not doubled: a visible word plus the sr-only one would have a
    // screen reader say "Elevated" twice.
    expect(within(tile).getAllByText(/elevated/i)).toHaveLength(1);
  });

  it("renders a dash for an endpoint with no calls to divide by", async () => {
    // `unmeasured` — the one tier the backend cannot currently produce for a
    // row (a row exists because calls exist), so it is only reachable through
    // this fixture. It is still the branch that decides `—` versus `0.0%`, and
    // the whole point of the treatment is that those two mean different things.
    mockSummaryRows([
      makeRow({ endpoint_id: "ep-quiet", endpoint_name: "Never called", calls: 0, errors: 0 }),
    ]);
    renderActivity();
    await screen.findByTestId("activity-per-endpoint");

    const cell = errorRateOf("Never called");
    expect(cell).toHaveTextContent("—");
    expect(within(cell).queryByText("0.0%")).not.toBeInTheDocument();
    // No severity marking on an absence: nothing was measured, so nothing is
    // good and nothing is bad.
    expect(cell.querySelector("svg")).toBeNull();
    expect(cell.querySelector(".text-success")).toBeNull();
  });

  it("leaves the tile unmarked when the range produced no errors at all", async () => {
    mockSummaryRows([makeRow({ calls: 100, errors: 0 })]);
    renderActivity();

    const tile = await screen.findByTestId("tile-error-rate");
    expect(tile).toHaveTextContent("0.0%");
    expect(within(tile).getByText(/no errors/i)).toBeInTheDocument();
    expect(tile.querySelector("svg")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// The ids a call row shows are the ids the workflow asks you to paste back.
// ---------------------------------------------------------------------------

describe("Activity — a call row's ids can be copied", () => {
  it("copies the request id — the value the Request ID filter asks for", async () => {
    // `userEvent.setup()` REPLACES `navigator.clipboard`, so a spy installed
    // beforehand is an object this component never touches. Read back through
    // userEvent's own stub instead: assert what LANDED, not that a call was made.
    const user = userEvent.setup();
    mockCalls([makeCall({ request_id: "req-fixture-1" })]);
    renderActivity();
    await screen.findByText("req-fixture-1");

    await user.click(screen.getByRole("button", { name: "Copy request id req-fixture-1" }));

    expect(await navigator.clipboard.readText()).toBe("req-fixture-1");
  });

  it("copies the client id too — the same identifier Clients already offers", async () => {
    const user = userEvent.setup();
    mockCalls([makeCall({ client_id: "client_abc123" })]);
    renderActivity();
    await screen.findByText("client_abc123");

    await user.click(screen.getByRole("button", { name: "Copy client id client_abc123" }));

    expect(await navigator.clipboard.readText()).toBe("client_abc123");
  });

  it("copies THAT row's id, not the first row's", async () => {
    // Fifty rows of "Copy" is fifty controls with one name. Naming each button
    // after the value it carries is what makes them distinguishable to a screen
    // reader — and it is what this asserts, by copying from the second row.
    const user = userEvent.setup();
    mockCalls([
      makeCall({ id: "c1", request_id: "req-first" }),
      makeCall({ id: "c2", request_id: "req-second" }),
    ]);
    renderActivity();
    await screen.findByText("req-second");

    await user.click(screen.getByRole("button", { name: "Copy request id req-second" }));

    expect(await navigator.clipboard.readText()).toBe("req-second");
  });

  it("does not claim 'Copied' when the clipboard write fails", async () => {
    // `writeText` rejects on a denied permission, an unfocused document, or
    // plain http with no clipboard API. The four other copies of this idiom in
    // the repo `void` the promise and tick regardless, telling the user an id
    // is on their clipboard when it is not — and the id is the entire point of
    // the button.
    const user = userEvent.setup();
    // AFTER `setup()`, which installs its own stub: patching before it would
    // be overwritten, and the test would pass over a component that ticks.
    const writeText = vi
      .spyOn(navigator.clipboard, "writeText")
      .mockRejectedValue(new Error("denied"));
    mockCalls([makeCall({ request_id: "req-denied" })]);
    renderActivity();
    await screen.findByText("req-denied");

    const button = screen.getByRole("button", { name: "Copy request id req-denied" });
    await user.click(button);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("req-denied"));

    // The tick is the claim, and lucide names its glyphs in the class list —
    // so this asks which SHAPE is on screen rather than trusting a state flag.
    await waitFor(() => expect(button.querySelector("svg")?.getAttribute("class")).toMatch(/copy/i));
    expect(button.querySelector("svg")?.getAttribute("class")).not.toMatch(/check/i);
    writeText.mockRestore();
  });

  it("ticks only after the write has actually landed", async () => {
    // The other half: with the tick gated on a promise, "it never ticks at all"
    // would satisfy the test above. This is the same assertion in the opposite
    // direction, so neither can pass alone on a broken button.
    const user = userEvent.setup();
    mockCalls([makeCall({ request_id: "req-ok" })]);
    renderActivity();
    await screen.findByText("req-ok");

    const button = screen.getByRole("button", { name: "Copy request id req-ok" });
    await user.click(button);

    await waitFor(() =>
      expect(button.querySelector("svg")?.getAttribute("class")).toMatch(/check/i),
    );
  });

  it("offers no client-id copy on a trial row, which carries no client identity", async () => {
    // A trial's `client_id` is the reserved marker `trial:<service_account_id>`
    // and the row shows the actor instead. A copy button there would hand over
    // a string that is not a client id.
    const user = userEvent.setup();
    mockCalls([makeCall({ id: "c-trial", actor_user_id: "user-77", actor_name: "alice@egret.local", client_id: "trial:sa-99", request_id: "req-trial" })]);
    renderActivity();
    await screen.findByText("alice@egret.local");

    expect(screen.queryByRole("button", { name: /copy client id/i })).not.toBeInTheDocument();
    // The request id is still copyable — that is the one support quotes.
    await user.click(screen.getByRole("button", { name: "Copy request id req-trial" }));
    expect(await navigator.clipboard.readText()).toBe("req-trial");
  });
});

/**
 * THE STATUS-CODE CHIP IS BOUND TO `CHIP_TONE`, PER BRANCH.
 *
 * `ActivityCallList` painted `text-success` on `bg-success/15` for 2xx and
 * `text-destructive` on `bg-destructive/15` for 4xx/5xx — 3.72:1 and 3.68:1 in
 * light mode, under WCAG 2.2 1.4.3's 4.5:1 for this 12px text — and moved onto
 * the shared map in `chipTone.ts`. `statusTone` is a three-way conditional and
 * the assertions above only ever look at the NUMBER, so until now no test
 * distinguished the branches from each other or from no tone at all.
 *
 * The `neutral` branch (1xx/3xx) is deliberately the ABSENCE of a state
 * colour, and it needs the same guard as the other two: it is the branch a
 * careless edit collapses into `destructive`, which would call a `304` an
 * error.
 *
 * Expected classes are read out of `CHIP_TONE`, never typed, so a contrast
 * change moves the map and these guards together. The ratios are measured in
 * `tests/e2e/dataapi-badge-ink.spec.ts`; jsdom composites nothing.
 */
describe("Activity — the call list's status chip is wired to the shared tone", () => {
  const ARMS = [
    [200, "success"],
    [500, "destructive"],
    [304, "neutral"],
  ] as const;

  it.each(ARMS)("paints a %i with CHIP_TONE.%s", async (status, tone) => {
    mockEmptySummary();
    mockCalls([makeCall({ id: `call-${status}`, status_code: status, request_id: `req-${status}` })]);
    renderActivity();
    // The request id, not the status code: waiting on the code would make the
    // query below find whatever it waited for.
    await screen.findByText(`req-${status}`);
    const row = screen.getByText(`req-${status}`).closest("tr")!;
    expectChipTone(within(row).getByText(String(status)), tone, `ActivityCallList: ${status}`);
  });
});
