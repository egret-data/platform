import { describe, it, expect, vi, beforeEach } from "vitest";
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// Same shape as every other Data API test file: no msw in this repo, so the
// generated API module is mocked directly and the hand-written wrapper
// (`@/api/dataApi`) is left to run on top of it.
vi.mock("@/api/generated/data-api/data-api");

import * as api from "@/api/generated/data-api/data-api";
import type { DataApiEndpointDto, DataApiEndpointVersionDto } from "@/api/generated/model";
import EndpointDetail, { useEndpointDetail } from "@/routes/explore/dataapi/EndpointDetail";

/**
 * THE HEADER SAYS WHAT THE ENDPOINT IS, AND THE BACK LINK ASKS BEFORE IT
 * THROWS AN EDIT AWAY.
 *
 * Two findings, one sentence: the app knows and does not say.
 *
 * 1. An endpoint whose API answers `state: "live"` with a `live_version_id`
 *    carried the word "Live" on the LIST and nowhere on the six tabs the
 *    author actually edits on — and after a Save, nothing anywhere said the
 *    served version was behind the draft. Measured live: mid-divergence the
 *    router returned `{"one":1}` at `version:1` while the draft read
 *    `SELECT 42`. Save and Publish had different consequences and the same
 *    signal.
 *
 * 2. `beforeunload` warned on a browser reload; clicking `← Endpoints`
 *    discarded the edit in silence. The app knew it was dirty — the badge
 *    rendered and the handler was installed — and the guarded door was the one
 *    nobody uses.
 *
 * Every assertion below is on an OBSERVABLE, never on a click: the leave tests
 * mount a real sibling route and assert on which route is on screen, so a
 * guard that opens a dialog and navigates anyway fails, and so does a guard
 * that swallows the click without ever letting the author through.
 */

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

function makeVersion(overrides: Partial<DataApiEndpointVersionDto> = {}): DataApiEndpointVersionDto {
  return {
    id: "ver-1",
    version: 1,
    note: "",
    is_live: false,
    created_by: "user-1",
    created_at: "2026-08-02T00:00:00Z",
    snapshot: { statements: [{ name: "main", sql: "SELECT 1", columns: [] }] },
    ...overrides,
  } as unknown as DataApiEndpointVersionDto;
}

const updateMutateAsync = vi.fn();
const restoreMutateAsync = vi.fn();

/**
 * A stand-in for whichever of the six sections the author happens to be on.
 *
 * It edits through the SHELL's `setForm`, which is the only way any real
 * section makes the page dirty, so "dirty" here is produced by the same
 * mechanism the product uses rather than by a flag the test sets. `name` is
 * the field it moves because `diffToUpdateInput` compares it as a plain
 * string — nothing about the assertions depends on which field it is.
 */
function EditingSection() {
  const { setForm } = useEndpointDetail();
  return (
    <button type="button" onClick={() => setForm((f) => ({ ...f, name: "Edited name" }))}>
      make an edit
    </button>
  );
}

