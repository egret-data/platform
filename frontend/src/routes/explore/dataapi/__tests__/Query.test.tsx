import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Link, MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// No msw in this repo — mock the API module directly. `@/api/access` is
// needed only by the Publish/shared-dirty-flag block below (`Versions`
// resolves a version's `created_by` via `listUsers`), and
// `@/lib/useUnsavedChangesWarning` only by the dirtiness block — both mocked
// here anyway since `vi.mock` must be module-scope, not per-describe.
vi.mock("@/api/generated/data-api/data-api");
vi.mock("@/api/databases");
vi.mock("@/api/dataApi");
vi.mock("@/api/access");

// Spy on the real hook rather than replace it with a no-op — see the
// equivalent mock in `EndpointDetail.test.tsx` for why (needs every call's
// argument, in order).
const warnSpy = vi.fn();
vi.mock("@/lib/useUnsavedChangesWarning", () => ({
  useUnsavedChangesWarning: (when: boolean) => warnSpy(when),
}));

import * as api from "@/api/generated/data-api/data-api";
import * as databasesApi from "@/api/databases";
import * as dataApi from "@/api/dataApi";
import * as accessApi from "@/api/access";
import { ParamLocation, ParamType, type DataApiEndpointDto } from "@/api/generated/model";
import EndpointDetail from "@/routes/explore/dataapi/EndpointDetail";
import Query from "@/routes/explore/dataapi/detail/Query";
import VersionsSection from "@/routes/explore/dataapi/detail/Versions";

// Copied verbatim from `EndpointDetail.test.tsx` (near the top of that file)
// rather than imported — that file does not export it, and duplicating a
// fixture builder is cheaper than making a 900-line test file into a module.
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
    live_version_id: null,
    state: "draft",
    created_by: "user-1",
    created_at: "2026-08-01T00:00:00Z",
    updated_at: "2026-08-10T00:00:00Z",
    ...overrides,
  } as unknown as DataApiEndpointDto;
}

