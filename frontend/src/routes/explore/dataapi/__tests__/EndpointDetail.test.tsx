import { describe, it, expect, vi, beforeEach } from "vitest";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// There is no msw in this repo — tests mock the API module directly (see
// Endpoints.test.tsx). Versions resolves a version's `created_by` to an email
// via `listUsers` (see Activity.test.tsx's equivalent mock for
// ActivityCallList's trial-actor column), so that module needs a mock.
vi.mock("@/api/generated/data-api/data-api");
vi.mock("@/api/access");

// Spy on the real hook rather than replace it with a no-op: the test needs to
// see every call's argument, in order, to tell "armed once, on a real edit"
// apart from "armed on hydration" or "never armed at all".
const warnSpy = vi.fn();
vi.mock("@/lib/useUnsavedChangesWarning", () => ({
  useUnsavedChangesWarning: (when: boolean) => warnSpy(when),
}));

import * as api from "@/api/generated/data-api/data-api";
import * as accessApi from "@/api/access";
import type {
  ColumnSpec,
  DataApiEndpointDto,
  DataApiEndpointVersionDto,
  StatementSnapshot,
} from "@/api/generated/model";
import EndpointDetail from "@/routes/explore/dataapi/EndpointDetail";
import Config from "@/routes/explore/dataapi/detail/Config";
import ResponseSection from "@/routes/explore/dataapi/detail/ResponseSection";
import VersionsSection from "@/routes/explore/dataapi/detail/Versions";
import { expectChipTone } from "./expectChipTone";
import {
  expectSectionIntroCapsItsProse,
  expectSectionLeavesWidthToThePage,
} from "./sectionWidth";

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
    live_version_id: null,
    state: "draft",
    created_by: "user-1",
    created_at: "2026-08-01T00:00:00Z",
    updated_at: "2026-08-10T00:00:00Z",
    ...overrides,
  } as unknown as DataApiEndpointDto;
}

const updateMutateAsync = vi.fn();
const validateMutateAsync = vi.fn();
const restoreMutateAsync = vi.fn();