function renderDetail(
  endpoint: DataApiEndpointDto,
  versions: DataApiEndpointVersionDto[] = [],
  { at = "" }: { at?: string } = {},
) {
  vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
    data: { status: 200, data: endpoint },
    isLoading: false,
    isError: false,
    error: null,
    refetch: vi.fn(),
  } as unknown as ReturnType<typeof api.useDataApiGetEndpoint>);
  vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue({
    data: { status: 200, data: versions },
    isLoading: false,
    isError: false,
    error: null,
  } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>);

  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/explore/eng/data-api/endpoints/${endpoint.id}${at}`]}>
        <Routes>
          {/* The real sibling route, so "did the author leave?" is a question
              about what is on screen and not about whether a spy was called. */}
          <Route
            path="/explore/:workspace/data-api/endpoints"
            element={<h1>the endpoint list page</h1>}
          />
          <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
            <Route index element={<EditingSection />} />
            <Route path="versions" element={<h1>the versions tab</h1>} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** The header's state pill, found by ROLE — it is a link into the lifecycle. */
function statePill() {
  return screen.getByRole("link", { name: /open the Versions tab/ });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.useDataApiUpdateEndpoint).mockReturnValue({
    mutateAsync: updateMutateAsync,
    isPending: false,
  } as unknown as ReturnType<typeof api.useDataApiUpdateEndpoint>);
  vi.mocked(api.useDataApiRestoreEndpoint).mockReturnValue({
    mutateAsync: restoreMutateAsync,
    isPending: false,
  } as unknown as ReturnType<typeof api.useDataApiRestoreEndpoint>);
  vi.mocked(api.getDataApiGetEndpointQueryKey).mockImplementation(
    (ws, id) => ["dataApiGetEndpoint", ws, id] as unknown as ReturnType<typeof api.getDataApiGetEndpointQueryKey>,
  );
  vi.mocked(api.getDataApiListEndpointsQueryKey).mockImplementation(
    (ws) => ["dataApiListEndpoints", ws] as unknown as ReturnType<typeof api.getDataApiListEndpointsQueryKey>,
  );
  updateMutateAsync.mockImplementation(
    ({ data }: { workspace: string; id: string; data: Partial<DataApiEndpointDto> }) =>
      Promise.resolve({ status: 200, data: { ...makeEndpoint(), ...data }, headers: new Headers() }),
  );
});

describe("the header says what the endpoint is", () => {
  it("names the live version, on a tab that is not Versions", () => {
    const live = makeVersion({ id: "ver-2", version: 2, is_live: true, created_at: "2026-08-10T00:00:00Z" });
    renderDetail(
      makeEndpoint({ state: "live", live_version_id: "ver-2", updated_at: "2026-08-10T00:00:00Z" }),
      [makeVersion(), live],
    );

    // The index route — the tab an author edits on, not the one Publish lives on.
    expect(screen.getByRole("button", { name: "make an edit" })).toBeInTheDocument();
    expect(statePill()).toHaveAccessibleName("Live · v2 — open the Versions tab");
    // …and the VISIBLE word, separately. The accessible name comes from an
    // `aria-label`, so a name assertion alone would pass over a pill that
    // showed something else entirely on screen.
    expect(screen.getByText("Live · v2")).toBeInTheDocument();
  });

  it("says the live version is BEHIND the draft once the draft is saved past it", () => {
    const live = makeVersion({ id: "ver-2", version: 2, is_live: true, created_at: "2026-08-10T00:00:00Z" });
    renderDetail(
      // The one fact the whole finding is about: the endpoint row moved after
      // the live version was frozen, so a consumer is receiving v2 while the
      // saved draft has moved on.
      makeEndpoint({ state: "live", live_version_id: "ver-2", updated_at: "2026-08-11T09:00:00Z" }),
      [makeVersion(), live],
    );

    expect(statePill()).toHaveAccessibleName(
      "Live · v2 · unpublished changes — open the Versions tab",
    );
    expect(screen.getByText("Live · v2 · unpublished changes")).toBeInTheDocument();
  });

  it("does NOT claim divergence when the draft has not moved since the publish", () => {
    // The control for the test above. Publish writes `endpoints.updated_at =
    // now()` in the same transaction that inserts the version row, and both
    // are Postgres `now()` — the transaction timestamp — so the two stamps are
    // EQUAL immediately after a publish. A `>=` here would report every
    // freshly published endpoint as diverged.
    const live = makeVersion({ id: "ver-2", version: 2, is_live: true, created_at: "2026-08-10T00:00:00Z" });
    renderDetail(
      makeEndpoint({ state: "live", live_version_id: "ver-2", updated_at: "2026-08-10T00:00:00Z" }),
      [makeVersion(), live],
    );

    expect(statePill()).toHaveAccessibleName("Live · v2 — open the Versions tab");
  });

  it("distinguishes a draft that has never been published", () => {
    renderDetail(makeEndpoint({ state: "draft", live_version_id: null }), []);
    expect(statePill()).toHaveAccessibleName("Draft · never published — open the Versions tab");
    expect(screen.getByText("Draft · never published")).toBeInTheDocument();
  });

  it("distinguishes a disabled endpoint that WAS live from one that never was", () => {
    renderDetail(makeEndpoint({ state: "disabled", live_version_id: "ver-2" }), []);
    expect(statePill()).toHaveAccessibleName("Disabled · was live — open the Versions tab");
  });

  it("a disabled endpoint that never published says so", () => {
    renderDetail(makeEndpoint({ state: "disabled", live_version_id: null }), []);
    expect(statePill()).toHaveAccessibleName(
      "Disabled · never published — open the Versions tab",
    );
  });

  it("a disabled endpoint is never reported as diverged, however far its row has moved", () => {
    // `disable` writes `updated_at = now()` and leaves `live_version_id`
    // alone (`endpoints.rs`), so the timestamp comparison alone says "the row
    // moved after the version was frozen" for every endpoint anyone has ever
    // paused. Divergence is a claim about what a LIVE endpoint is serving.
    const live = makeVersion({ id: "ver-2", version: 2, is_live: true, created_at: "2026-08-10T00:00:00Z" });
    renderDetail(
      makeEndpoint({ state: "disabled", live_version_id: "ver-2", updated_at: "2026-08-30T00:00:00Z" }),
      [live],
    );
    expect(statePill()).toHaveAccessibleName("Disabled · was live — open the Versions tab");
  });

  it("the pill is the route from an editing tab to the one place Publish lives", async () => {
    const user = userEvent.setup();
    const live = makeVersion({ id: "ver-2", version: 2, is_live: true, created_at: "2026-08-10T00:00:00Z" });
    renderDetail(
      makeEndpoint({ state: "live", live_version_id: "ver-2", updated_at: "2026-08-11T00:00:00Z" }),
      [makeVersion(), live],
    );

    await user.click(statePill());
    expect(screen.getByRole("heading", { name: "the versions tab" })).toBeInTheDocument();
  });

  it("state is not carried by colour alone — every tier has its own glyph", () => {
    // The icon is DECORATION beside a word (`aria-hidden`), so it is asserted
    // by IDENTITY: four tiers, four distinct glyphs. A count would pass over a
    // component that painted the same icon four times.
    const live = makeVersion({ id: "ver-2", version: 2, is_live: true, created_at: "2026-08-10T00:00:00Z" });
    const glyph = () => {
      // lucide stamps each icon with its own name class (`lucide-circle-check`),
      // which is what makes two glyphs distinguishable in jsdom, where nothing
      // paints. Every tier gets the same Tailwind classes, so the class string
      // differs ONLY where the glyph does.
      const named = screen
        .getByTestId("endpoint-state-icon")
        .getAttribute("class")!
        .split(/\s+/)
        .filter((c) => c.startsWith("lucide-"))
        .sort()
        .join(" ");
      expect(named).not.toBe("");
      return named;
    };

    const seen: string[] = [];
    for (const [endpoint, versions] of [
      [makeEndpoint({ state: "live", live_version_id: "ver-2", updated_at: "2026-08-10T00:00:00Z" }), [live]],
      [makeEndpoint({ state: "live", live_version_id: "ver-2", updated_at: "2026-08-11T00:00:00Z" }), [live]],
      [makeEndpoint({ state: "draft" }), []],
      [makeEndpoint({ state: "disabled", live_version_id: "ver-2" }), []],
    ] as [DataApiEndpointDto, DataApiEndpointVersionDto[]][]) {
      renderDetail(endpoint, versions);
      seen.push(glyph());
      cleanup();
    }

    expect(new Set(seen).size).toBe(4);
  });

  it("the pill's ink is --foreground, never a tinted ink that fails contrast", () => {
    // Measured in Chromium by painting the composite onto a canvas and reading
    // the pixel back (never by parsing `oklch()`): `text-success` on
    // `bg-success/15` is 3.72:1 in light mode — under the 4.5:1 that WCAG
    // 2.2 1.4.3 asks of this 12px text. `--foreground` on the same fill is
    // 14.76:1 light / 15.95:1 dark. The tint stays; the ink does not.
    const live = makeVersion({ id: "ver-2", version: 2, is_live: true, created_at: "2026-08-10T00:00:00Z" });
    const { container } = renderDetail(
      makeEndpoint({ state: "live", live_version_id: "ver-2", updated_at: "2026-08-10T00:00:00Z" }),
      [live],
    );
    const badge = container.querySelector('[data-slot="badge"]')!;
    expect(badge.className).toContain("text-foreground");
    expect(badge.className).not.toContain("text-success");
    expect(badge.className).not.toContain("text-destructive");
    expect(badge.className).not.toContain("text-highlight");
  });
});