function renderQuery(endpoint: DataApiEndpointDto = makeEndpoint()) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/explore/ws1/data-api/endpoints/${endpoint.id}`]}>
        <Routes>
          <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
            <Route index element={<Query />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("Query section — preview", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }],
    } as never);
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({
      mutateAsync: vi.fn(),
      isPending: false,
    } as never);
  });

  it("sends the on-screen SQL, not the loaded draft", async () => {
    const endpoint = makeEndpoint({ sql: "SELECT 1" });
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    const mutateAsync = vi.fn().mockResolvedValue({
      status: 200,
      data: { columns: [{ name: "n", type: "integer" }], rows: [[1]], row_count: 1, truncated: false },
    });
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync,
      isPending: false,
    } as never);

    renderQuery(endpoint);
    const sql = await screen.findByLabelText("SQL");
    await userEvent.clear(sql);
    await userEvent.type(sql, "SELECT 2");
    await userEvent.click(screen.getByRole("button", { name: /preview/i }));

    await waitFor(() => expect(mutateAsync).toHaveBeenCalled());
    expect(mutateAsync.mock.calls[0][0].data.sql).toBe("SELECT 2");
  });

  it("says the result was capped when the engine truncated it", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync: vi.fn().mockResolvedValue({
        status: 200,
        data: { columns: [{ name: "n", type: "integer" }], rows: [[1]], row_count: 1, truncated: true },
      }),
      isPending: false,
    } as never);

    renderQuery(endpoint);
    await userEvent.click(await screen.findByRole("button", { name: /preview/i }));
    // A capped preview read as a small result set is the failure this exists to
    // stop, so the copy must name the cap.
    expect(await screen.findByText(/first 100 rows/i)).toBeInTheDocument();
  });

  it("shows the engine's own error text inline", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({
      mutateAsync: vi.fn().mockRejectedValue(new Error("line 1:8: mismatched input 'FRUM'")),
      isPending: false,
    } as never);

    renderQuery(endpoint);
    await userEvent.click(await screen.findByRole("button", { name: /preview/i }));
    // Inline, not a toast: a toast vanishes before an author mid-edit reads it,
    // and this is exactly the moment they need the message on screen.
    expect(await screen.findByText(/mismatched input 'FRUM'/)).toBeInTheDocument();
  });

  it("says preview shows the engine's column names, not the published contract's", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    renderQuery(endpoint);
    // Spec §2.4.1: say which is which or the two get confused.
    expect(await screen.findByText(/column names come from the engine/i)).toBeInTheDocument();
  });

  it("says a green preview does not prove a consumer can call it", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    renderQuery(endpoint);
    // Spec §2.1: "The consequence must reach the UI, not just this document."
    expect(await screen.findByText(/runs as you/i)).toBeInTheDocument();
  });
});

describe("Query section — layout", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }],
    } as never);
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
  });

  it("holds the database picker, the SQL editor and the parameter declarations", async () => {
    const endpoint = makeEndpoint({
      params: [
        {
          name: "region",
          type: ParamType.string,
          location: ParamLocation.query,
          required: true,
          description: "",
        },
      ],
    });
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);

    renderQuery(endpoint);
    expect(await screen.findByLabelText("Database")).toBeInTheDocument();
    expect(screen.getByLabelText("SQL")).toBeInTheDocument();
    // The DECLARATION, from ParamEditor…
    expect(screen.getByDisplayValue("region")).toBeInTheDocument();
    // …and a separate test-value input for the same parameter, which is the
    // distinction merging the two would destroy.
    expect(screen.getByLabelText(/^region/)).toBeInTheDocument();
  });

  it("does not hold name, path or the config tables — those are Config's", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderQuery(endpoint);
    await screen.findByLabelText("SQL");
    expect(screen.queryByLabelText("Name")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Path")).not.toBeInTheDocument();
    expect(screen.queryByText("Rate limit")).not.toBeInTheDocument();
  });
});

// Relocated from `EndpointDetail.test.tsx` (Task 4): these tests exercise the
// shared dirty-tracking / save machinery (`EndpointDetailInner`) through the
// SQL field, which now lives here rather than in the old `Definition`.
describe("Query section — dirtiness and save", () => {
  const updateMutateAsync = vi.fn();

  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }],
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    updateMutateAsync.mockReset();
    updateMutateAsync.mockImplementation(
      ({ data }: { workspace: string; id: string; data: Partial<DataApiEndpointDto> }) =>
        Promise.resolve({ status: 200, data: { ...makeEndpoint(), ...data }, headers: new Headers() }),
    );
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({
      mutateAsync: updateMutateAsync,
      isPending: false,
    } as never);
  });

  it("arms the unsaved-changes warning only after a real change, and disarms it when reverted", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderQuery(endpoint);
    const sql = await screen.findByDisplayValue("SELECT 1");
    // Radix hydration and controlled-input echo must not count as a change —
    // not even a transient one: a flag that latches `true` during hydration
    // and only "catches up" to `false` a render later would still show the
    // phantom dialog to anyone who closes the tab in that window, so the
    // stronger assertion is that `true` is never seen at all, not just that
    // the LAST call happens to be `false`.
    expect(warnSpy).not.toHaveBeenCalledWith(true);
    expect(warnSpy).toHaveBeenLastCalledWith(false);

    await userEvent.type(sql, " -- x");
    expect(warnSpy).toHaveBeenLastCalledWith(true);

    // The property that actually separates "compare to baseline" from "a
    // flag that only latches": reverting the edit back to the loaded value
    // must disarm the warning. A `touched` flag can never do this without
    // extra reset logic bolted on; the comparison gets it for free.
    await userEvent.type(sql, "{Backspace}".repeat(5));
    expect(sql).toHaveValue("SELECT 1");
    expect(warnSpy).toHaveBeenLastCalledWith(false);
  });

  it("re-baselines after a successful save", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderQuery(endpoint);
    await userEvent.type(await screen.findByLabelText("SQL"), " -- x");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(warnSpy).toHaveBeenLastCalledWith(false));
  });

  // Pins the hydrate-once guard: ChartExplore and NotebookSettings have both
  // shipped a regression where re-seeding the form on every data reference
  // (rather than once) clobbers a mid-edit form the instant react-query hands
  // back a new object identity for unchanged data (e.g. a background refetch).
  it("does not clobber a mid-edit field when the query result gets a new object identity", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderQuery(endpoint);
    const sql = await screen.findByLabelText("SQL");
    await userEvent.type(sql, " -- x");
    expect(sql).toHaveValue("SELECT 1 -- x");

    // Simulate a react-query refetch that resolves to an equal-but-new object.
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: { ...endpoint } },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    // Force a re-render by re-rendering the same tree is not enough on its own
    // (React Testing Library doesn't re-run hooks without a state change in the
    // tree), so instead assert the value survived the mock swap plus a fresh
    // paint triggered by a further keystroke.
    await userEvent.type(sql, "y");
    expect(sql).toHaveValue("SELECT 1 -- xy");
  });

  // Reviewer finding (Critical #1): a bare "ran once" ref never re-arms when
  // only the `:id` route param changes, because React Router reconciles the
  // SAME `EndpointDetail` instance across `/endpoints/A` → `/endpoints/B`.
  // Without keying the hydration guard (and the nested Outlet) by id, this
  // would show B's name in the header over A's SQL/path/params/database —
  // and a subsequent Save would silently overwrite B's record with A's data.
  it("shows the newly-navigated endpoint's own fields, not the previous one's, after an in-app navigation", async () => {
    const epA = makeEndpoint({ id: "ep-a", name: "Endpoint A", sql: "SELECT A" });
    const epB = makeEndpoint({ id: "ep-b", name: "Endpoint B", sql: "SELECT B" });
    vi.mocked(api.useDataApiGetEndpoint).mockImplementation(((_ws: string, id: string) => ({
      data: { status: 200, data: id === "ep-a" ? epA : epB },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    })) as unknown as typeof api.useDataApiGetEndpoint);

    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const user = userEvent.setup();
    render(
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={["/explore/eng/data-api/endpoints/ep-a"]}>
          <Routes>
            {/* The `Link` inside the SAME route element is what reproduces the
                bug: the route pattern (and so the rendered `EndpointDetail`
                element) never changes across this navigation, only `:id` does —
                matching what a future in-app link between endpoints (Tasks
                8/9) will look like. */}
            <Route
              path="/explore/:workspace/data-api/endpoints/:id"
              element={
                <>
                  <Link to="/explore/eng/data-api/endpoints/ep-b">Go to B</Link>
                  <EndpointDetail />
                </>
              }
            >
              <Route index element={<Query />} />
            </Route>
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>,
    );

    await screen.findByDisplayValue("SELECT A");
    await user.click(screen.getByText("Go to B"));

    await waitFor(() => expect(screen.getByLabelText("SQL")).toHaveValue("SELECT B"));
    // The visible breadcrumb heading (`h2`), not `getByText` — the page also
    // carries an `sr-only` `h1` with the same name, so a bare text match is
    // now ambiguous.
    expect(screen.getByRole("heading", { level: 2, name: "Endpoint B" })).toBeInTheDocument();
    expect(screen.queryByDisplayValue("SELECT A")).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 2, name: "Endpoint A" })).not.toBeInTheDocument();
  });

  it("saves only the changed fields, not the whole form (a partial PATCH)", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderQuery(endpoint);
    await userEvent.type(await screen.findByLabelText("SQL"), " -- x");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    const call = updateMutateAsync.mock.calls[0][0];
    expect(call).toMatchObject({ workspace: "ws1", id: "ep-1" });
    // Reviewer finding (Important #6): `UpdateEndpointInput`'s own doc says an
    // absent field means "leave it alone" — a whole-document PATCH built from
    // a form hydrated once at mount would silently revert any change made
    // elsewhere since. Only `sql` changed, so only `sql` should be sent.
    expect(call.data).toEqual({ sql: "SELECT 1 -- x" });
  });
});

// Relocated from `EndpointDetail.test.tsx` (Task 4): parameters (declaration
// + the allowed-values field) are `ParamEditor`'s, which lives in `Query`.
describe("Query section — ParamEditor", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }],
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
  });

  it("does not offer a location control for parameters", async () => {
    const endpoint = makeEndpoint({
      params: [{ name: "region", type: ParamType.string, location: ParamLocation.query, required: false }],
    });
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderQuery(endpoint);
    // Confirms ParamEditor actually rendered a row — otherwise this assertion
    // would pass trivially against an empty editor with nothing to check.
    await screen.findByRole("combobox", { name: "Parameter 1 type" });
    expect(screen.queryByText(/location/i)).not.toBeInTheDocument();
  });

  // Reviewer finding (Critical #2): the field used to be fully controlled —
  // `value={allowed.join(", ")}` with an `onChange` that immediately
  // re-split/trimmed/filtered — which silently ate a trailing comma on every
  // keystroke (existing "a", type "," → "a," → normalized straight back to
  // "a"), so a second value could never be started by typing.
  it("accepts a comma typed as a separate keystroke when building an enum's allowed values", async () => {
    const endpoint = makeEndpoint({
      params: [{ name: "region", type: ParamType.enum, location: ParamLocation.query, required: true, allowed: [] }],
    });
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    const updateMutateAsync = vi.fn().mockResolvedValue({
      status: 200,
      data: makeEndpoint(),
      headers: new Headers(),
    });
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({ mutateAsync: updateMutateAsync, isPending: false } as never);

    renderQuery(endpoint);
    const allowed = await screen.findByLabelText("Allowed values");

    await userEvent.type(allowed, "a,b");
    expect(allowed).toHaveValue("a,b");

    // Clicking elsewhere blurs the field, committing the projected array —
    // and re-formats the display for a stray typo in spacing.
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(allowed).toHaveValue("a, b");

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    const call = updateMutateAsync.mock.calls[0][0];
    expect(call.data.params[0].allowed).toEqual(["a", "b"]);
  });

  /**
   * Final-review finding (#8, second half): `AllowedValuesField` holds its
   * text locally so a trailing comma is not eaten mid-typing — but the local
   * copy must not survive a Save. `EndpointDetail` re-seeds the whole form
   * from the SERVER RESPONSE, so a server-normalised `allowed` list has to
   * reach the field, and the field's stale text must never be pushed back
   * over it.
   */
  describe("the allowed-values field resyncs from the server", () => {
    const ENUM_PARAM = {
      name: "region",
      type: ParamType.enum,
      location: ParamLocation.query,
      required: false,
      allowed: ["eu", "us"],
    };

    it("shows the server's normalised list after a Save, not the pre-save text", async () => {
      const endpoint = makeEndpoint({ params: [ENUM_PARAM] } as Partial<DataApiEndpointDto>);
      vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
        data: { status: 200, data: endpoint },
        isLoading: false,
        isError: false,
        refetch: vi.fn(),
      } as never);
      // The server normalises: it lower-cases and de-duplicates what was sent.
      const updateMutateAsync = vi.fn().mockResolvedValue({
        status: 200,
        data: makeEndpoint({ params: [{ ...ENUM_PARAM, allowed: ["eu", "us", "apac"] }] } as Partial<DataApiEndpointDto>),
        headers: new Headers(),
      });
      vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({ mutateAsync: updateMutateAsync, isPending: false } as never);
      renderQuery(endpoint);

      const allowed = await screen.findByLabelText("Allowed values");
      await userEvent.clear(allowed);
      await userEvent.type(allowed, "EU, US, APAC");
      await userEvent.tab();

      await userEvent.click(screen.getByRole("button", { name: "Save" }));
      await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());

      // What the server returned, not what was typed.
      await waitFor(() => expect(screen.getByLabelText("Allowed values")).toHaveValue("eu, us, apac"));
    });

    // The other half of the same defect: a focus/blur that typed nothing
    // must not `commit()` a stale local string back over the prop.
    it("commits nothing on a focus/blur that typed nothing", async () => {
      const endpoint = makeEndpoint({ params: [ENUM_PARAM] } as Partial<DataApiEndpointDto>);
      vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
        data: { status: 200, data: endpoint },
        isLoading: false,
        isError: false,
        refetch: vi.fn(),
      } as never);
      renderQuery(endpoint);

      const allowed = await screen.findByLabelText("Allowed values");
      await userEvent.click(allowed);
      await userEvent.tab();

      expect(screen.getByLabelText("Allowed values")).toHaveValue("eu, us");
      // Touching a field without changing it is not an edit.
      expect(screen.queryByText("Unsaved changes")).not.toBeInTheDocument();
    });
  });
});

/**
 * Final-review finding (Important #3): Publish ignored the shared `dirty` flag.
 *
 * The five sections share ONE form that survives section navigation
 * (`EndpointDetail` keys the `Outlet` by id, not by pathname), so an author
 * could edit SQL in Query, walk to Versions with "Unsaved changes" still in
 * the header, and publish — freezing the SAVED draft while the dialog said
 * "Freezes the current draft", which they would read as the one on screen.
 * Config already guards the far cheaper Validate this way; Publish changes
 * what every deployed consumer receives.
 *
 * This mounts Query AND Versions under the real shell and NAVIGATES between
 * them, because the bug only exists across that navigation — a test that
 * mounted Versions alone could never see it.
 */
describe("Query section — Publish and the shared dirty flag", () => {
  const publishAsync = vi.fn();

  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(databasesApi.useDatabases).mockReturnValue({
      data: [{ id: "db-1", name: "warehouse" }],
    } as never);
    vi.mocked(api.useDataApiPreviewEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    vi.mocked(dataApi.useUpdateEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    vi.mocked(accessApi.listUsers).mockResolvedValue([]);

    publishAsync.mockReset();
    publishAsync.mockResolvedValue({
      status: 200,
      data: { version_id: "v-3", version: 3, state: "live" },
      headers: new Headers(),
    });
    vi.mocked(dataApi.usePublishEndpoint).mockReturnValue({ mutateAsync: publishAsync, isPending: false } as never);
    vi.mocked(dataApi.useRollbackEndpoint).mockReturnValue({ mutateAsync: vi.fn(), isPending: false } as never);
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue({
      data: { status: 200, data: [] },
      isLoading: false,
      isError: false,
    } as never);
  });

  function renderBoth(endpoint: DataApiEndpointDto = makeEndpoint({ live_version_id: "v-2", state: "live" })) {
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    return render(
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={[`/explore/eng/data-api/endpoints/${endpoint.id}`]}>
          <Routes>
            <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
              <Route index element={<Query />} />
              <Route path="versions" element={<VersionsSection />} />
            </Route>
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>,
    );
  }

  it("refuses to publish an edit made in Query and carried unsaved into Versions", async () => {
    renderBoth();
    const sql = await screen.findByDisplayValue("SELECT 1");
    await userEvent.type(sql, " -- unsaved");
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("link", { name: "Versions" }));
    const publish = await screen.findByRole("button", { name: "Publish" });

    expect(publish).toBeDisabled();
    expect(publish).toHaveAttribute("title", expect.stringContaining("Save your changes"));

    // Not merely visually disabled: the confirmation never opens, so the
    // publish cannot be reached at all.
    await userEvent.click(publish);
    expect(screen.queryByText(/does not grant data access/i)).not.toBeInTheDocument();
    expect(publishAsync).not.toHaveBeenCalled();
  });

  it("publishes normally once there is nothing unsaved", async () => {
    renderBoth();
    await screen.findByDisplayValue("SELECT 1");
    await userEvent.click(screen.getByRole("link", { name: "Versions" }));

    const publish = await screen.findByRole("button", { name: "Publish" });
    expect(publish).toBeEnabled();
    await userEvent.click(publish);
    expect(await screen.findByText(/does not grant data access/i)).toBeInTheDocument();
  });

  // The dialog's own copy is part of the fix: "the current draft" is what the
  // author reads as "the one I am looking at".
  it("says the SAVED draft is what gets frozen", async () => {
    renderBoth();
    await screen.findByDisplayValue("SELECT 1");
    await userEvent.click(screen.getByRole("link", { name: "Versions" }));
    await userEvent.click(await screen.findByRole("button", { name: "Publish" }));
    expect(await screen.findByText(/freezes the saved draft/i)).toBeInTheDocument();
  });
});