// Mounts Config at its real route (`/config`), not at the index — Query
// (Task 3) owns the index route and its own test file mounts it there. A
// bare `/explore/:workspace/data-api/endpoints/:id` entry with no trailing
// segment would hit neither route and render nothing.
function renderConfig(endpoint: DataApiEndpointDto = makeEndpoint()) {
  vi.mocked(api.useDataApiGetEndpoint).mockReturnValue(
    { data: { status: 200, data: endpoint }, isLoading: false, isError: false, error: null, refetch: vi.fn() } as unknown as
      ReturnType<typeof api.useDataApiGetEndpoint>,
  );

  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const utils = render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/explore/eng/data-api/endpoints/${endpoint.id}/config`]}>
        <Routes>
          <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
            <Route path="config" element={<Config />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return { ...utils, qc };
}

beforeEach(() => {
  vi.clearAllMocks();
  // The shell reads the version history on EVERY tab now: the header names the
  // version a consumer is actually receiving, and says when the saved draft has
  // moved past it. Automocked, that hook hands back `undefined`, so this default
  // is what keeps the shell mountable at all here — tests that care about
  // versions override it afterwards.
  vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
    { data: { status: 200, data: [] }, isLoading: false, isError: false, error: null } as unknown as
      ReturnType<typeof api.useDataApiListEndpointVersions>,
  );

  // Default: resolves with nobody, so the Who column falls back to a
  // truncated id everywhere that doesn't set up its own fixture.
  vi.mocked(accessApi.listUsers).mockResolvedValue([]);

  vi.mocked(api.useDataApiUpdateEndpoint).mockReturnValue(
    { mutateAsync: updateMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiUpdateEndpoint>,
  );
  vi.mocked(api.useDataApiValidateEndpoint).mockReturnValue(
    { mutateAsync: validateMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiValidateEndpoint>,
  );
  // The shell constructs this on EVERY render, trashed or not (hooks cannot be
  // conditional), so without it the automock's `undefined` breaks every test in
  // this file rather than only the trash ones.
  vi.mocked(api.useDataApiRestoreEndpoint).mockReturnValue(
    { mutateAsync: restoreMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiRestoreEndpoint>,
  );

  // Automocking `vi.mock("@/api/generated/data-api/data-api")` turns these
  // plain query-key builders into `vi.fn()`s that return `undefined` — give
  // them a deterministic, argument-sensitive fake so a test can tell "the
  // right query was invalidated" from "invalidateQueries was called with
  // nothing distinguishable".
  vi.mocked(api.getDataApiGetEndpointQueryKey).mockImplementation(
    (ws, id) => ["dataApiGetEndpoint", ws, id] as unknown as ReturnType<typeof api.getDataApiGetEndpointQueryKey>,
  );
  vi.mocked(api.getDataApiListEndpointsQueryKey).mockImplementation(
    (ws) => ["dataApiListEndpoints", ws] as unknown as ReturnType<typeof api.getDataApiListEndpointsQueryKey>,
  );

  updateMutateAsync.mockImplementation(({ data }: { workspace: string; id: string; data: Partial<DataApiEndpointDto> }) =>
    Promise.resolve({ status: 200, data: { ...makeEndpoint(), ...data }, headers: new Headers() }));
  validateMutateAsync.mockResolvedValue({ status: 200, data: { statements: [] }, headers: new Headers() });
  restoreMutateAsync.mockResolvedValue({
    status: 200,
    data: makeEndpoint({ state: "disabled", deleted_at: null }),
    headers: new Headers(),
  });
});

describe("Config", () => {
  /**
   * A two-statement endpoint at the defaults is 2 x 30 = 60s, past the 50s a
   * served call is given. That is LEGAL — publish does not refuse it — so the
   * author has to be told where they set it, not by a 504 arriving in someone
   * else's logs.
   *
   * Both directions, because a warning that is always on is not a warning: the
   * default one-statement endpoint must show nothing, and it is asserted first
   * so a component that renders the sentence unconditionally fails here rather
   * than passing the interesting half.
   *
   * The 50 is duplicated in `Config.tsx` from Rust's `SERVING_REQUEST_DEADLINE`
   * and pinned by `serving::guard`'s
   * `the_config_surface_warns_against_this_modules_deadline`, which reads that
   * file. Nothing in this file could catch that drift.
   */
  it("Config warns when the endpoint's worst case outruns a served call, and stays quiet when it does not", async () => {
    const { unmount } = renderConfig(
      makeEndpoint({ statements: [{ name: "main", sql: "SELECT 1" }] } as Partial<DataApiEndpointDto>),
    );
    expect(await screen.findByLabelText("Timeout (seconds)")).toBeInTheDocument();
    expect(screen.queryByTestId("worst-case-warning")).not.toBeInTheDocument();
    unmount();

    // Two statements, no explicit timeout: the default 30 applies to EACH, so
    // the worst case is 60s. Exactly the configuration the review named.
    renderConfig(
      makeEndpoint({
        statements: [
          { name: "rows", sql: "SELECT 1" },
          { name: "total", sql: "SELECT count(*)" },
        ],
      } as Partial<DataApiEndpointDto>),
    );
    const warning = await screen.findByTestId("worst-case-warning");
    expect(warning).toHaveTextContent("60s");
    expect(warning).toHaveTextContent("50s");
    // It must not read as a refusal: publishing this is allowed.
    expect(warning.textContent ?? "").toMatch(/allowed/i);
  });

  /**
   * The value is per STATEMENT, and serving clamps it — a warning computed from
   * the literal typed number would over-report for an author who typed 10,000
   * (clamped to 300) and under-report for one who typed 0 (clamped to 1).
   */
  it("Config computes the worst case per statement, against the clamped timeout", async () => {
    renderConfig(
      makeEndpoint({
        statements: [
          { name: "a", sql: "SELECT 1" },
          { name: "b", sql: "SELECT 2" },
        ],
        limits: { timeout_seconds: 10_000 },
      } as Partial<DataApiEndpointDto>),
    );
    const warning = await screen.findByTestId("worst-case-warning");
    // 2 x clamp(10000, 1, 300) = 600, not 2 x 10000.
    expect(warning).toHaveTextContent("600s");
    expect(warning.textContent ?? "").not.toContain("20000");
  });

  it("Config holds the endpoint's identity and its four config groups", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderConfig(endpoint);
    expect(await screen.findByLabelText("Name")).toBeInTheDocument();
    expect(screen.getByLabelText("Description")).toBeInTheDocument();
    expect(screen.getByLabelText("Method")).toBeInTheDocument();
    expect(screen.getByLabelText("Path")).toBeInTheDocument();
    expect(screen.getByText("Cache")).toBeInTheDocument();
    expect(screen.getByText("Limits")).toBeInTheDocument();
    expect(screen.getByText("Rate limit")).toBeInTheDocument();
  });

  it("Config does not hold the SQL editor — that is Query's", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderConfig(endpoint);
    await screen.findByLabelText("Name");
    expect(screen.queryByLabelText("SQL")).not.toBeInTheDocument();
  });

  // Reviewer finding: Config's subtitle once near-duplicated Query's ("The
  // query this endpoint serves, and how callers reach it."), contradicting
  // Config's own module doc that nothing here decides what the query
  // returns. Pin Config's own copy and pin the absence of Query's, so the
  // two sections' subtitles can't drift back into saying the same thing.
  it("has its own subtitle, not Query's", async () => {
    renderConfig();
    await screen.findByLabelText("Name");
    expect(
      screen.getByText("The endpoint as an HTTP object: the URL contract callers use, and the cache, limits, and rate-limit policy applied to their calls."),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("The query this endpoint runs, and the parameters callers may pass it."),
    ).not.toBeInTheDocument();
  });

  // Spec §3.4: Validate runs the SAVED draft, not what's on screen — unlike
  // Preview. The label has to say so, or an author mid-edit reads "Validate"
  // as checking their current changes.
  it("labels Validate as checking the SAVED draft", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderConfig(endpoint);
    expect(await screen.findByRole("button", { name: /validate saved draft/i })).toBeInTheDocument();
  });

  /**
   * `POST /validate` answers with `statements` — one entry per statement, each
   * carrying the columns `describe` resolved for it. It carried a single
   * `columns` before M2, and the editor still read that key: the field is now
   * `undefined`, so the result read "Resolves to 0 columns" for every endpoint
   * whose SQL was perfectly fine. Nothing failed at build time, because nothing
   * gates the generated client against `openapi.json`.
   *
   * Two statements in the fixture, and the columns of the SECOND asserted:
   * reading `statements[0]` for everything is the shape a one-statement fixture
   * cannot tell from reading each entry.
   */
  it("reports every statement's resolved columns, not the first one's", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    validateMutateAsync.mockResolvedValue({
      status: 200,
      data: {
        statements: [
          { name: "rows", sql: "SELECT …", columns: [{ name: "order_id", ty: "varchar" }] },
          {
            name: "total",
            sql: "SELECT count(*)",
            columns: [{ name: "n", ty: "bigint" }, { name: "pages", ty: "integer" }],
          },
        ],
      },
      headers: new Headers(),
    });
    renderConfig(endpoint);
    await userEvent.click(await screen.findByRole("button", { name: /validate saved draft/i }));

    const alert = await screen.findByRole("alert");
    expect(within(alert).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      "rows: 1 column — order_id (varchar)",
      "total: 2 columns — n (bigint), pages (integer)",
    ]);
  });

  // The common case keeps the sentence it had. A one-statement endpoint's
  // response is an ARRAY, so its name is not a key any caller reads, and
  // putting `main:` in front of the answer would be the first place on this
  // page to suggest otherwise.
  it("keeps the one-statement result a plain sentence with no statement name", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    validateMutateAsync.mockResolvedValue({
      status: 200,
      data: {
        statements: [
          {
            name: "main",
            sql: "SELECT …",
            columns: [{ name: "order_id", ty: "varchar" }, { name: "total_revenue", ty: "bigint" }],
          },
        ],
      },
      headers: new Headers(),
    });
    renderConfig(endpoint);
    await userEvent.click(await screen.findByRole("button", { name: /validate saved draft/i }));

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Resolves to 2 columns: order_id (varchar), total_revenue (bigint)");
    expect(within(alert).queryByRole("listitem")).not.toBeInTheDocument();
    expect(alert).not.toHaveTextContent("main");
  });

  // Phase 5 §5: the four groups used to be four `ConfigSection` cards, each
  // framing a table of one or two rows — a column of mostly-empty boxes. They
  // are now group rows of ONE table.
  //
  // Every assertion here has to die on a wrong implementation, which is why
  // this test does not stop at counting tables: the group rows are looked up
  // *inside* that table, and the four identity fields are located by row index
  // between the `Endpoint` group row and the `Cache` one. A `ConfigTableGroup`
  // that rendered nothing, a group row rendered outside the table, or the
  // identity form left sitting above the table all fail — and none of those
  // would disturb a bare table count.
  it("renders its groups as rows of one table, not as separate cards", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderConfig(endpoint);
    await screen.findByLabelText("Name");

    // Two tables would mean the cards were renamed rather than removed.
    expect(screen.getAllByRole("table")).toHaveLength(1);
    const table = screen.getByRole("table");
    const rows = within(table).getAllByRole("row");
    const rowIndexOf = (el: HTMLElement) => rows.indexOf(el.closest("tr") as HTMLElement);
    const groupAt = (name: string) => rowIndexOf(within(table).getByRole("rowheader", { name }));

    const endpointAt = groupAt("Endpoint");
    const cacheAt = groupAt("Cache");
    const limitsAt = groupAt("Limits");
    const rateLimitAt = groupAt("Rate limit");
    expect(endpointAt).toBeLessThan(cacheAt);
    expect(cacheAt).toBeLessThan(limitsAt);
    expect(limitsAt).toBeLessThan(rateLimitAt);

    // The identity fields are rows of that same table, under the `Endpoint`
    // group — not a form above it. This is the half of the change a table
    // count cannot see.
    for (const field of ["Name", "Description", "Method", "Path"]) {
      const at = rowIndexOf(within(table).getByLabelText(field));
      expect(at).toBeGreaterThan(endpointAt);
      expect(at).toBeLessThan(cacheAt);
    }
  });

  // Fix round 1, Minor 1: the reviewer replaced Name's `onChange` with
  // `() => {}` and the whole file stayed green — the four identity controls'
  // wiring was pinned by nothing, and this task rewrote the JSX around all
  // four of them. Four near-identical rows edited by hand is exactly how a
  // control ends up bound to its neighbour's field, so this drives all four
  // and asserts the WHOLE save payload with `toEqual`, not one key: a dead
  // `onChange` drops a key, and a cross-wired one puts the value under
  // someone else's. The values are deliberately distinct from each other and
  // from every default in `makeEndpoint`, so no swap can coincide.
  it("carries every identity field's edit to Save under its own key", async () => {
    const user = userEvent.setup();
    renderConfig();
    await screen.findByLabelText("Name");

    await user.clear(screen.getByLabelText("Name"));
    await user.type(screen.getByLabelText("Name"), "Weekly report");
    await user.clear(screen.getByLabelText("Description"));
    await user.type(screen.getByLabelText("Description"), "Rows for the weekly digest");
    await user.clear(screen.getByLabelText("Path"));
    await user.type(screen.getByLabelText("Path"), "reports/weekly");
    await user.click(screen.getByRole("combobox", { name: "Method" }));
    await user.click(await screen.findByRole("option", { name: "POST" }));

    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    // The shell PATCHes only what changed (`diffToUpdateInput`), so these four
    // keys and nothing else is the exact contract of "four edits, four fields".
    expect(updateMutateAsync.mock.calls[0][0].data).toEqual({
      name: "Weekly report",
      description: "Rows for the weekly digest",
      method: "POST",
      path: "reports/weekly",
    });
  });
});

// KI-023. The served URL is the LIVE route, which only publish moves; a new
// path or method on the draft waits for the next publish, and the row it is
// typed in says so, naming the URL callers still use.
describe("EndpointDetail — Config, a path that goes live at publish", () => {
  const live = { state: "live", live_version_id: "v-1", live_method: "GET", live_path: "/reports/daily" } as const;

  it("says a typed path goes live when you publish, with the URL callers still use", async () => {
    const user = userEvent.setup();
    renderConfig(makeEndpoint({ ...live, path: "/reports/daily" }));
    const path = await screen.findByLabelText("Path");
    expect(screen.queryByText(/goes live when you publish/i)).not.toBeInTheDocument();

    await user.clear(path);
    await user.type(path, "orders/lookup");
    const note = screen.getByText(/the new path goes live when you publish/i);
    expect(note).toHaveTextContent("Callers still use GET /reports/daily.");
    expect(path).toHaveAttribute("aria-describedby", note.id);
  });

  it("keeps saying so once the new path is saved and not yet published", async () => {
    renderConfig(makeEndpoint({ ...live, path: "/orders/lookup" }));
    await screen.findByLabelText("Path");
    expect(screen.getByText(/the new path goes live when you publish/i)).toHaveTextContent(
      "Callers still use GET /reports/daily.",
    );
  });

  it("does not call the same path in another spelling a move", async () => {
    renderConfig(makeEndpoint({ ...live, path: "reports/daily/" }));
    await screen.findByLabelText("Path");
    expect(screen.queryByText(/goes live when you publish/i)).not.toBeInTheDocument();
  });

  it("says nothing on an endpoint never published", async () => {
    renderConfig(makeEndpoint({ path: "/orders/lookup", live_method: null, live_path: null }));
    await screen.findByLabelText("Path");
    expect(screen.queryByText(/goes live when you publish/i)).not.toBeInTheDocument();
  });
});

describe("EndpointDetail — save invalidation", () => {
  // Reviewer finding (Important #5): the generated `useDataApiUpdateEndpoint`
  // attaches no invalidation of its own, so a rename would leave the OLD name
  // in the header and the endpoints list stale, forever, until a hard reload.
  // This is section-agnostic (the mutation and its `onSuccess` live in the
  // shared `EndpointDetailInner` shell, not in any one section), so it is
  // exercised through Config rather than duplicated per section.
  it("invalidates the endpoint and list query caches after a successful save", async () => {
    const { qc } = renderConfig();
    await screen.findByLabelText("Name");
    const invalidateSpy = vi.spyOn(qc, "invalidateQueries");

    const opts = vi.mocked(api.useDataApiUpdateEndpoint).mock.calls.at(-1)?.[0];
    await opts?.mutation?.onSuccess?.(
      { status: 200, data: makeEndpoint(), headers: new Headers() } as never,
      { workspace: "eng", id: "ep-1", data: {} } as never,
      undefined as never,
      undefined as never,
    );

    expect(invalidateSpy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiGetEndpoint", "eng", "ep-1"] }),
    );
    expect(invalidateSpy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListEndpoints", "eng"] }),
    );
  });
});

/**
 * Cache/Limits/Rate limit moved from bare Label+control pairs to the
 * `Setting | Details | Value` layout the control-plane engine config pages
 * use (`ConfigTable`/`ConfigTableRow`). This pins the one thing that layout
 * exists for — a Details explanation actually renders in the same row as its
 * control — so a future edit can't silently drop the `details` prop back to
 * nothing and leave the column blank again.
 */
describe("Config — config table Details column", () => {
  it("shows the Max rows Details copy in the same row as its control", async () => {
    renderConfig();
    const maxRows = await screen.findByLabelText("Max rows");
    const row = maxRows.closest("tr");
    expect(row).not.toBeNull();
    // The row renders the Details text twice (a `lg:table-cell` Details
    // column, and an `lg:hidden` inline copy for narrow screens) — jsdom
    // doesn't evaluate either media query, so both are present in the DOM;
    // asserting "at least one" is what "renders next to its control" means
    // here, matching how this codebase already asserts duplicated
    // responsive text elsewhere (see EngineDetail.test.tsx's `no delegation`
    // assertion).
    expect(
      within(row as HTMLElement).getAllByText(
        /hard cap on rows returned\. a result larger than this fails the call rather than being silently truncated/i,
      ).length,
    ).toBeGreaterThan(0);
  });

  /**
   * A rate limit of `0` is the value every endpoint has until somebody sets
   * one, and on a security-shaped setting "unlimited" and "blocked" are
   * opposites. The answer is not a matter of taste — it is in
   * `crates/egret-dataapi/src/serving/ratelimit.rs`:
   *
   *   - `Rate::from_spec` (`:86-88`) returns `None` for `qps == 0`, and
   *     `check_request` (`:202`, `:214`) then installs no bucket at all. So `0`
   *     on either qps field means NO LIMIT. Its own doc says so (`:73-81`):
   *     "Treating that as 'zero requests per second' would make every default
   *     endpoint unreachable; it means unlimited."
   *   - `burst == 0` is a DIFFERENT sentinel (`:91-95`): the burst becomes the
   *     qps, i.e. one second of allowance. Not unlimited, and not zero. And it
   *     is passed only for the per-KEY bucket — `:214` hard-codes `0` for the
   *     endpoint-wide one — so a burst with no `per_key_qps` does nothing at
   *     all.
   *
   * The three rows shipped one placeholder, "unlimited", on all three. Two were
   * right for the wrong reason (they described the EMPTY box, and a stored `0`
   * renders as `0` and never shows a placeholder) and the third was simply
   * false.
   */
  /**
   * The row above the rate limits carries the same nullish trap and a THIRD
   * sentinel, and its `placeholder="unlimited"` was false twice over. From the
   * backend, not from the shape of the neighbouring rows:
   *
   *   - `crates/egret-dataapi-api/src/lib.rs:172-183` — `LimitsSpec` defaults
   *     `max_rows` to **10,000** and `timeout_seconds` to **30**, not to `0`.
   *     So a blank box is the default, not "no limit".
   *   - `crates/egret-dataapi/src/serving/router.rs:2054-2057` — serving clamps
   *     both: `max_rows.clamp(1, MAX_ROWS_CEILING)` and
   *     `timeout_seconds.clamp(1, TIMEOUT_CEILING_SECS)`. So `0` means ONE ROW
   *     and ONE SECOND — the opposite of unlimited — and `:2718`'s own test
   *     spells it out: "a 0 cap must not mean 'return nothing'".
   *   - `router.rs:239`, `:244` — the ceilings are 1,000,000 rows and 300
   *     seconds. **There is no value that means unlimited.**
   */
  it("says what the row and time limits default to, and that neither 0 means unlimited", async () => {
    renderConfig();
    const maxRows = await screen.findByLabelText("Max rows");
    const timeout = screen.getByLabelText("Timeout (seconds)");
    const rowText = (el: HTMLElement) => (el.closest("tr") as HTMLElement).textContent ?? "";

    // What the empty box does, and what the value an author might type as
    // "no limit" actually does.
    expect(rowText(maxRows)).toMatch(/10,000/);
    expect(rowText(maxRows)).toMatch(/one row/i);
    expect(rowText(timeout)).toMatch(/\b30\b/);
    expect(rowText(timeout)).toMatch(/one second/i);

    // The word that was there before, on a setting where it is not reachable
    // at all. This is the assertion that fails on the shipped copy.
    expect(rowText(maxRows)).not.toMatch(/\bunlimited\b/i);
    expect(maxRows).toHaveAttribute("placeholder", "10,000 — the default");
    expect(timeout).toHaveAttribute("placeholder", "30 — the default");
  });

  it("says what a rate limit of 0 does, and does not say the same thing for Burst", async () => {
    renderConfig();
    const perCaller = await screen.findByLabelText("Per caller (qps)");
    const burst = screen.getByLabelText("Burst");
    const perEndpoint = screen.getByLabelText("Endpoint-wide (qps)");

    // The row's text, not the Details cell's: the copy renders twice (the
    // `lg:table-cell` column and the `lg:hidden` inline copy), which is how
    // this file already asserts Details copy above.
    const rowText = (el: HTMLElement) => (el.closest("tr") as HTMLElement).textContent ?? "";

    expect(rowText(perCaller)).toMatch(/no per-caller limit/i);
    expect(rowText(perEndpoint)).toMatch(/no endpoint-wide limit/i);

    // Burst's zero is the other sentinel, and the row has to say both halves:
    // what 0 means, and that it is inert without a per-caller rate.
    expect(rowText(burst)).toMatch(/one second of the per-caller rate/i);
    expect(rowText(burst)).toMatch(/per caller \(qps\)/i);

    // The discriminating half. All three rows said "unlimited" before, so a
    // test that only checked the two qps rows would pass on the copy that was
    // wrong — and would pass again the next time somebody "makes the three
    // rows consistent".
    expect(rowText(burst)).not.toMatch(/\bunlimited\b/i);
    expect(perCaller).toHaveAttribute("placeholder", "0 — unlimited");
    expect(perEndpoint).toHaveAttribute("placeholder", "0 — unlimited");
    expect(burst.getAttribute("placeholder")).not.toBe("0 — unlimited");
    expect(burst.getAttribute("placeholder")).toMatch(/per-caller rate/i);
  });
});

function renderResponse(endpoint: DataApiEndpointDto = makeEndpoint()) {
  vi.mocked(api.useDataApiGetEndpoint).mockReturnValue(
    { data: { status: 200, data: endpoint }, isLoading: false, isError: false, error: null, refetch: vi.fn() } as unknown as
      ReturnType<typeof api.useDataApiGetEndpoint>,
  );

  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/explore/eng/data-api/endpoints/${endpoint.id}/response`]}>
        <Routes>
          <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
            <Route path="response" element={<ResponseSection />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

// `servers[0].url` already carries the workspace slug (P5a fixed the backend
// for exactly this — see `openapi_gen.rs`) — the fixture pins that shape so a
// regression that drops the slug or reconstructs the URL from parts shows up
// here rather than only in a live check.
const OPENAPI_FIXTURE = {
  openapi: "3.0.3",
  info: { title: "Data API", version: "1.0.0" },
  servers: [{ url: "https://egret.test/data-api/v1/analytics" }],
  paths: {},
};

// `columns` is a second parameter rather than something a caller has to
// reconstruct the whole `snapshot` to change: the frozen-column table is a
// LIST, and a one-row fixture cannot tell "rendered the row it was given" apart
// from "rendered row 0" (§8) — the trap this branch has now paid for three
// times. Default kept at the single column the pre-existing tests were written
// against so none of them change meaning.
function makeVersion(
  overrides: Partial<DataApiEndpointVersionDto> = {},
  // `ColumnSpec`, not a local `{ name; ty }`, because `required` is the third
  // field and the whole point of the tests below is which value of it the row
  // was built from. The generated type has it OPTIONAL (`required?: boolean`)
  // — it carries a serde default server-side and so is absent from the
  // schema's `required` list — which is exactly the shape a fixture must be
  // able to express to pin `undefined` meaning "guaranteed".
  columns: ColumnSpec[] = [{ name: "total_revenue", ty: "bigint" }],
  // A snapshot freezes a LIST of statements (M2), each with its own frozen
  // columns — `columns` above builds the one-statement case every test in this
  // file was written against, and this overrides it outright for the few that
  // are about several. Both spellings rather than only the list, so none of the
  // pre-existing tests had to be rewritten to keep meaning what they meant.
  statements?: StatementSnapshot[],
): DataApiEndpointVersionDto {
  return {
    id: "v-1",
    version: 1,
    note: "",
    is_live: true,
    created_by: "user-1",
    created_at: "2026-08-01T00:00:00Z",
    snapshot: {
      statements: statements ?? [{ name: "main", sql: "SELECT 1", columns }],
      method: "GET",
      path: "reports/daily",
      database_id: "db-1",
      params: [],
      output: {},
      cache: {},
      limits: {},
      rate_limit: {},
      delegated: false,
    },
    ...overrides,
  } as unknown as DataApiEndpointVersionDto;
}

describe("ResponseSection", () => {
  beforeEach(() => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      { data: { status: 200, data: [] }, isLoading: false, isError: false } as unknown as
        ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    vi.mocked(api.useDataApiListServiceAccounts).mockReturnValue(
      {
        data: {
          status: 200,
          data: [{ id: "sa-1", name: "svc", description: "", email: "svc@x", created_at: "2026-01-01T00:00:00Z" }],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListServiceAccounts>,
    );
  });

  it("shows the served base URL beside the download", async () => {
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: { status: 200, data: OPENAPI_FIXTURE }, isError: false } as unknown as
        ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    renderResponse();
    // OPENAPI_FIXTURE.servers[0].url — the slug is part of it; see the backend note above.
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
    // Scoped to the error alert, not a bare `findByText`: the Naming row's
    // own Details copy ("...re-published") now also matches `/re-publish/i`
    // (rendered twice, per the Details-column note above), so an unscoped
    // query would fail with "multiple elements found" rather than pinning
    // the OpenAPI card's error message specifically.
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(/re-publish/i)).toBeInTheDocument();
    expect(within(alert).getByText(/daily/)).toBeInTheDocument();
  });

  it("picks the is_live version's columns, not a superseded one's", async () => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: {
          status: 200,
          data: [
            makeVersion({ id: "v-0", version: 1, is_live: false }),
            makeVersion({ id: "v-1", version: 2, is_live: true }),
          ],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    renderResponse(makeEndpoint({ live_version_id: "v-1" }));
    expect(await screen.findByText("total_revenue")).toBeInTheDocument();
    expect(screen.getByText("bigint")).toBeInTheDocument();
  });

  // A draft endpoint (never published) has no frozen columns yet — rendering
  // an empty table here would misrepresent "nothing published" as "publishes
  // zero columns", so this must be a distinct message, not an empty table.
  it("says there is no live version rather than showing the draft's columns", async () => {
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    renderResponse(makeEndpoint({ live_version_id: null, state: "draft" }));
    expect(await screen.findByText(/no live version/i)).toBeInTheDocument();
  });

  // Reviewer finding (Important #1): `columnsLoading` alone was provable only
  // by reading the code. A published endpoint whose versions fetch hasn't
  // landed yet must show a loading state, not "no live version yet" — the
  // exact conflation this test and the next one pin apart.
  it("shows a loading state, not the publish prompt, while a live endpoint's versions are still in flight", async () => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      { data: undefined, isLoading: true, isError: false } as unknown as
        ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    renderResponse(makeEndpoint({ live_version_id: "v-1" }));
    expect(await screen.findByText("Loading…")).toBeInTheDocument();
    expect(screen.queryByText(/no live version/i)).not.toBeInTheDocument();
  });

  // Reviewer finding (Important #1): react-query clears `isLoading` on ANY
  // settle, error included, so a versions-fetch failure for a LIVE endpoint
  // would otherwise fall into the same branch as "never published" — telling
  // an author who has already published to do the one thing they've done.
  it("reports the versions fetch failing as an error, not as never having been published", async () => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: undefined,
        isLoading: false,
        isError: true,
        error: new Error("network error"),
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    renderResponse(makeEndpoint({ live_version_id: "v-1" }));
    expect(await screen.findByText(/could not load the published contract/i)).toBeInTheDocument();
    expect(screen.queryByText(/no live version/i)).not.toBeInTheDocument();
  });

  it("changes only the touched output field when the shape is switched, via a partial PATCH", async () => {
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    const endpoint = makeEndpoint({ output: { shape: "objects", naming: "snake" } });
    const user = userEvent.setup();
    renderResponse(endpoint);

    await user.click(screen.getByRole("combobox", { name: "Shape" }));
    await user.click(await screen.findByRole("option", { name: "Arrays" }));
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    const call = updateMutateAsync.mock.calls[0][0];
    // `naming` must survive untouched — rebuilding `output` from scratch
    // (rather than spreading the existing object) is exactly the mistake
    // Task 3's review flagged: it would silently drop or reorder fields no
    // control on this page even touched.
    expect(call.data).toEqual({ output: { shape: "arrays", naming: "snake" } });
  });

  // Shape's sibling. Two near-identical rows written by hand is exactly how a
  // control ends up bound to its neighbour's field (Task 2 hit this with four
  // of them), and this task rewrote the JSX around both. Asserting the WHOLE
  // PATCH body catches it: the shell sends only what changed, so a Naming
  // control wired to `shape` puts "camel" under `shape` and leaves `naming` at
  // its fixture value — a `naming: "camel"`-only assertion would miss that,
  // and a dead `onValueChange` drops the key entirely. The fixture's starting
  // values are distinct from the ones typed here so no swap can coincide.
  it("changes only the touched output field when the naming is switched, via a partial PATCH", async () => {
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    const endpoint = makeEndpoint({ output: { shape: "arrays", naming: "as_is" } });
    const user = userEvent.setup();
    renderResponse(endpoint);

    await user.click(screen.getByRole("combobox", { name: "Key naming" }));
    await user.click(await screen.findByRole("option", { name: "camelCase" }));
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    expect(updateMutateAsync.mock.calls[0][0].data).toEqual({
      output: { shape: "arrays", naming: "camel" },
    });
  });

  // Phase 5 §5, the Response half: "Output shape" used to be a `ConfigSection`
  // card framing a table of two rows. It is now a group ROW of that table.
  //
  // The two assertions on each block are deliberately opposite, so neither a
  // half-done conversion nor an over-eager one passes. `ConfigSection` renders
  // its title as an `<h3>`; `ConfigTableGroup` renders it as a `rowheader`
  // cell. So "Output shape" must be a rowheader and NOT a heading (a leftover
  // card fails), while "Frozen columns" and "OpenAPI document" must stay
  // headings of their own and NOT be rowheaders (a cleanup that swallows either
  // into the config table fails). A group row that rendered nothing fails the
  // `getByRole("rowheader")` lookup, which is also the precondition the
  // row-order assertions below are computed from — they cannot pass without it.
  //
  // The third assertion is this task's, and is the one that changed: those two
  // blocks are no longer `Card`s either. #521 kept them on the argument that
  // neither is `Setting | Details | Value` shaped — still true, and still why
  // neither is a rowheader — but §4.3 later supplied the option that argument
  // did not have: a heading plus a plain `Table` (or plain prose) in the
  // section rhythm. `bg-card` is the probe because it is the one class the
  // `Card` primitive contributes that nothing else on this section renders
  // (`ConfigTable`, `Table`, `Alert`, `Select` and the download dialog were all
  // grepped for it, and none carry it).
  it("renders Output shape as a group row while the columns and document stay their own, cardless blocks", async () => {
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: { status: 200, data: OPENAPI_FIXTURE }, isError: false } as unknown as
        ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    // A live version so the Frozen columns card renders its own real `<table>`
    // — the harder case, and the one where scoping by "the table holding the
    // controls" rather than by a bare table count actually matters.
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      { data: { status: 200, data: [makeVersion()] }, isLoading: false, isError: false } as unknown as
        ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderResponse(makeEndpoint({ live_version_id: "v-1" }));

    const shape = await screen.findByRole("combobox", { name: "Shape" });
    const configTable = shape.closest("table") as HTMLElement;
    expect(configTable).not.toBeNull();

    // The group heading is a row of the very table that holds the controls.
    const group = within(configTable).getByRole("rowheader", { name: "Output shape" });
    // ...and it spans the whole row. A group occupying only the Setting column
    // reads as a setting with no value — the confusion the cards were removed
    // to fix.
    expect(group).toHaveAttribute("colspan", "3");
    // No `ConfigSection` left behind: that would render "Output shape" as an h3.
    expect(screen.queryByRole("heading", { name: "Output shape" })).not.toBeInTheDocument();

    const rows = within(configTable).getAllByRole("row");
    const rowIndexOf = (el: HTMLElement) => rows.indexOf(el.closest("tr") as HTMLElement);
    const groupAt = rowIndexOf(group);
    for (const control of ["Shape", "Key naming"]) {
      const at = rowIndexOf(within(configTable).getByRole("combobox", { name: control }));
      expect(at).toBeGreaterThan(groupAt);
    }

    // These two are NOT `Setting | Details | Value` shaped — a frozen column
    // list and a download button — so neither is a row of the table above…
    for (const block of ["Frozen columns", "OpenAPI document"]) {
      const title = screen.getByRole("heading", { name: block });
      expect(title.closest("table")).toBeNull();
      expect(screen.queryByRole("rowheader", { name: block })).not.toBeInTheDocument();
      // …and neither is framed by a `Card` any more.
      expect(title.closest(".bg-card")).toBeNull();
    }
  });

  /**
   * `OpenAPI document` is an ACTION, not a setting (§4.2), so it stays out of
   * the settings table — and this task did not change that. What changed is
   * the SHAPE it takes outside the table. It was a heading, a paragraph, then
   * a button on a line of its own: a third block layout on a tab that already
   * has a read-only listing and a settings table, which is the "four shapes"
   * complaint this task exists to answer.
   *
   * `Config.tsx`'s `Validate` block is the same KIND of thing — an action at
   * the end of a settings tab — and it puts the heading and the button on ONE
   * line under a `border-t`. That is the shape on the tab the user called
   * fine, so this one takes it rather than inventing a third.
   *
   * Asserted as a SHARED PARENT rather than as a class: the identity holds
   * only while the two are on one line, and it cannot be satisfied by the
   * section root, which the second assertion rules out by naming something
   * the root does hold and that line must not.
   */
  it("ends the tab with the OpenAPI action on its heading line, the way Config's Validate block does", async () => {
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: { status: 200, data: OPENAPI_FIXTURE }, isError: false } as unknown as
        ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    renderResponse();

    const heading = await screen.findByRole("heading", { name: "OpenAPI document" });
    const button = screen.getByRole("button", { name: "Download OpenAPI" });
    expect(button.parentElement).toBe(heading.parentElement);
    // The heading LINE, not the whole block: the explanatory sentence is a
    // sibling of that line, so a parent holding a `<p>` means the button was
    // dropped back into the block (or that both are sitting in the section
    // root, which holds the intro paragraph).
    expect((heading.parentElement as HTMLElement).querySelector("p")).toBeNull();
    // …and the sentence is still on the page. Without this, the assertion
    // above passes just as well on a block that lost its explanation.
    expect(screen.getByText(/Generated per service account/i)).toBeInTheDocument();
  });

  // The frozen column list is a LIST (§4.3), so it is the shared `Table`, and
  // its states are rows INSIDE it rather than a branch that replaces it. That
  // is not a cosmetic difference: with the card gone, a branch that replaced
  // the table would take `Name` and `Type` with it, and an author who has never
  // published would get a sentence with nothing on screen saying what a frozen
  // column even is — the job the card title used to do alone.
  //
  // All three states are checked in one test on purpose: the regression is
  // "someone re-writes ONE branch as a replacement", and a test covering only
  // the empty state would pass while the loading branch did it.
  it("names the Name and Type columns in every state of the frozen column list", async () => {
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );

    const states: [string, () => void, RegExp][] = [
      [
        "never published",
        () =>
          vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
            { data: { status: 200, data: [] }, isLoading: false, isError: false } as unknown as
              ReturnType<typeof api.useDataApiListEndpointVersions>,
          ),
        /no live version/i,
      ],
      [
        "loading",
        () =>
          vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
            { data: undefined, isLoading: true, isError: false } as unknown as
              ReturnType<typeof api.useDataApiListEndpointVersions>,
          ),
        /Loading…/,
      ],
      [
        "errored",
        () =>
          vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
            { data: undefined, isLoading: false, isError: true, error: new Error("network error") } as unknown as
              ReturnType<typeof api.useDataApiListEndpointVersions>,
          ),
        /could not load the published contract/i,
      ],
    ];

    for (const [name, arrange, message] of states) {
      arrange();
      // A live endpoint for the loading/error states (both are gated on
      // `live_version_id`); the empty state's own mock returns no versions, so
      // it lands on the "no live version" branch either way.
      const { unmount } = renderResponse(makeEndpoint({ live_version_id: "v-1" }));

      const message_el = await screen.findByText(message);
      const table = message_el.closest("table") as HTMLElement;
      // The state is a row of the list, not something rendered instead of it.
      expect(table, name).not.toBeNull();
      expect(
        within(table)
          .getAllByRole("columnheader")
          .map((th) => th.textContent),
        name,
      ).toEqual(["Name", "Type"]);
      // Spanning the whole list — a sentence in the Name column with an empty
      // Type cell beside it is the shape §4.1 records as the confusion the
      // cards were removed to fix, applied to a row.
      expect(message_el.closest("td"), name).toHaveAttribute("colspan", "2");

      unmount();
    }
  });

  // Two columns, and the assertion is a deep-equal of the WHOLE grid. A
  // one-row fixture makes "put the type beside its own name" and "put row 0's
  // type beside row 0's name" the same claim, which is why this branch keeps
  // re-learning it (§8). With two, transposing the cells, dropping a row, or
  // reading `columns[0].ty` for every row each produce a different array.
  it("puts each frozen column's type in its own row", async () => {
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: {
          status: 200,
          data: [
            makeVersion({ id: "v-1", version: 2, is_live: true }, [
              { name: "order_id", ty: "varchar" },
              { name: "total_revenue", ty: "bigint" },
            ]),
          ],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderResponse(makeEndpoint({ live_version_id: "v-1" }));

    const first = await screen.findByText("order_id");
    const body = (first.closest("table") as HTMLElement).querySelector(
      "tbody",
    ) as HTMLTableSectionElement;
    expect(
      Array.from(body.rows).map((r) => Array.from(r.cells).map((c) => c.textContent)),
    ).toEqual([
      ["order_id", "varchar"],
      ["total_revenue", "bigint"],
    ]);
  });

  /**
   * The branching-columns half of the contract, and the half an author could
   * not see: a published endpoint's frozen `columns` are the UNION of every
   * branch publish described, and `required` says whether every branch returns
   * that column. A consumer reads the distinction twice — in the served
   * `meta.columns` and in the generated document's per-schema `required` list.
   * The editor rendered `Name | Type` only, so a column a caller can rely on
   * and one that vanishes on some parameter values looked identical.
   *
   * **A badge on the rows that differ, not a third column.** §4.3: "a column
   * whose every cell is identical is section-level copy in disguise". Every
   * endpoint published before this branch, and every endpoint whose definition
   * does not branch, has `required` true on every column — so a `Required`
   * column would hold one repeated word in every cell of the common case,
   * while costing width in the one table that was measured down to `max-w-xl`
   * *because* the type had drifted away from the name it describes (§5.4b —
   * §5.1 carried that measurement until its reversal moved it out). Marking
   * only the exceptions leaves that case byte-identical to what it renders
   * today, which is why neither test above changed.
   *
   * The two tests below are deliberately separable, one per mutation
   * direction. Dropping the badge reddens the first and leaves the second
   * green; flagging every column — the `!c.required` spelling that reads
   * natural and inverts the feature — reddens the second and leaves the first
   * green. A single test asserting the whole grid would have been shorter and
   * would have collapsed both directions into one signal.
   */
  function renderColumns(columns: ColumnSpec[]) {
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: { status: 200, data: [makeVersion({ id: "v-1", version: 2, is_live: true }, columns)] },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    return renderResponse(makeEndpoint({ live_version_id: "v-1" }));
  }

  // Direction one: the indicator exists, and it is on the row it describes
  // rather than merely somewhere on the page. Two columns in the fixture and
  // the badge scoped to `order_id`'s ROW, so "rendered a badge" cannot pass for
  // "rendered the badge beside the column that earned it" (§8).
  it("marks a column that only some branches return", async () => {
    renderColumns([
      { name: "refund_reason", ty: "varchar", required: false },
      { name: "total_revenue", ty: "bigint", required: true },
    ]);

    const optionalRow = (await screen.findByText("refund_reason")).closest("tr") as HTMLElement;
    expect(within(optionalRow).getByText(/may be absent/i)).toBeInTheDocument();
  });

  // Direction two, and the one a careless implementation passes by accident.
  // `required` is ABSENT from the schema's own `required` array — it carries a
  // serde default of `true` on the Rust side, precisely so a snapshot frozen
  // before this field existed keeps exact-match semantics — so TypeScript types
  // it `required?: boolean` and `undefined` MUST read as guaranteed. The
  // `total_revenue` row is that case: no `required` key at all, the literal
  // shape of every endpoint published before this branch. Flagging it would
  // tell an author that every column of every pre-existing endpoint may be
  // missing, which is the opposite of what the server does with it.
  it("leaves a guaranteed column unmarked, including one frozen before the flag existed", async () => {
    renderColumns([
      { name: "order_id", ty: "varchar", required: true },
      { name: "total_revenue", ty: "bigint" },
    ]);

    await screen.findByText("order_id");
    for (const name of ["order_id", "total_revenue"]) {
      const row = screen.getByText(name).closest("tr") as HTMLElement;
      expect(within(row).queryByText(/may be absent/i), name).not.toBeInTheDocument();
    }
  });

  // §4.3 again, the other half of the same rule: when the exception is a mark
  // rather than a column, what the ABSENCE of the mark means has to be said
  // once, above the table — otherwise an unmarked row is ambiguous between
  // "guaranteed" and "nobody knows". It is said only when a marked row is
  // actually on screen: an endpoint with no branching column has no marker to
  // explain, and a sentence about one would be the noise the badge was chosen
  // over a column to avoid.
  //
  // Both halves in one test, because "renders the sentence" is satisfied by an
  // implementation that always renders it, and that implementation is the
  // whole thing this asserts against.
  it("explains what an unmarked row means only where a marked row exists", async () => {
    const withOptional = renderColumns([
      { name: "refund_reason", ty: "varchar", required: false },
      { name: "total_revenue", ty: "bigint", required: true },
    ]);
    expect(await screen.findByText(/every other column is in every response/i)).toBeInTheDocument();
    withOptional.unmount();

    // Same two columns, same types, same order — only the flag differs, so a
    // difference here can be nothing but the flag.
    renderColumns([
      { name: "refund_reason", ty: "varchar", required: true },
      { name: "total_revenue", ty: "bigint", required: true },
    ]);
    await screen.findByText("refund_reason");
    expect(screen.queryByText(/every other column is in every response/i)).not.toBeInTheDocument();
  });

  /**
   * A snapshot freezes ONE column set PER STATEMENT (M2), and this section is
   * the only place an author reads the published contract.
   *
   * The pair is deliberately two tests, one per direction. Folding every
   * statement's columns into one table reddens the second and leaves the first
   * green; labelling the one-statement case with the name `main` — which no
   * caller ever reads, because a one-statement response is an ARRAY, not a
   * keyed object — reddens the first and leaves the second green.
   */
  it("shows a one-statement contract as the plain column list, with no statement name", async () => {
    renderColumns([
      { name: "order_id", ty: "varchar" },
      { name: "total_revenue", ty: "bigint" },
    ]);
    await screen.findByText("order_id");
    expect(screen.queryByText("main")).not.toBeInTheDocument();
    // One table, and it is the whole contract.
    const body = (screen.getByText("order_id").closest("table") as HTMLElement).querySelector(
      "tbody",
    ) as HTMLTableSectionElement;
    expect(Array.from(body.rows).map((r) => Array.from(r.cells).map((c) => c.textContent))).toEqual([
      ["order_id", "varchar"],
      ["total_revenue", "bigint"],
    ]);
  });

  it("gives each statement of a several-statement contract its own named list", async () => {
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: {
          status: 200,
          data: [
            makeVersion({ id: "v-1", version: 2, is_live: true }, undefined, [
              { name: "rows", sql: "SELECT …", columns: [{ name: "order_id", ty: "varchar" }] },
              { name: "total", sql: "SELECT count(*)", columns: [{ name: "n", ty: "bigint" }] },
            ]),
          ],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderResponse(makeEndpoint({ live_version_id: "v-1" }));

    // Each name labels its OWN table, so "rendered both names" cannot pass for
    // "put each column under the statement that returns it".
    const rowsTable = (await screen.findByText("order_id")).closest("table") as HTMLElement;
    const totalTable = screen.getByText("n").closest("table") as HTMLElement;
    expect(rowsTable).not.toBe(totalTable);
    expect(screen.getByRole("heading", { name: "rows" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "total" })).toBeInTheDocument();
    expect(within(rowsTable).queryByText("n")).not.toBeInTheDocument();
    expect(within(totalTable).queryByText("order_id")).not.toBeInTheDocument();
  });

  /**
   * `output.envelope` shipped with no control at all: `bare` was reachable
   * through the API and invisible to the person publishing the endpoint — the
   * same gap `required` had, and had to be come back for.
   *
   * Two directions again. Dropping the control reddens the first; hard-coding
   * the value the picker sends (`"bare"` because it is the one being chosen,
   * or `"wrapped"` because it is the default) reddens the second.
   */
  it("offers the envelope, defaulting to the wrapper a snapshot with no value reads back as", async () => {
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    // `output: {}` — no `envelope` key at all, which is what every endpoint
    // saved before this field existed carries and what the server reads as
    // `wrapped`.
    renderResponse(makeEndpoint({ output: {} }));
    expect(await screen.findByRole("combobox", { name: "Envelope" })).toHaveTextContent(/wrapped/i);
  });

  it("sends the chosen envelope, and only that field", async () => {
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    renderResponse(makeEndpoint({ output: { shape: "objects", naming: "as_is" } }));

    await userEvent.click(await screen.findByRole("combobox", { name: "Envelope" }));
    await userEvent.click(await screen.findByRole("option", { name: /^bare/i }));
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    // The whole `output`, not a `toMatchObject` on `envelope`: this control
    // shares an object with Shape and Naming, and a spread that drops either
    // silently reverts a setting the author never touched.
    expect(updateMutateAsync.mock.calls[0][0].data).toEqual({
      output: { shape: "objects", naming: "as_is", envelope: "bare" },
    });
  });
});