describe("the in-app back link asks before it discards an edit", () => {
  it("leaves immediately when there is nothing to lose", async () => {
    const user = userEvent.setup();
    renderDetail(makeEndpoint());

    await user.click(screen.getByRole("link", { name: "← Endpoints" }));

    expect(screen.getByRole("heading", { name: "the endpoint list page" })).toBeInTheDocument();
  });

  it("does not navigate when the form is dirty — it asks", async () => {
    const user = userEvent.setup();
    renderDetail(makeEndpoint());

    await user.click(screen.getByRole("button", { name: "make an edit" }));
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();

    await user.click(screen.getByRole("link", { name: "← Endpoints" }));

    // The observable, not the click: the list route is NOT on screen.
    expect(screen.queryByRole("heading", { name: "the endpoint list page" })).toBeNull();
    expect(
      screen.getByRole("alertdialog", { name: "Leave without saving?" }),
    ).toBeInTheDocument();
  });

  it("staying keeps the edit AND the page", async () => {
    const user = userEvent.setup();
    renderDetail(makeEndpoint());

    await user.click(screen.getByRole("button", { name: "make an edit" }));
    await user.click(screen.getByRole("link", { name: "← Endpoints" }));
    const dialog = screen.getByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Stay on this page" }));

    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.queryByRole("heading", { name: "the endpoint list page" })).toBeNull();
    // Still dirty: Save is live and the badge is still up, so "Stay" did not
    // quietly reset the form it was protecting.
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });

  it("discarding really does leave", async () => {
    const user = userEvent.setup();
    renderDetail(makeEndpoint());

    await user.click(screen.getByRole("button", { name: "make an edit" }));
    await user.click(screen.getByRole("link", { name: "← Endpoints" }));
    const dialog = screen.getByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Discard changes and leave" }));

    expect(screen.getByRole("heading", { name: "the endpoint list page" })).toBeInTheDocument();
  });

  it("saving first clears the guard, so the next click just leaves", async () => {
    const user = userEvent.setup();
    renderDetail(makeEndpoint());

    await user.click(screen.getByRole("button", { name: "make an edit" }));
    await user.click(screen.getByRole("button", { name: "Save" }));
    await screen.findByRole("button", { name: "Save" });

    await user.click(screen.getByRole("link", { name: "← Endpoints" }));

    expect(screen.getByRole("heading", { name: "the endpoint list page" })).toBeInTheDocument();
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("the tab strip is NOT guarded, because switching tabs keeps the form", async () => {
    // Not an omission — the six sections share ONE form owned by the shell and
    // the `Outlet` is keyed by id, not by pathname, so moving between tabs
    // loses nothing. A confirm dialog here would be a false alarm on the one
    // navigation that is always safe.
    const user = userEvent.setup();
    renderDetail(makeEndpoint());

    await user.click(screen.getByRole("button", { name: "make an edit" }));
    await user.click(screen.getByRole("link", { name: "Versions" }));

    expect(screen.getByRole("heading", { name: "the versions tab" })).toBeInTheDocument();
    expect(screen.queryByRole("alertdialog")).toBeNull();
    // The edit survived the move, which is what makes the un-guarded tab safe.
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();
  });
});