/**
 * The detail page for an endpoint that is IN THE TRASH.
 *
 * The Trash view links a row into this page by name — deliberately, and unlike
 * the row menu beside it, which drops its Open item because every action on a
 * trashed endpoint is a server-side 409. Before this block existed, that link
 * landed on the full six-section editor with an enabled Save, no banner and no
 * Restore; the way an author discovered the rule was `toast.error("Error:
 * endpoint is in the trash; restore it before editing it")` after typing.
 *
 * Every test here renders the SAME section with a trashed and a non-trashed
 * endpoint. That is the falsifiability requirement: "Save is absent" and "the
 * Name field is disabled" are both satisfied by a page that never renders Save
 * and disables everything always, and those two implementations are
 * indistinguishable from a one-state fixture.
 */
describe("EndpointDetail — a trashed endpoint", () => {
  const TRASHED = makeEndpoint({ state: "disabled", deleted_at: "2026-08-18T09:00:00Z" });

  it("says it is in the trash and offers Restore where Save would be", async () => {
    renderConfig(TRASHED);
    await screen.findByLabelText("Name");

    expect(screen.getByText("This endpoint is in the trash")).toBeInTheDocument();
    // Restore is named as the way forward, and the sentence says where it
    // lands — never straight back to serving.
    expect(screen.getByText(/never straight back to serving/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Restore" })).toBeInTheDocument();
    // Not disabled-Save: a greyed Save still claims this page edits things.
    expect(screen.queryByRole("button", { name: /^Sav/ })).not.toBeInTheDocument();
  });

  it("shows Save and no trash banner for an endpoint that is not trashed", async () => {
    // The other half of the test above. Without it, a shell that ALWAYS hid
    // Save and always rendered the banner would pass everything here.
    renderConfig(makeEndpoint());
    await screen.findByLabelText("Name");

    expect(screen.getByRole("button", { name: "Save" })).toBeInTheDocument();
    expect(screen.queryByText("This endpoint is in the trash")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Restore" })).not.toBeInTheDocument();
  });

  it("Restore sends this endpoint's id, and it is not inside the disabled region", async () => {
    const user = userEvent.setup();
    renderConfig(makeEndpoint({ id: "ep-77", state: "disabled", deleted_at: "2026-08-18T09:00:00Z" }));
    await screen.findByLabelText("Name");

    const restore = screen.getByRole("button", { name: "Restore" });
    // The `fieldset` that disables the editor must not swallow the one control
    // that gets the author out — a Restore button inside it would render
    // greyed and do nothing, which is the exact failure this whole block is
    // about, moved one level up.
    expect(restore).toBeEnabled();
    await user.click(restore);

    // The PAYLOAD, and an id that is not the fixture default, so a hardcoded
    // "ep-1" fails rather than coincidentally matching.
    await waitFor(() => {
      expect(restoreMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "ep-77" });
    });
  });

  it("makes the editor read-only, and typing into it changes nothing", async () => {
    const user = userEvent.setup();
    renderConfig(TRASHED);
    const name = await screen.findByLabelText("Name");

    // Disabled through the shell's `fieldset`, which is what covers the
    // controls no section remembered to check.
    expect(name).toBeDisabled();
    expect(screen.getByLabelText("Path")).toBeDisabled();
    await user.type(name, "edited");
    expect(name).toHaveValue("Daily report");
    expect(updateMutateAsync).not.toHaveBeenCalled();
  });

  it("leaves the editor editable when the endpoint is not in the trash", async () => {
    const user = userEvent.setup();
    renderConfig(makeEndpoint());
    const name = await screen.findByLabelText("Name");

    expect(name).toBeEnabled();
    await user.clear(name);
    await user.type(name, "Renamed");
    expect(name).toHaveValue("Renamed");
    // And the edit is real: Save wakes up, which is what "editing works" means
    // on this page.
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });
});

function renderVersions(
  endpoint: DataApiEndpointDto = makeEndpoint({ live_version_id: "v-2", state: "live" }),
  refetch: () => void = vi.fn(),
) {
  vi.mocked(api.useDataApiGetEndpoint).mockReturnValue(
    { data: { status: 200, data: endpoint }, isLoading: false, isError: false, error: null, refetch } as unknown as
      ReturnType<typeof api.useDataApiGetEndpoint>,
  );

  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/explore/eng/data-api/endpoints/${endpoint.id}/versions`]}>
        <Routes>
          <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
            <Route path="versions" element={<VersionsSection />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/**
 * The two sibling blocks that hold `a` and `b`: walk both up until they share
 * a parent, and return each one's ancestor that is a direct child of it.
 *
 * Written because `parentElement` (and worse, `parentElement.parentElement`)
 * answers a question about the tree the author EXPECTED. `Versions`' first
 * heading guard asserted `title.parentElement` held no button and was green on
 * the very layout it was written to forbid — at the base the `h2` was in a
 * bare inner div and the button was that div's sibling. This resolves the
 * block from the tree that is actually rendered, and throws rather than
 * degrading if the two share no ancestor.
 */
function blocksOf(a: HTMLElement, b: HTMLElement): [HTMLElement, HTMLElement] {
  const chain = (el: HTMLElement) => {
    const out: HTMLElement[] = [];
    for (let n: HTMLElement | null = el; n; n = n.parentElement) out.push(n);
    return out;
  };
  const ca = chain(a);
  const cb = chain(b);
  const common = ca.find((n) => cb.includes(n));
  if (!common) throw new Error("blocksOf: the two elements share no ancestor");
  const childOf = (c: HTMLElement[]) => {
    const found = c.find((n) => n.parentElement === common);
    if (!found) throw new Error("blocksOf: no direct child of the shared ancestor");
    return found;
  };
  return [childOf(ca), childOf(cb)];
}

const publishMutateAsync = vi.fn();
const rollbackMutateAsync = vi.fn();

describe("VersionsSection", () => {
  const versionsRefetchSpy = vi.fn();

  beforeEach(() => {
    versionsRefetchSpy.mockClear();
    publishMutateAsync.mockReset();
    rollbackMutateAsync.mockReset();

    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      { data: { status: 200, data: [] }, isLoading: false, isError: false } as unknown as
        ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    vi.mocked(api.useDataApiPublishEndpoint).mockReturnValue(
      { mutateAsync: publishMutateAsync, isPending: false } as unknown as
        ReturnType<typeof api.useDataApiPublishEndpoint>,
    );
    vi.mocked(api.useDataApiRollbackEndpoint).mockReturnValue(
      { mutateAsync: rollbackMutateAsync, isPending: false } as unknown as
        ReturnType<typeof api.useDataApiRollbackEndpoint>,
    );
    vi.mocked(api.getDataApiListEndpointVersionsQueryKey).mockImplementation(
      (ws, id) => ["dataApiListEndpointVersions", ws, id] as unknown as
        ReturnType<typeof api.getDataApiListEndpointVersionsQueryKey>,
    );

    publishMutateAsync.mockResolvedValue({
      status: 200,
      data: { version_id: "v-3", version: 3, state: "live" },
      headers: new Headers(),
    });
    rollbackMutateAsync.mockResolvedValue({
      status: 200,
      data: { version_id: "v-1", version: 1, state: "live" },
      headers: new Headers(),
    });
  });

  // Spec §3.4: Publish runs the SAVED draft, not what's on screen — unlike
  // Preview, the same distinction Config's "labels Validate as checking the
  // SAVED draft" test pins for Validate. Asserted here as its own test, not
  // only implicitly via the `{ name: "Publish saved draft" }` selector the
  // dirty-flag tests in `Query.test.tsx` already use to find this button —
  // that selector proves the label only as a side effect of finding
  // something to click.
  it("labels Publish as running the SAVED draft", async () => {
    renderVersions();
    expect(await screen.findByRole("button", { name: /publish saved draft/i })).toBeInTheDocument();
  });

  // The one thing this section must not get wrong (spec §6.3, second of its
  // three placements): this is the moment BEFORE the endpoint goes live, and
  // the one place the warning reaches an author who never opens Access. It
  // must not be a toast fired after the fact — Publish still has to be
  // confirmed once the warning is showing.
  it("warns that publishing does not grant data access before it publishes", async () => {
    renderVersions();
    await userEvent.click(await screen.findByRole("button", { name: /publish/i }));
    expect(await screen.findByText(/does not grant data access/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /publish/i })).toBeInTheDocument();
    expect(publishMutateAsync).not.toHaveBeenCalled();
  });

  it("lists each version with its note and who, and puts the live badge on the one live_version_id points at", async () => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: {
          status: 200,
          data: [
            makeVersion({ id: "v-1", version: 1, is_live: false, note: "initial", created_by: "user-1" }),
            makeVersion({ id: "v-2", version: 2, is_live: true, note: "fixed a typo", created_by: "user-2" }),
          ],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderVersions();

    expect(await screen.findByText("fixed a typo")).toBeInTheDocument();
    expect(screen.getByText("initial")).toBeInTheDocument();
    expect(screen.getByText("user-2")).toBeInTheDocument();
    // Exactly one Live badge — on v-2, not on both rows.
    expect(screen.getAllByText("Live")).toHaveLength(1);
  });

  /**
   * THE `Live` BADGE IS BOUND TO `CHIP_TONE`, NOT JUST TO THE WORD.
   *
   * This chip shipped as `bg-success/15 text-success` — 3.72:1 in light mode,
   * under WCAG 2.2 1.4.3's 4.5:1 for its 12px text — and moved onto the shared
   * map in `chipTone.ts` so that this `Live` and the header pill one click away
   * are the same chip. The test above finds the badge by its WORD, which is
   * true of a `variant="success"` badge (a hardcoded `bg-green-600`, not the
   * OKLCH token) and of an untoned one, so it does not hold the tone.
   *
   * Expected classes are read out of `CHIP_TONE.success` rather than typed, so
   * a future contrast change moves the map and this guard together. jsdom
   * resolves no `oklch()`; the ratios are measured in
   * `tests/e2e/dataapi-badge-ink.spec.ts`.
   */
  it("paints the live version's badge with CHIP_TONE.success", async () => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: {
          status: 200,
          data: [makeVersion({ id: "v-2", version: 2, is_live: true, note: "fixed a typo" })],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderVersions();
    // Scoped to the row: the shell's own header pill also says Live, and an
    // unscoped query could measure that one instead — the chip this branch
    // never touched — and pass with this call site reverted.
    const row = await screen.findByTestId("version-row-v-2");
    expectChipTone(within(row).getByText("Live"), "success", "Versions: Live");
  });

  // The defect this whole section exists to fix: on the live cluster the Who
  // column showed a raw user UUID (`5513767f-8cdd-4e48-9cf3-c493a74b683e`),
  // exactly the class of bug the Roles page was fixed for once already. The
  // DTO only ever carries the bare id (see `DataApiEndpointVersionDto`), so
  // resolving it to a name is a client-side lookup, the same shape
  // `ActivityCallList.tsx` uses for its trial-actor column.
  it("resolves created_by to the publisher's email, not the raw user id", async () => {
    const PUBLISHER_ID = "5513767f-8cdd-4e48-9cf3-c493a74b683e";
    vi.mocked(accessApi.listUsers).mockResolvedValue([
      { id: PUBLISHER_ID, email: "priya@egret.local", name: "Priya" },
    ] as unknown as accessApi.AccessUser[]);
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: {
          status: 200,
          data: [makeVersion({ id: "v-1", version: 1, is_live: true, created_by: PUBLISHER_ID })],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderVersions();

    expect(await screen.findByText("priya@egret.local")).toBeInTheDocument();
    expect(screen.queryByText(PUBLISHER_ID)).not.toBeInTheDocument();
  });

  /**
   * The other half of the `Who` widening, and the half the widening cannot do.
   *
   * That column is `truncate`, and its `w-80` is gated at `xl` (§5.3) — so
   * BELOW `xl` it is exactly the 224px column that was cutting realistic
   * addresses, unchanged. Without a `title` there is then no route at all to
   * the clipped value: the defect the fix is named after, left whole wherever
   * the fix does not reach. `Note` beside it has carried one since it shipped.
   *
   * Asserted on the CELL, not on the row, and with the em-dash case as the
   * negative half — an unconditional `title={who ?? "—"}` would satisfy the
   * first assertion and put a tooltip reading "—" on every unattributed row.
   */
  it("keeps a route to an address the narrow regime still clips", async () => {
    const PUBLISHER_ID = "5513767f-8cdd-4e48-9cf3-c493a74b683e";
    const LONG = "priyanka.venkataraman@egret.local";
    vi.mocked(accessApi.listUsers).mockResolvedValue([
      { id: PUBLISHER_ID, email: LONG, name: "P V" },
    ] as unknown as accessApi.AccessUser[]);
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: {
          status: 200,
          data: [
            makeVersion({ id: "v-2", version: 2, is_live: true, created_by: PUBLISHER_ID }),
            // A note, so the row's only em-dash is the one this test is about
            // — `Note` renders the same placeholder and `getByText` cannot
            // tell two of them apart.
            makeVersion({
              id: "v-1",
              version: 1,
              is_live: false,
              note: "first cut",
              created_by: null as unknown as string,
            }),
          ],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderVersions();

    const address = await screen.findByText(LONG);
    const cell = address.closest("td") as HTMLElement;
    expect(cell).toHaveAttribute("title", LONG);
    // …and the cell is the truncating one, so the `title` is on the element
    // that does the clipping rather than on an ancestor of it.
    expect(cell).toHaveClass("truncate");

    // No author: the placeholder carries no tooltip.
    const unattributed = within(screen.getByTestId("version-row-v-1")).getByText("—");
    expect(unattributed.closest("td")).not.toHaveAttribute("title");
  });

  // A viewer-level operator may not be permitted `listUsers` (it's a
  // platform-level call — see the component's comment). That failure must
  // degrade this ONE column to a truncated id, not take down a page that
  // otherwise has everything it needs to render.
  it("falls back to a truncated id, not blank or a page error, when the users lookup fails", async () => {
    const PUBLISHER_ID = "5513767f-8cdd-4e48-9cf3-c493a74b683e";
    vi.mocked(accessApi.listUsers).mockRejectedValue(new Error("forbidden"));
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: {
          status: 200,
          data: [makeVersion({ id: "v-1", version: 1, is_live: true, note: "initial", created_by: PUBLISHER_ID })],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderVersions();

    // The rest of the table rendered fine — this isn't the page-level error.
    await screen.findByText("initial");
    expect(screen.getByText(PUBLISHER_ID.slice(0, 8))).toBeInTheDocument();
    expect(screen.queryByText(PUBLISHER_ID)).not.toBeInTheDocument();
  });

  it("publishes with an optional note and refetches the endpoint afterward", async () => {
    renderVersions(makeEndpoint({ live_version_id: "v-2", state: "live" }), versionsRefetchSpy);
    await userEvent.click(await screen.findByRole("button", { name: /publish/i }));
    await screen.findByText(/does not grant data access/i);

    await userEvent.type(screen.getByLabelText(/note/i), "shipping v3");
    await userEvent.click(screen.getByRole("button", { name: "Publish" }));

    await waitFor(() =>
      expect(publishMutateAsync).toHaveBeenCalledWith({
        workspace: "eng",
        id: "ep-1",
        data: { note: "shipping v3" },
      }),
    );
    // `refetch` exists for exactly this case — publish changes `state` and
    // `live_version_id` server-side, and the header/other sections read
    // those off `endpoint`, not off this section's own versions query.
    await waitFor(() => expect(versionsRefetchSpy).toHaveBeenCalled());
  });

  // `PublishEndpointInput` documents `{}` as a valid publish body — an empty
  // note must be OMITTED, not sent as `{ note: "" }`. Every other publish
  // test types a note first, so a mutant that always attaches `note: ""`
  // would survive them undetected.
  it("publishes with no note as an empty body, not as an empty-string note", async () => {
    renderVersions(makeEndpoint({ live_version_id: "v-2", state: "live" }), versionsRefetchSpy);
    await userEvent.click(await screen.findByRole("button", { name: /publish/i }));
    await screen.findByText(/does not grant data access/i);

    await userEvent.click(screen.getByRole("button", { name: "Publish" }));

    await waitFor(() =>
      expect(publishMutateAsync).toHaveBeenCalledWith({
        workspace: "eng",
        id: "ep-1",
        data: {},
      }),
    );
  });

  /**
   * A refused publish must leave its reason on screen, and the dialog open.
   *
   * The C1 refusal ("parameter `n` has a `default` that violates its own
   * `minimum` of `10`") is the only thing that tells the author which FIELD to
   * change, and it used to be a `toast.error(String(e))`. Measured in Chrome on
   * this branch: that toast lived 4.3 s and could not be paused, because a
   * refused publish leaves this dialog OPEN and its overlay takes the pointer
   * — `elementFromPoint` at the toast's own centre resolved to
   * `[data-slot="dialog-overlay"]`, so sonner's pause-on-hover never fired.
   * The author was left looking at the dialog with no trace of the reason.
   * `Config`'s Validate block already states the rule this now follows.
   *
   * jsdom cannot judge that 4.3 s or that hit-test — this pins the part it CAN
   * judge, which is the part that was actually changed: the sentence is
   * rendered into the dialog, it is not a toast, and it survives an arbitrary
   * wait rather than being torn down on a timer.
   */
  it("keeps a refused publish's reason on screen, in the dialog, rather than in a toast", async () => {
    publishMutateAsync.mockRejectedValue(
      new Error("invalid input: parameter `n` has a `default` that violates its own `minimum` of `10`"),
    );
    renderVersions(makeEndpoint({ live_version_id: "v-2", state: "live" }), versionsRefetchSpy);
    await userEvent.click(await screen.findByRole("button", { name: /publish saved draft/i }));
    const dialog = await screen.findByRole("dialog");

    await userEvent.click(within(dialog).getByRole("button", { name: "Publish" }));

    const alert = await within(dialog).findByRole("alert");
    // Names the parameter, the constraint and the bound — the three things the
    // author needs and the serving path's 400 deliberately withholds.
    expect(alert).toHaveTextContent(/`n`/);
    expect(alert).toHaveTextContent(/minimum/);
    expect(alert).toHaveTextContent(/10/);
    // `String(e)` on an `ApiError` prefixed the sentence with a bare `Error: `.
    expect(alert).not.toHaveTextContent(/^Error:/);

    // Still there, and still inside the dialog, after the toast's own lifetime
    // would have expired. A `toast.error` here would have been torn down.
    await new Promise((r) => setTimeout(r, 50));
    expect(within(screen.getByRole("dialog")).getByRole("alert")).toBeInTheDocument();
    expect(versionsRefetchSpy).not.toHaveBeenCalled();
  });

  // The other half: a refusal the author has FIXED must not leave its sentence
  // behind. Without this, a second Publish that succeeds would close the dialog
  // over a stale alert that reappears the next time it is opened — the same
  // defect `handleOpenChange` clears the note for.
  it("clears a previous refusal when the dialog is reopened", async () => {
    publishMutateAsync.mockRejectedValue(new Error("invalid input: something the author must fix"));
    renderVersions(makeEndpoint({ live_version_id: "v-2", state: "live" }), versionsRefetchSpy);
    await userEvent.click(await screen.findByRole("button", { name: /publish saved draft/i }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Publish" }));
    await within(screen.getByRole("dialog")).findByRole("alert");

    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

    await userEvent.click(screen.getByRole("button", { name: /publish saved draft/i }));
    const reopened = await screen.findByRole("dialog");
    expect(within(reopened).queryByRole("alert")).not.toBeInTheDocument();
  });

  it("rolls back only after typing the confirming word, naming the version by id, and refetches the endpoint afterward", async () => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: {
          status: 200,
          data: [
            makeVersion({ id: "v-1", version: 1, is_live: false }),
            makeVersion({ id: "v-2", version: 2, is_live: true }),
          ],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderVersions(makeEndpoint({ live_version_id: "v-2", state: "live" }), versionsRefetchSpy);

    await userEvent.click(await screen.findByRole("button", { name: /roll back/i }));
    // The shared dialog's confirm button must say what this action actually
    // does — rollback is reversible (publish again), unlike the component's
    // default "Delete" label, which every OTHER call site of
    // `ConfirmDeleteDialog` genuinely wants.
    const confirmButton = screen.getByRole("button", { name: "Roll back" });
    expect(confirmButton).toBeDisabled();
    expect(rollbackMutateAsync).not.toHaveBeenCalled();

    await userEvent.type(screen.getByPlaceholderText("rollback"), "rollback");
    await userEvent.click(confirmButton);

    await waitFor(() =>
      expect(rollbackMutateAsync).toHaveBeenCalledWith({
        workspace: "eng",
        id: "ep-1",
        data: { version_id: "v-1" },
      }),
    );
    await waitFor(() => expect(versionsRefetchSpy).toHaveBeenCalled());
  });

  /**
   * Publish and Roll back on a TRASHED endpoint. Both are 409s server-side
   * (`lifecycle.rs`'s trash guards), and publish on a trashed `disabled`
   * endpoint was worse than a refusal before those guards: it succeeded and
   * froze a new version into the trash.
   *
   * Both states in one test, because "Publish is disabled" is also true of a
   * page that disables Publish unconditionally — and the version list is
   * rendered non-live so Roll back exists at all.
   */
  it("disables Publish and Roll back for a trashed endpoint, and says to restore it first", async () => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: {
          status: 200,
          data: [
            makeVersion({ id: "v-1", version: 1, is_live: false }),
            makeVersion({ id: "v-2", version: 2, is_live: true }),
          ],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );

    // Not trashed first: both controls are live, so their state below is a
    // consequence of `deleted_at` and not of this section's own layout.
    renderVersions(makeEndpoint({ live_version_id: "v-2", state: "live" }));
    expect(await screen.findByRole("button", { name: /publish saved draft/i })).toBeEnabled();
    expect(screen.getByRole("button", { name: /^roll back$/i })).toBeEnabled();
    expect(screen.queryByText("Restore it first")).not.toBeInTheDocument();

    renderVersions(
      makeEndpoint({ live_version_id: "v-2", state: "disabled", deleted_at: "2026-08-18T09:00:00Z" }),
    );
    const publishes = await screen.findAllByRole("button", { name: /publish saved draft/i });
    // `renderVersions` mounts a SECOND tree into the same document rather than
    // replacing the first, so the trashed render is the last of each pair.
    expect(publishes.at(-1)).toBeDisabled();
    expect(screen.getAllByRole("button", { name: /^roll back$/i }).at(-1)).toBeDisabled();
    // Visible, not a `title`: the rule is only taught if it is on screen.
    expect(screen.getByText("Restore it first")).toBeInTheDocument();
  });

  // Same class of bug ResponseSection was fixed for: react-query clears
  // `isLoading` on any settle, error included, so a fetch failure must not
  // fall into the same branch as "nothing published yet".
  it("reports a versions-fetch failure as an error, not as never having been published", async () => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: undefined,
        isLoading: false,
        isError: true,
        error: new Error("network error"),
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderVersions();
    expect(await screen.findByText(/could not load/i)).toBeInTheDocument();
    expect(screen.queryByText(/never been published/i)).not.toBeInTheDocument();
  });

  it("says the endpoint has never been published when the version list is genuinely empty", async () => {
    renderVersions();
    expect(await screen.findByText(/never been published/i)).toBeInTheDocument();
  });

  /**
   * Three snapshots with the LIVE one in the MIDDLE — the state a rollback
   * leaves behind, and the only arrangement in which "badged the live version"
   * and "badged the first row" are different claims.
   *
   * The pre-existing coverage is a count (`getAllByText("Live")` has length 1)
   * on a two-row fixture whose live version also sorted first, so an
   * implementation that badged `sorted[0]` passed it. Same trap `#529` hit with
   * a removal test that clicked row 1: a fixture where the right row IS row 0
   * proves nothing about which row was read.
   */
  function threeVersionsWithV2Live() {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: {
          status: 200,
          data: [
            makeVersion({ id: "v-1", version: 1, is_live: false, note: "first cut", created_by: "user-1" }),
            makeVersion({ id: "v-2", version: 2, is_live: true, note: "the served one", created_by: "user-2" }),
            makeVersion({ id: "v-3", version: 3, is_live: false, note: "rolled back off", created_by: "user-3" }),
          ],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
  }

  it("puts the Live badge on the version live_version_id points at, even when a newer one exists", async () => {
    threeVersionsWithV2Live();
    renderVersions(makeEndpoint({ live_version_id: "v-2", state: "live" }));

    const live = await screen.findByTestId("version-row-v-2");
    expect(within(live).getByText("Live")).toBeInTheDocument();
    expect(within(screen.getByTestId("version-row-v-3")).queryByText("Live")).not.toBeInTheDocument();
    expect(within(screen.getByTestId("version-row-v-1")).queryByText("Live")).not.toBeInTheDocument();

    // The premise of the three assertions above: newest-first is still the
    // display order, so the badged row is the SECOND on screen. Without this
    // the fixture could drift back to one where live and first coincide and
    // the test would keep passing while proving nothing.
    expect(screen.getAllByTestId(/^version-row-/).map((r) => r.dataset.testid)).toEqual([
      "version-row-v-3",
      "version-row-v-2",
      "version-row-v-1",
    ]);
  });

  // The re-presentation itself (`docs/en/ui-conventions.md` §4): the history is
  // list-shaped, so it is a plain `Table` — not the `Card` that used to frame
  // it, and not `ConfigTable`, which would have to fabricate a Value column for
  // a read-only listing. Column positions, not merely presence: a header row
  // that says "Note" over the Who column is a table that reads as wrong.
  it("lays the history out as one table row per snapshot, each fact under its own column", async () => {
    threeVersionsWithV2Live();
    renderVersions(makeEndpoint({ live_version_id: "v-2", state: "live" }));

    expect((await screen.findAllByRole("columnheader")).map((h) => h.textContent)).toEqual([
      "Version",
      "Note",
      "Who",
      "When",
      // Named for a screen reader and hidden on screen — an "Actions" heading
      // over one small button is louder than what it labels.
      "Actions",
    ]);

    const cells = within(screen.getByTestId("version-row-v-2")).getAllByRole("cell");
    expect(cells).toHaveLength(5);
    expect(cells[0]).toHaveTextContent("v2");
    expect(cells[1]).toHaveTextContent("the served one");
    expect(cells[2]).toHaveTextContent("user-2");
  });

  /**
   * Roll back has to name the row it was clicked on. With the live version
   * FIRST in display order and two rollbackable rows behind it, "the version
   * this button belongs to" and "the first rollbackable version" are different
   * answers — and this clicks the second of the two.
   *
   * Asserted as a whole-call deep equal rather than `objectContaining`, so a
   * mutant that also attached, dropped or renamed a field fails here rather
   * than passing a partial match.
   */
  it("rolls back the version whose own row's button was clicked, not the first rollbackable one", async () => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: {
          status: 200,
          data: [
            makeVersion({ id: "v-1", version: 1, is_live: false, note: "first cut" }),
            makeVersion({ id: "v-2", version: 2, is_live: false, note: "middle" }),
            makeVersion({ id: "v-3", version: 3, is_live: true, note: "the served one" }),
          ],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderVersions(makeEndpoint({ live_version_id: "v-3", state: "live" }), versionsRefetchSpy);

    const target = await screen.findByTestId("version-row-v-1");
    // The live row offers nothing to roll back to — rolling back to what is
    // already served is a no-op the row should not advertise.
    expect(
      within(screen.getByTestId("version-row-v-3")).queryByRole("button", { name: /roll back/i }),
    ).not.toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: /^roll back$/i })).toHaveLength(2);

    await userEvent.click(within(target).getByRole("button", { name: /^roll back$/i }));
    // The confirmation names the version too — a dialog that said v2 while the
    // mutation carried v-1 would be a worse defect than either alone.
    expect(await screen.findByText("Roll back to v1?")).toBeInTheDocument();

    await userEvent.type(screen.getByPlaceholderText("rollback"), "rollback");
    await userEvent.click(screen.getByRole("button", { name: "Roll back" }));

    await waitFor(() => expect(rollbackMutateAsync).toHaveBeenCalledTimes(1));
    expect(rollbackMutateAsync).toHaveBeenCalledWith({
      workspace: "eng",
      id: "ep-1",
      data: { version_id: "v-1" },
    });
  });

  // The loading state moved INSIDE the table when the card went, so the columns
  // are named before there is a row to put under them. It is also the third of
  // the three states §2 requires and the only one this suite never had.
  it("renders the loading state inside the history table, with the columns already named", async () => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      { data: undefined, isLoading: true, isError: false } as unknown as
        ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderVersions();

    expect(await screen.findByText("Loading…")).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "Version" })).toBeInTheDocument();
    // Not the empty state, and not an error: react-query clears `isLoading` on
    // any settle, so these three must stay three branches.
    expect(screen.queryByText(/never been published/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/could not load/i)).not.toBeInTheDocument();
  });

  // The section this sentence names has to be one the shell puts in its nav.
  // It used to say "Definition", which was renamed to Query before this
  // section shipped — so the copy pointed an author at a tab that is not on
  // the screen while they are reading it.
  it("names a section that exists", () => {
    renderVersions();

    const blurb = screen.getByText(/frozen snapshot/i);
    expect(blurb.textContent).not.toContain("Definition");
    expect(blurb).toHaveTextContent("Query");
    // …and "Query" is not just a nicer word: the shell renders it as a nav
    // link, so the reader can act on the sentence.
    expect(screen.getByRole("link", { name: "Query" })).toBeInTheDocument();
  });

  /**
   * The heading treatment, which is what made this tab read differently from
   * its five siblings in the nav.
   *
   * Every other section opens with a bare `<div>` holding the `h2` and one
   * capped paragraph; `Versions` alone wrapped that in a
   * `flex … justify-between` row with a filled primary button pinned to the
   * right of it — directly under the shell's own `Save`, so the top-right
   * corner of the page carried two primary-looking buttons that do unrelated
   * things. `Config.tsx` already ships the shape this section wanted: its
   * `Validate` action sits on the heading line of the block it acts on, not
   * on the section title.
   *
   * Publish belongs on `History`'s line for a reason beyond symmetry — what
   * it does is add a row to that table, so it is the same relationship
   * `Validate` has to the block under it.
   *
   * Both halves are asserted, and the second is what makes the first mean
   * something: "the title's block holds no button" passes just as well on a
   * section that lost Publish entirely.
   *
   * # The first version of the first half was GREEN ON THE DEFECT
   *
   * It read `title.parentElement.querySelector("button")`, which is trap #1
   * from this branch's own spec header — a counted DOM hop — and it was not a
   * near miss. At `9fd2e493` the `h2` sat in a BARE INNER `<div>` with the
   * paragraph, and Publish was that div's SIBLING inside the
   * `flex … justify-between` row; `title.parentElement` was therefore the
   * inner div, which never held a button, before or after. The red run proves
   * it: this test failed on `div.text-right ≠ div.space-y-2` — the *second*
   * assertion — and never on the first.
   *
   * The fix is not another hop. `blocksOf` walks BOTH headings up until they
   * are siblings and returns the two direct children of whatever they share,
   * so "the block that holds the title" is resolved by identity from the tree
   * that exists rather than counted from the tree that was expected. At the
   * base structure that block is the flex row and it DOES contain Publish, so
   * the assertion fails there — which is the whole point of it.
   */
  it("gives Versions the same heading block as its five siblings, with Publish on the History line", async () => {
    renderVersions();

    const title = await screen.findByRole("heading", { name: "Versions", level: 2 });
    const history = screen.getByRole("heading", { name: "History", level: 3 });
    const publish = screen.getByRole("button", { name: /publish saved draft/i });

    const [titleBlock, historyBlock] = blocksOf(title, history);
    // The section title's own block carries the title and its paragraph and
    // nothing to press. Asserted against the button by IDENTITY, not against
    // "a button" — a section that grew an unrelated control in its intro is a
    // different failure and should read as one.
    expect(titleBlock.contains(publish)).toBe(false);
    expect(titleBlock.contains(title)).toBe(true);
    expect(historyBlock.contains(publish)).toBe(true);

    // …and inside that block, on the heading's own line.
    expect(publish.parentElement).toBe(history.parentElement);
    // The heading LINE, not the block: the table is a sibling of that line, so
    // a parent holding it means the button landed in the block instead.
    expect((history.parentElement as HTMLElement).querySelector("table")).toBeNull();
  });

  /**
   * §5.1's last open mitigation. Removing this section's `max-w-5xl` handed
   * all 272 freed pixels to `Note` — the only unsized column of a
   * `table-fixed` table — and the instrument for that is the one `Access`
   * used: size the row table's own columns so the width lands where content
   * wants it.
   *
   * Measured in Chromium at 1600×1000 with five versions: `Who` laid out a
   * realistic 33-character address at 231px against 224px of column and cut
   * it, on two of five rows, while `Note` was handed 688px. `w-80` (320px)
   * fits the realistic band; a note is unbounded prose that no finite width
   * fixes, and it keeps the `title` fallback it already had.
   *
   * Gated at `xl` for §5.3's reason — `table-fixed` scrolls rather than
   * shrinks, so a column that is wide at every width costs the narrow regime
   * the widening was never meant to touch. Both halves asserted: a flat
   * `w-80` is a different and worse thing than `w-56 xl:w-80`.
   */
  it("widens Who where there is room, so the freed width lands on the addresses it was cutting", async () => {
    renderVersions();

    const who = await screen.findByRole("columnheader", { name: "Who" });
    expect(who).toHaveClass("w-56");
    expect(who).toHaveClass("xl:w-80");
    // `Note` stays the unsized remainder — this is a REALLOCATION, not a
    // second sized column, and sizing all five would let fixed layout hand the
    // surplus back proportionally (measured: 128/512/288/128/128 in a 1296px
    // box lays out as 140/553/312/140/140).
    // `max-w-` as well as `w-`: the first version of this guard anchored on a
    // word boundary, so `max-w-96` — the obvious way somebody would try to
    // bound this column — slipped through the very assertion written to catch
    // a sizing.
    const noteClass = screen.getByRole("columnheader", { name: "Note" }).className;
    expect(noteClass).not.toMatch(/(?:^|\s)(?:[a-z]+:)*w-\d/);
    expect(noteClass).not.toMatch(/(?:^|\s)(?:[a-z]+:)*max-w-/);
  });

});

/**
 * This route used to sit inside `DataApiLayout`'s `<Outlet/>`, which supplied
 * the page's one document-outline `h1` ("Data API", text-2xl). Once the shell
 * was removed (Data API became a sidebar group instead of a shell page), the
 * three list pages (Endpoints/Clients/Activity) got their own `h1` back —
 * but this route was missed: its visible title is a breadcrumb-style `h2`
 * (the endpoint name), and nothing above it carries a level-1 heading, so a
 * screen-reader user landing here got no page-level heading at all and the
 * document's heading outline started at level 2. Fixed first with an
 * `sr-only` h1; since KI-214 the endpoint name on screen IS that h1, the
 * compact tool title (FE-PAGE-7), so the page has one title, visible.
 */
describe("EndpointDetail — document heading", () => {
  it("exposes exactly one level-1 heading, naming the endpoint", async () => {
    renderConfig(makeEndpoint({ name: "Weekly rollup" }));
    const headings = await screen.findAllByRole("heading", { level: 1 });
    expect(headings).toHaveLength(1);
    expect(headings[0]).toHaveTextContent("Weekly rollup");
    // Visible, at the compact tool scale — not an `sr-only` twin of an `h2`.
    expect(headings[0]).not.toHaveClass("sr-only");
    expect(headings[0]).toHaveAttribute("data-page-title", "compact");
  });
});

/**
 * `max-w-3xl` on the `Outlet` wrapper is why the Details column wrapped to
 * three lines while the right third of the page sat empty.
 *
 * **The first assertion below is weak and is meant to be**: it pins a class
 * name, not a layout. It exists only so the cap cannot come back silently.
 * Whether the result is a good use of the width is a question only the browser
 * check can answer. The second assertion is the one with teeth — it says the
 * space the cap used to waste actually reaches the *controls*, not only the
 * muted Details prose that `table-fixed` would otherwise hand all of it to.
 */
describe("EndpointDetail — width", () => {
  /**
   * Local, and it is a BUG FIX rather than tidying.
   *
   * Four of the tests below render `ResponseSection` or `Versions`, both of
   * which call `useDataApiListServiceAccounts` and
   * `useDataApiListEndpointVersions`. Neither mock was set here — they were
   * LEAKING from `describe("ResponseSection")`'s own `beforeEach` further up
   * this file. There is no `clearMocks`/`mockReset` in `vitest.config.ts`, so a
   * `mockReturnValue` set by an earlier describe survives into this one, and
   * these four tests were passing on state they never asked for.
   *
   * Measured on the clean, all-green tree: unfiltered, the four Data API detail
   * files give 185 passed / 0 failed; the same four with `-t "width"` give
   * **4 failed**, every one a `TypeError` inside `ResponseSection` for a hook
   * that returned `undefined`.
   *
   * Two things follow, and the second is the one that bites:
   *
   *  - **No `-t`-filtered count in this suite is a result.** Filtering skips
   *    the tests whose side effects the survivors depend on, so a filtered run
   *    reports failures that do not exist and can equally hide ones that do.
   *  - **It was an ORDER DEPENDENCY, not a filtering curiosity.** Anything that
   *    changes which tests run before these — `.only`, `--shard`, a bisect, or
   *    splitting this 1900-line file, which is a standing temptation — turned
   *    three real width guards red for a reason having nothing to do with
   *    width. That is a guard failing for the wrong reason, which is only one
   *    step better than passing for the wrong reason.
   *
   * Setting them here makes this block self-contained. The last test overrides
   * `useDataApiListEndpointVersions` for its own fixture, which still works:
   * `beforeEach` runs first and the test body's `mockReturnValue` wins.
   */
  beforeEach(() => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      { data: { status: 200, data: [] }, isLoading: false, isError: false } as unknown as
        ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    vi.mocked(api.useDataApiListServiceAccounts).mockReturnValue(
      {
        data: {
          status: 200,
          data: [{ id: "sa-1", name: "svc", description: "", email: "svc@x", created_at: "2026-01-01T00:00:00Z" }],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListServiceAccounts>,
    );
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    // `Versions` renders `PublishDialog`, which reads `isPending` off both
    // mutations on first paint — these were leaking from
    // `describe("VersionsSection")` in exactly the same way.
    vi.mocked(api.useDataApiPublishEndpoint).mockReturnValue(
      { mutateAsync: vi.fn(), isPending: false } as unknown as
        ReturnType<typeof api.useDataApiPublishEndpoint>,
    );
    vi.mocked(api.useDataApiRollbackEndpoint).mockReturnValue(
      { mutateAsync: vi.fn(), isPending: false } as unknown as
        ReturnType<typeof api.useDataApiRollbackEndpoint>,
    );
    vi.mocked(api.getDataApiListEndpointVersionsQueryKey).mockImplementation(
      (ws, id) => ["dataApiListEndpointVersions", ws, id] as unknown as
        ReturnType<typeof api.getDataApiListEndpointVersionsQueryKey>,
    );
  });

  /**
   * §5.1's intro rule for the three sections this file renders, pinned where
   * the class actually is — one test rather than three, because the claim is
   * about all of them and a per-section copy is what let `Try it` and `Access`
   * ship with no guard at all (§5.6).
   *
   * The class assertion is the RIGHT instrument here rather than a stand-in for
   * a measurement — see `expectSectionIntroCapsItsProse`. Before this test the
   * only guard was a Playwright spec that has never executed, plus a population
   * count `Response` could satisfy with its intro uncapped, because it renders
   * three unconditional `max-w-prose` paragraphs.
   */
  it("caps each section intro at a reading measure, not the section's (§5.1)", async () => {
    renderConfig();
    expectSectionIntroCapsItsProse(
      await screen.findByRole("heading", { name: "Config", level: 2 }),
    );
    cleanup();

    renderResponse();
    expectSectionIntroCapsItsProse(
      await screen.findByRole("heading", { name: "Response", level: 2 }),
    );
    cleanup();

    renderVersions();
    expectSectionIntroCapsItsProse(
      await screen.findByRole("heading", { name: "Versions", level: 2 }),
    );
  });

  it("wraps the Outlet in nothing that caps the width", async () => {
    // The subject is the ROUTE, so the section is a sentinel rather than a real
    // one. Two earlier shapes of this guard were wrong in opposite directions
    // and both are worth naming, because the sentinel is what makes each
    // impossible rather than merely unlikely:
    //
    //  - `container.querySelector(".max-w-3xl")` scanned the whole subtree, so
    //    it forbade one class name to an entire page. A section may legitimately
    //    cap its own children — `ResponseSection`'s editable `returns` grid
    //    (`max-w-xl`) does, and as of §5.1's reversal it is the only one.
    //
    //    This line used to cite `Try it`'s parameter grid instead. That was
    //    never true: `TryIt.tsx:389` is a bare `<Table className="table-fixed">`
    //    with no cap of its own — it was sitting inside `Try it`'s SECTION cap,
    //    which the reversal removed. The same false precedent was written in
    //    `ResponseSection.tsx`'s own comment on the `returns` grid, so it
    //    existed in two places and was corrected in both.
    //  - Walking up from a real section's content and stopping at "the child of
    //    the route container" reads the wrapper we are hunting AS the section
    //    root whenever it exists — measured: a restored `max-w-3xl` around the
    //    `Outlet` passes 31/31 under that shape. The guard would be dead in
    //    exactly the case it was written for.
    //
    // With a sentinel there is no section root to confuse for a wrapper: every
    // element strictly above it belongs to the route. So this asserts the route
    // interposes no cap, and says nothing about what any section does to itself
    // — which is what lets `Versions` cap its own root while `Config` does
    // not. Both of those are pinned by their own guards below; being silent
    // about sections is this guard's design, not an oversight, but it is also
    // why those two guards have to exist.
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue(
      { data: { status: 200, data: endpoint }, isLoading: false, isError: false, error: null, refetch: vi.fn() } as unknown as
        ReturnType<typeof api.useDataApiGetEndpoint>,
    );
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { container } = render(
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={[`/explore/eng/data-api/endpoints/${endpoint.id}/config`]}>
          <Routes>
            <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
              <Route path="config" element={<div data-testid="section-content" />} />
            </Route>
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>,
    );

    const sentinel = await screen.findByTestId("section-content");
    const routeRoot = container.querySelector(".p-4");
    expect(routeRoot).not.toBeNull();

    const capped: string[] = [];
    for (let el = sentinel.parentElement; el && el !== routeRoot!.parentElement; el = el.parentElement) {
      if (/(?:^|\s)(?:[a-z0-9]+:)*max-w-/.test(el.className)) capped.push(el.className);
    }
    expect(capped).toEqual([]);
  });

  it("leaves Versions on the page's width, like all six tabs (§5.1, as reversed)", async () => {
    // REVERSED, and the reversal is the point of the assertion. This used to
    // say Versions CAPS its own width — it was the tab where full width read
    // worst: five short columns (`v3`, a truncated note, an email, a relative
    // time, one button) spread across a wide monitor, with Roll back marooned
    // at the far right of the row it acts on.
    //
    // That description of THIS tab is still accurate; §5.1's reversal is that
    // it stopped being the deciding fact. Measured on the deployed build at
    // 1600×1000 with seeded data: content box 1296px on all six tabs, widest
    // block 1296 on Query/Config/Response and 1024 on Access/Try it/Versions —
    // the content edge moved 272px on a tab change, three times over, for a
    // reason no single tab could explain. The superseded argument is kept in
    // §5.1 rather than edited away.
    //
    // Class name only (§5.5). That the six tabs actually AGREE is a browser
    // claim and lives in `tests/e2e/data-api-detail-tab-widths.spec.ts`.
    const { container } = renderVersions();
    const heading = await screen.findByRole("heading", { name: "Versions", level: 2 });
    expectSectionLeavesWidthToThePage(container, heading);
  });

  it("leaves Response's own root uncapped, like all six tabs (§5.1, as reversed)", async () => {
    // REVERSED TWICE, and the second reversal subsumed the first.
    //
    // It first said "caps Response on its own root". §5.4b flipped it on
    // §5.1's what-a-section-HOLDS criterion — settings tables and code editors
    // use the page, prose and short lists cap themselves — because Response
    // gained a `Setting | Details | Value` table (§4.2) and a Monaco editor
    // for the response transform, moving it from the second group to the
    // first.
    //
    // That criterion is now retired: §5.1 was reversed to ONE width for all
    // six tabs, so there are no groups to sort into and this section is no
    // longer uncapped for a reason special to itself. The measurement below is
    // kept because it is still why THIS cap came off first.
    //
    // Measured in Chromium at 1600×1000, sidebar expanded, before this change:
    // the section stopped at 1024px inside a 1296px content box, leaving a
    // 288px gutter that no block could ever use, while its four blocks ended at
    // 840 / 796 / 1288 / 796 — the ragged edge the audit reported. Class name
    // only here; jsdom judges no geometry (§5.5), and the browser numbers are
    // in `tests/e2e/data-api-response-width.spec.ts` and the task report.
    const { container } = renderResponse();
    const heading = await screen.findByRole("heading", { name: "Response", level: 2 });
    expectSectionLeavesWidthToThePage(container, heading);
  });

  it("gives Response's Value column more than the shared default, like Config's", async () => {
    // §5.2, and the reason this line has to exist next to the one above:
    // removing a cap is not enough. `ConfigTable` is `table-fixed`, so every
    // pixel the section gains goes to Details — the only unsized column — and
    // the three Selects stay at the shared 208px however wide the page is.
    // Measured in Chromium at 1600×1000: 192px trigger before, 368px after,
    // with the column itself 208 → 384.
    //
    // Both halves asserted for the same reason Config's guard asserts both:
    // below `lg` the sized columns do not shrink, they scroll (§5.3), so a
    // flat `w-96` is a different and worse thing than `w-52 lg:w-96`.
    renderResponse();
    await screen.findByLabelText("Envelope");
    const valueHead = screen.getByRole("columnheader", { name: "Value" });
    expect(valueHead).toHaveClass("lg:w-96");
    expect(valueHead).toHaveClass("w-52");
  });

  it("puts the frozen column list on the page's measure, with Name on the settings grid", async () => {
    // The frozen `Name | Type` table carried `max-w-xl` because, left at the
    // shared `w-full` with AUTO layout, `bigint` sat far from the name it
    // describes. Measured in Chromium, that cap did not actually fix it: at
    // 576px the auto layout gave Name 444px, so `Type` started 444px into a
    // 576px table. `table-fixed` with a sized Name column is what puts the type
    // beside the name — 288px in, at any table width — which is also the
    // column the `Setting | Details | Value` table below uses, so the two read
    // as one grid down the section.
    //
    // Class names, not geometry (§5.5). The `w-72` is the load-bearing half:
    // `table-fixed` with no sized column splits the surplus evenly and puts the
    // type back in the middle of the page.
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      { data: { status: 200, data: [makeVersion()] }, isLoading: false, isError: false } as unknown as
        ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    renderResponse(makeEndpoint({ live_version_id: "v-1" }));

    const nameHead = await screen.findByRole("columnheader", { name: "Name" });
    const table = nameHead.closest("table") as HTMLElement;
    expect(table).toHaveClass("table-fixed");
    expect(table.className).not.toMatch(/(?:^|\s)(?:[a-z0-9]+:)*max-w-/);
    expect(nameHead).toHaveClass("w-72");
  });

  it("leaves Config's own root uncapped, which is where the width was meant to go", async () => {
    // The route guard above mounts a SENTINEL by design, so it says the ROUTE
    // interposes no cap and deliberately says nothing about what a section does
    // to itself. That leaves a hole exactly the shape of the defect this branch
    // exists to remove: re-applying the cap to Config's OWN root wastes the
    // same right third of the page, and measured, `<div className="max-w-3xl
    // space-y-6">` at Config.tsx's root passed the sentinel guard and every
    // other Config test — 230/230 green. `Versions` has a guard saying it OWNS
    // its cap; this is the inverse, on the tab the width pass was motivated by.
    const { container } = renderConfig();
    const heading = await screen.findByRole("heading", { name: "Config", level: 2 });
    expectSectionLeavesWidthToThePage(container, heading);
  });

  it("gives Config's Value column more than the shared default", async () => {
    renderConfig();
    await screen.findByLabelText("Max rows");
    const valueHead = screen.getByRole("columnheader", { name: "Value" });
    // Config owns the two widest controls on any detail tab (a 2-row
    // Description textarea and the Path input); at the shared `w-52` they show
    // roughly 50 and 25 characters. Removing the page cap does not help them —
    // under `table-fixed` the surplus goes only to the unsized Details column
    // — so this section opts into a wider one.
    expect(valueHead).toHaveClass("lg:w-96");
    // The `lg:` half is the whole point and jsdom cannot evaluate it, so this
    // second line is what stops a later "simplification" to a flat `w-96`:
    // below `lg` the sized columns do not shrink, they scroll, and a flat
    // `w-96` more than doubles that scroll on a narrow screen.
    expect(valueHead).toHaveClass("w-52");
  });
});

/**
 * The shell is a bounded flex column so ONE section (Query) can be a
 * full-height editing surface — an SQL editor above a preview, split by a drag
 * handle — without the other five losing anything.
 *
 * **What these tests are, stated plainly: assertions about the class chain, not
 * about layout.** jsdom implements no CSS layout — every box is 0×0, nothing
 * overflows, and nothing scrolls — so no test in this file can observe that a
 * long section actually scrolls, that the nav actually stays put, or that the
 * editor actually fills the height. What it CAN observe is the chain of
 * elements that produces those things, and that chain is where the regression
 * lives: dropping `min-h-0` (a flex item's default `min-height: auto` refuses
 * to shrink below its content) is the classic way this shape silently reverts
 * to a growing page, and it is invisible to every other test in this file
 * because every one of them still passes.
 *
 * So these walk UP from a rendered section — never `querySelector` on
 * hand-picked nodes — which is what makes them depend on the real structure: an
 * extra wrapper interposed between the section and the scroll box breaks the
 * walk instead of being silently walked past. The real check is a browser, and
 * it is written down in the plan's post-implementation pass.
 */
describe("EndpointDetail — one bounded scroll region", () => {
  /** The `<div className="p-4">` this route renders as its own root. */
  function routeRootOf(container: HTMLElement) {
    const root = container.querySelector<HTMLElement>(".p-4");
    expect(root).not.toBeNull();
    return root!;
  }

  it("makes the section area the flexible, scrollable remainder of a full-height column", async () => {
    const { container } = renderConfig();
    const heading = await screen.findByRole("heading", { name: "Config", level: 2 });

    // Walked from the section's own heading, so this is the element the
    // section is REALLY inside of, whatever wrappers exist today.
    const scrollBox = heading.closest("fieldset");
    expect(scrollBox).not.toBeNull();

    // `flex-1` takes the space the fixed-size header and nav leave; `min-h-0`
    // is what lets it hand space back, which is the half that regresses
    // silently; `overflow-auto` is what makes the overflow reachable rather
    // than clipped; `min-w-0` is the pre-existing fieldset guard (a UA
    // `min-inline-size: min-content` that Tailwind's preflight does not reset).
    expect(scrollBox).toHaveClass("flex-1", "min-h-0", "overflow-auto", "min-w-0");

    // Its parent is the route root, and the root is the full-height column —
    // `h-full` and not a viewport constant, because `AppShell`'s `main` is
    // already a sized flex child of an `h-screen` column.
    const root = routeRootOf(container);
    expect(scrollBox!.parentElement).toBe(root);
    expect(root).toHaveClass("flex", "h-full", "flex-col");

    // Exactly one scroll region on the vertical axis. A second `overflow-auto`
    // on the root gives the page two scrollbars, and the inner one stops at
    // whatever height the outer one has already grown to.
    expect(root.className).not.toMatch(/(?:^|\s)(?:[a-z0-9]+:)*overflow(?:-y)?-(?:auto|scroll)(?:\s|$)/);
  });

  it("keeps the header and the section nav outside the scrolling area", async () => {
    const { container } = renderConfig();
    const heading = await screen.findByRole("heading", { name: "Config", level: 2 });
    const scrollBox = heading.closest("fieldset")!;
    const root = routeRootOf(container);

    const nav = screen.getByRole("navigation", { name: "Endpoint sections" });
    const save = screen.getByRole("button", { name: "Save" });

    // Not merely present — present OUTSIDE the box that scrolls, which is what
    // makes them stay put while a long section scrolls under them. Both would
    // still be `getBy`-findable if they had been moved inside it.
    expect(scrollBox.contains(nav)).toBe(false);
    expect(scrollBox.contains(save)).toBe(false);

    // …and siblings of it in the same column, sized by their content. Flex
    // items shrink by default, so without `shrink-0` a tall section squeezes
    // the nav — and the nav is how the other five sections are reached at all.
    expect(nav.parentElement).toBe(root);
    expect(nav).toHaveClass("shrink-0");
    const headerRow = save.closest("div")!.parentElement!;
    expect(headerRow.parentElement).toBe(root);
    expect(headerRow).toHaveClass("shrink-0");
  });
});
