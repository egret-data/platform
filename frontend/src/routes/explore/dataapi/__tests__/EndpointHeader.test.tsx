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
import type {
  DataApiEndpointDto,
  DataApiEndpointVersionDto,
  EndpointSnapshot,
} from "@/api/generated/model";
import EndpointDetail, { useEndpointDetail } from "@/routes/explore/dataapi/EndpointDetail";
import {
  draftVsServed,
  endpointHeaderStateLabel,
  liveVersionLookup,
} from "@/routes/explore/dataapi/endpointState";
import type { EndpointStateTier } from "@/routes/explore/dataapi/endpointState";

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

/**
 * The snapshot `publish` would freeze for this endpoint — the fixture the whole
 * divergence question now turns on, so it models the real writer rather than a
 * plausible-looking object.
 *
 * `lifecycle::prospective_snapshot` — the ONE builder `validate` and `publish`
 * both go through — clones nine fields off the LOCKED endpoint row and adds
 * four of its own. The four are here and are all DIFFERENT from anything
 * on the draft on purpose: `columns` is non-empty, `iterated` is set,
 * `param_gate` is stamped and `delegated`/`impersonate` are present. A
 * comparison that failed to exclude any of them would call every published
 * endpoint diverged, and these values are what makes that fail rather than
 * pass.
 */
function snapshotOf(e: DataApiEndpointDto): EndpointSnapshot {
  return {
    statements: e.statements.map((st) => ({
      ...st,
      columns: [{ name: "one", ty: "bigint", required: true }],
    })),
    iterated: ["regions"],
    method: e.method,
    path: e.path,
    database_id: e.database_id,
    params: e.params,
    output: e.output,
    cache: e.cache,
    limits: e.limits,
    rate_limit: e.rate_limit,
    delegated: true,
    impersonate: true,
    param_gate: 1,
  } as unknown as EndpointSnapshot;
}

function makeVersion(overrides: Partial<DataApiEndpointVersionDto> = {}): DataApiEndpointVersionDto {
  return {
    id: "ver-1",
    version: 1,
    note: "",
    is_live: false,
    created_by: "user-1",
    created_at: "2026-08-02T00:00:00Z",
    snapshot: snapshotOf(makeEndpoint()),
    ...overrides,
  } as unknown as DataApiEndpointVersionDto;
}

/** The live version of an endpoint exactly as it stands — i.e. nothing to publish. */
function publishedAs(e: DataApiEndpointDto, version = 2): DataApiEndpointVersionDto {
  return makeVersion({
    id: `ver-${version}`,
    version,
    is_live: true,
    created_at: "2026-08-10T00:00:00Z",
    snapshot: snapshotOf(e),
  });
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
  { at = "", versionsQuery = "ok" }: { at?: string; versionsQuery?: "ok" | "error" | "loading" } = {},
) {
  vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
    data: { status: 200, data: endpoint },
    isLoading: false,
    isError: false,
    error: null,
    refetch: vi.fn(),
  } as unknown as ReturnType<typeof api.useDataApiGetEndpoint>);
  vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue({
    // The two states with no envelope at all: the request failed, or it has
    // not landed yet. Both used to reach the pill as an empty version list.
    data: versionsQuery === "ok" ? { status: 200, data: versions } : undefined,
    isLoading: versionsQuery === "loading",
    isError: versionsQuery === "error",
    error: versionsQuery === "error" ? new Error("versions unavailable") : null,
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
    const endpoint = makeEndpoint({
      state: "live",
      live_version_id: "ver-2",
      updated_at: "2026-08-10T00:00:00Z",
    });
    renderDetail(endpoint, [makeVersion(), publishedAs(endpoint)]);

    // The index route — the tab an author edits on, not the one Publish lives on.
    expect(screen.getByRole("button", { name: "make an edit" })).toBeInTheDocument();
    expect(statePill()).toHaveAccessibleName("Live · v2 — open the Versions tab");
    // …and the VISIBLE word, separately. The accessible name comes from an
    // `aria-label`, so a name assertion alone would pass over a pill that
    // showed something else entirely on screen.
    expect(screen.getByText("Live · v2")).toBeInTheDocument();
  });

  it("says the live version is BEHIND the draft once the draft is saved past it", () => {
    // The one fact the whole finding is about: the SQL a consumer is receiving
    // at v2 is not the SQL on screen. Expressed as a difference in the
    // DEFINITION, because that is what the pill now claims — not as a moved
    // timestamp, which was only ever a proxy for it.
    const published = makeEndpoint({ state: "live", live_version_id: "ver-2" });
    const edited = { ...published, statements: [{ name: "main", sql: "SELECT 42" }] };
    renderDetail(edited as DataApiEndpointDto, [makeVersion(), publishedAs(published)]);

    expect(statePill()).toHaveAccessibleName(
      "Live · v2 · unpublished changes — open the Versions tab",
    );
    expect(screen.getByText("Live · v2 · unpublished changes")).toBeInTheDocument();
  });

  it("does NOT claim divergence when the draft is exactly what was published", () => {
    // The control for the test above, and the state every endpoint is in for
    // the whole of its life until someone edits it.
    const endpoint = makeEndpoint({ state: "live", live_version_id: "ver-2" });
    renderDetail(endpoint, [makeVersion(), publishedAs(endpoint)]);

    expect(statePill()).toHaveAccessibleName("Live · v2 — open the Versions tab");
  });

  it("does NOT claim divergence after Disable then Enable", () => {
    // The defect that retired the timestamp rule. `enable` goes through
    // `EndpointStore::set_state` (`endpoints.rs:819`), which writes
    // `updated_at = now()` and puts `state` back to `live` — so publish,
    // disable, enable leaves `updated_at > liveVersion.created_at` on a `live`
    // endpoint FOREVER, and the old rule read that as "unpublished changes" on
    // an endpoint nobody had edited. The definition is untouched, so this says
    // so.
    const endpoint = makeEndpoint({
      state: "live",
      live_version_id: "ver-2",
      updated_at: "2026-09-30T00:00:00Z",
    });
    renderDetail(endpoint, [makeVersion(), publishedAs(endpoint)]);

    expect(statePill()).toHaveAccessibleName("Live · v2 — open the Versions tab");
  });

  it("does NOT claim divergence after rolling back to the version already live", () => {
    // The second writer the timestamp rule missed. `rollback`
    // (`lifecycle.rs:5240`) writes `live_version_id` and `updated_at = now()`
    // and leaves `state` alone, so rolling an endpoint back onto the version
    // it is ALREADY serving — a first-class button, `Versions.tsx:79` — used to
    // report it as diverged from itself.
    const endpoint = makeEndpoint({
      state: "live",
      live_version_id: "ver-2",
      updated_at: "2026-08-25T12:00:00Z",
    });
    const live = publishedAs(endpoint);
    renderDetail(endpoint, [makeVersion(), live]);

    expect(statePill()).toHaveAccessibleName("Live · v2 — open the Versions tab");
  });

  it("says it does not KNOW when the versions request failed", () => {
    // Finding 2. This used to render a bare, confident `Live` with a
    // `circle-check` beside it: the envelope was not a 200, the list became
    // `[]`, the live version became `undefined` and divergence became `false`.
    // A diverged endpoint read as in step for exactly as long as the request
    // kept failing, and nothing on screen hinted that anything had.
    renderDetail(makeEndpoint({ state: "live", live_version_id: "ver-2" }), [], {
      versionsQuery: "error",
    });

    expect(statePill()).toHaveAccessibleName(
      "Live · publish state unknown — open the Versions tab",
    );
    expect(screen.getByText("Live · publish state unknown")).toBeInTheDocument();
  });

  it("says it does not know on the first paint either, before the versions land", () => {
    renderDetail(makeEndpoint({ state: "live", live_version_id: "ver-2" }), [], {
      versionsQuery: "loading",
    });

    expect(statePill()).toHaveAccessibleName(
      "Live · publish state unknown — open the Versions tab",
    );
  });

  it("says it does not know when the list arrives without the live version in it", () => {
    // A `live` endpoint always has a `live_version_id`, so a 200 with no
    // `is_live` row means two readings of the same record disagree. Reporting
    // `Live` alone would pick the more flattering one.
    renderDetail(makeEndpoint({ state: "live", live_version_id: "ver-2" }), [makeVersion()]);

    expect(statePill()).toHaveAccessibleName(
      "Live · publish state unknown — open the Versions tab",
    );
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

  it("a paused endpoint with an edited draft still reads as Disabled, not as diverged", () => {
    // This pins the RENDERED pill and nothing more, and that is worth saying
    // out loud: `endpointHeaderStateLabel` and `endpointStateTier` both ignore
    // `served` unless the state is `live`, so this test passes with
    // `draftVsServed`'s own state guard deleted. Measured — that mutant
    // SURVIVED here when the guard returned a bare `false`. The guard now
    // returns its own value (`not-serving`) and is pinned where it lives,
    // below.
    const published = makeEndpoint({ state: "disabled", live_version_id: "ver-2" });
    const edited = { ...published, statements: [{ name: "main", sql: "SELECT 42" }] };
    renderDetail(edited as DataApiEndpointDto, [publishedAs(published)]);
    expect(statePill()).toHaveAccessibleName("Disabled · was live — open the Versions tab");
  });

  it("the pill is the route from an editing tab to the one place Publish lives", async () => {
    const user = userEvent.setup();
    const endpoint = makeEndpoint({ state: "live", live_version_id: "ver-2" });
    renderDetail(endpoint, [makeVersion(), publishedAs(endpoint)]);

    await user.click(statePill());
    expect(screen.getByRole("heading", { name: "the versions tab" })).toBeInTheDocument();
  });

  /**
   * One case per tier, shared by the glyph test and the ink test — because the
   * ink test used to assert the LIVE tier only while the glyph test looped all
   * of them, so a regression to `text-destructive` on `disabled` survived it.
   * A per-tier table is what makes "all four" (now five) a fact about the
   * table rather than about which case someone remembered to add.
   */
  /**
   * Every tier the pill can paint, listed once. The `satisfies` is what makes
   * it exhaustive: a sixth member of `EndpointStateTier` fails to COMPILE
   * until it is named here, so the table below cannot fall behind the type.
   */
  const ALL_TIERS = Object.keys({
    live: true,
    diverged: true,
    unknown: true,
    draft: true,
    disabled: true,
  } satisfies Record<EndpointStateTier, true>);

  function tierCases(): [string, DataApiEndpointDto, DataApiEndpointVersionDto[], "ok" | "error"][] {
    const liveEndpoint = makeEndpoint({ state: "live", live_version_id: "ver-2" });
    const edited = {
      ...liveEndpoint,
      statements: [{ name: "main", sql: "SELECT 42" }],
    } as DataApiEndpointDto;
    return [
      ["live", liveEndpoint, [publishedAs(liveEndpoint)], "ok"],
      ["diverged", edited, [publishedAs(liveEndpoint)], "ok"],
      ["unknown", liveEndpoint, [], "error"],
      ["draft", makeEndpoint({ state: "draft" }), [], "ok"],
      ["disabled", makeEndpoint({ state: "disabled", live_version_id: "ver-2" }), [], "ok"],
    ];
  }

  it("state is not carried by colour alone — every tier has its own glyph", () => {
    // The icon is DECORATION beside a word (`aria-hidden`), so it is asserted
    // by IDENTITY: five tiers, five distinct glyphs. A count would pass over a
    // component that painted the same icon five times.
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

    const cases = tierCases();
    const seen: string[] = [];
    for (const [, endpoint, versions, versionsQuery] of cases) {
      renderDetail(endpoint, versions, { versionsQuery });
      seen.push(glyph());
      cleanup();
    }

    // Against the tier NAMES, not against `cases.length`: that read the test's
    // own table on both sides of the equality, so deleting a row shrank the
    // coverage and the assertion stayed true. It has to be an outside number.
    expect(cases.map(([tier]) => tier)).toEqual(ALL_TIERS);
    expect(new Set(seen).size).toBe(ALL_TIERS.length);
  });

  it("the pill's ink is --foreground on EVERY tier, never a tinted ink that fails contrast", () => {
    // Measured in Chromium by painting the composite onto a canvas and reading
    // the pixel back (never by parsing `oklch()`): `text-success` on
    // `bg-success/15` is 3.72:1 in light mode — under the 4.5:1 that WCAG
    // 2.2 1.4.3 asks of this 12px text. `--foreground` on the same fill is
    // 14.76:1 light / 15.95:1 dark. The tint stays; the ink does not.
    //
    // This is a CLASS-STRING assertion and nothing paints in jsdom, so it
    // proves the wiring — that each tier is handed `text-foreground` and no
    // tinted ink — and not the contrast itself. The numbers above are the
    // measurement; this is the regression guard on the decision they led to.
    for (const [tier, endpoint, versions, versionsQuery] of tierCases()) {
      const { container } = renderDetail(endpoint, versions, { versionsQuery });
      const badge = container.querySelector('[data-slot="badge"]')!;
      expect(badge.className, tier).toContain("text-foreground");
      expect(badge.className, tier).not.toContain("text-success");
      expect(badge.className, tier).not.toContain("text-destructive");
      expect(badge.className, tier).not.toContain("text-highlight");
      cleanup();
    }
  });
});

describe("the in-app back link asks before it discards an edit", () => {
  it("leaves immediately when there is nothing to lose", async () => {
    const user = userEvent.setup();
    renderDetail(makeEndpoint());

    await user.click(screen.getByRole("link", { name: "Endpoints" }));

    expect(screen.getByRole("heading", { name: "the endpoint list page" })).toBeInTheDocument();
  });

  it("does not navigate when the form is dirty — it asks", async () => {
    const user = userEvent.setup();
    renderDetail(makeEndpoint());

    await user.click(screen.getByRole("button", { name: "make an edit" }));
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();

    await user.click(screen.getByRole("link", { name: "Endpoints" }));

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
    await user.click(screen.getByRole("link", { name: "Endpoints" }));
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
    await user.click(screen.getByRole("link", { name: "Endpoints" }));
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

    await user.click(screen.getByRole("link", { name: "Endpoints" }));

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

/**
 * `draftVsServed` is exported and its name makes a claim, so it is pinned as a
 * unit rather than only through whatever the pill happens to render today.
 * Several of these are invisible from the rendered header — the label and the
 * tier both ignore `served` off the `live` state — which is exactly why the
 * contract is asserted where it is written. The previous rule learnt that the
 * hard way: the mutant that deleted its state guard SURVIVED a render test.
 */
describe("draftVsServed", () => {
  const live = (o: Partial<DataApiEndpointDto> = {}) =>
    makeEndpoint({ state: "live", live_version_id: "ver-2", ...o });
  const known = (v: DataApiEndpointVersionDto | undefined) => ({ known: true as const, version: v });

  it("is in-step when the draft is exactly the definition being served", () => {
    const e = live();
    expect(draftVsServed(e, known(publishedAs(e)))).toBe("in-step");
  });

  it("is in-step however far `updated_at` has moved — Enable and Roll back move it", () => {
    // The finding that retired the timestamp rule, at the level the rule lived
    // at. THREE writers of `dataapi.endpoints.updated_at` leave or restore
    // `state = 'live'` without touching the definition: `set_state` via
    // `enable` (`endpoints.rs:819`), `rollback` (`lifecycle.rs:5240`), and
    // `update` itself when the author changes only a field the snapshot does
    // not carry. A comparison against the snapshot cannot see any of them,
    // which is the whole reason it replaced the clock.
    const e = live({ updated_at: "2099-01-01T00:00:00Z" });
    expect(draftVsServed(e, known(publishedAs(e)))).toBe("in-step");
  });

  it("is in-step after a rename — a rename changes nothing a consumer receives", () => {
    // `name` and `description` are on the endpoint row and NOT in the
    // snapshot, so publishing after a rename freezes a version identical to
    // the last one. The old rule lit the pill on this.
    const published = live();
    const renamed = { ...published, name: "Renamed", description: "new words" } as DataApiEndpointDto;
    expect(draftVsServed(renamed, known(publishedAs(published)))).toBe("in-step");
  });

  it("ignores everything publish COMPUTES rather than copies", () => {
    // `columns`, `iterated`, `param_gate`, `delegated` and `impersonate` are
    // all written by publish and have no counterpart on the draft. Comparing
    // any of them would report every published endpoint as diverged; this
    // fixture carries a different value for each.
    const e = live();
    const version = publishedAs(e);
    const snapshot = version.snapshot as unknown as Record<string, unknown>;
    expect(snapshot.iterated).toEqual(["regions"]);
    expect(snapshot.param_gate).toBe(1);
    expect((snapshot.statements as { columns: unknown[] }[])[0].columns).toHaveLength(1);
    expect(draftVsServed(e, known(version))).toBe("in-step");
  });

  it("is ahead when a statement's SQL differs from the frozen one", () => {
    const published = live();
    const edited = {
      ...published,
      statements: [{ name: "main", sql: "SELECT 42" }],
    } as DataApiEndpointDto;
    expect(draftVsServed(edited, known(publishedAs(published)))).toBe("ahead");
  });

  it("is ahead when a statement is ADDED, not only when one is changed", () => {
    const published = live();
    const edited = {
      ...published,
      statements: [...published.statements, { name: "total", sql: "SELECT count(*)" }],
    } as DataApiEndpointDto;
    expect(draftVsServed(edited, known(publishedAs(published)))).toBe("ahead");
  });

  it("is ahead when a declared parameter changes", () => {
    const published = live();
    const edited = {
      ...published,
      params: [{ name: "region", type: "string", required: true }],
    } as unknown as DataApiEndpointDto;
    expect(draftVsServed(edited, known(publishedAs(published)))).toBe("ahead");
  });

  it("is ahead when the route or the database moves under the same SQL", () => {
    const published = live();
    for (const change of [
      { path: "reports/weekly" },
      { method: "POST" },
      { database_id: "db-2" },
    ]) {
      const edited = { ...published, ...change } as DataApiEndpointDto;
      expect(draftVsServed(edited, known(publishedAs(published))), JSON.stringify(change)).toBe(
        "ahead",
      );
    }
  });

  it("compares against the LIVE version, not the newest one", () => {
    // Reachable: publish v1, edit, publish v2, roll back to v1, edit back. A
    // comparison against `versions[0]` — the list is `ORDER BY version DESC` —
    // would call this diverged.
    const draft = live({ live_version_id: "ver-1" });
    const other = makeEndpoint({ statements: [{ name: "main", sql: "SELECT 42" }] });
    const newest = makeVersion({ id: "ver-2", version: 2, is_live: false, snapshot: snapshotOf(other) });
    const serving = makeVersion({ id: "ver-1", version: 1, is_live: true, snapshot: snapshotOf(draft) });
    expect(draftVsServed(draft, liveVersionLookup({ status: 200, data: [newest, serving] }))).toBe(
      "in-step",
    );
  });

  it("is not-serving for a DISABLED endpoint, whatever its draft says", () => {
    // The state guard, asserted on its OWN return value. It used to fold into
    // `false` alongside "in step", which made it invisible to every test:
    // deleting it changed nothing anything could observe.
    const published = makeEndpoint({ state: "disabled", live_version_id: "ver-2" });
    const edited = {
      ...published,
      statements: [{ name: "main", sql: "SELECT 42" }],
    } as DataApiEndpointDto;
    expect(draftVsServed(edited, known(publishedAs(published)))).toBe("not-serving");
  });

  it("is not-serving for a draft, which has no live version to be ahead of", () => {
    expect(draftVsServed(makeEndpoint({ state: "draft" }), known(undefined))).toBe("not-serving");
  });

  it("is unknown when the caller could not read the version list", () => {
    // Finding 2's whole subject: this is NOT `in-step`, and the type makes it
    // impossible to spell it as one.
    expect(draftVsServed(live(), { known: false })).toBe("unknown");
  });

  it("is unknown when the list was read but carries no live version", () => {
    expect(draftVsServed(live(), known(undefined))).toBe("unknown");
  });

  it("is unknown, never in-step, on a snapshot it cannot read", () => {
    // A snapshot with no statements cannot be the definition of anything, so
    // there is nothing to compare. `unknown` rather than `ahead` because we do
    // not know either way, and rather than `in-step` because that is the claim
    // the pill must never make on no evidence.
    const broken = makeVersion({ id: "ver-2", version: 2, is_live: true, snapshot: {} as EndpointSnapshot });
    expect(draftVsServed(live(), known(broken))).toBe("unknown");
  });

  it("does not depend on the order the JSON keys happen to arrive in", () => {
    const published = live();
    const version = publishedAs(published);
    const reordered = {
      ...published,
      output: { format: "json", pretty: true },
    } as unknown as DataApiEndpointDto;
    version.snapshot = {
      ...(version.snapshot as object),
      output: { pretty: true, format: "json" },
    } as unknown as EndpointSnapshot;
    expect(draftVsServed(reordered, known(version))).toBe("in-step");
  });

  /**
   * THE FOUR COMPARED FIELDS NO FIXTURE EVER MOVED.
   *
   * `output`, `cache`, `limits` and `rate_limit` are four of the nine fields
   * `draftVsServed` compares, and until now every fixture in this file left all
   * four as `{}` on BOTH sides. Measured: deleting their four lines from both
   * mirror projections killed ZERO of 42 tests. Four ninths of the rule was
   * guarded by nothing.
   *
   * They are not decoration — each one is part of the contract a consumer is
   * being served under, and none of them shows up in the SQL an author is
   * looking at while the pill claims to speak for the whole definition:
   *
   * - `limits.max_rows` / `timeout_seconds` — the caps a response is truncated
   *   and timed out by (`LimitsSpec`, `egret-dataapi-api/src/lib.rs:172`).
   * - `rate_limit.per_key_qps` / `burst` / `per_endpoint_qps` — how often a key
   *   may call at all (`RateLimitSpec`, same file, `:187`).
   * - `cache.ttl_seconds` / `scope` — how stale an answer may be, and whether
   *   it is shared across principals (`CacheSpec`, `:153`).
   * - `output.envelope` — whether the body is `{status, message, data, meta}`
   *   or `data` on its own (`OutputSpec`, `:1273`); `shape` and `naming` change
   *   the row shape and the key casing.
   *
   * Raise a cap, save, and a pill that had stopped comparing these would read
   * `Live · v2` with a check beside it while consumers keep the old ones.
   *
   * Both sides carry a full, realistic spec throughout and only ONE VALUE ONE
   * LEVEL DOWN moves, which is the interesting case: a projection that compared
   * these fields merely by presence, or a `sameJson` that stopped recursing,
   * would still pass a presence/absence test.
   */
  const servedSpecs: Pick<DataApiEndpointDto, "output" | "cache" | "limits" | "rate_limit"> = {
    output: { shape: "objects", naming: "as_is", envelope: "wrapped" },
    cache: { enabled: true, ttl_seconds: 60, scope: "per_principal" },
    limits: { max_rows: 10_000, timeout_seconds: 30 },
    rate_limit: { per_key_qps: 10, burst: 20, per_endpoint_qps: 100 },
  };

  /** One nested edit per field — the smallest change an author can save. */
  const nestedEdits: [keyof typeof servedSpecs, Record<string, unknown>][] = [
    ["output", { envelope: "bare" }],
    ["cache", { ttl_seconds: 3600 }],
    ["limits", { max_rows: 50_000 }],
    ["rate_limit", { per_key_qps: 50 }],
  ];

  it.each(nestedEdits)("is ahead when `%s` moves, under untouched SQL", (field, edit) => {
    const published = live(servedSpecs);
    const edited = {
      ...published,
      [field]: { ...servedSpecs[field], ...edit },
    } as unknown as DataApiEndpointDto;
    expect(draftVsServed(edited, known(publishedAs(published)))).toBe("ahead");
  });

  it.each(nestedEdits)("is in-step when `%s` is an equal value, freshly built", (field) => {
    // The control for the row above, and the half that would fail if the field
    // were compared by object IDENTITY: the draft rebuilds the same spec every
    // time the form round-trips, so an equal-but-not-same value must not light
    // the pill on an endpoint nobody edited.
    const published = live(servedSpecs);
    const redrawn = {
      ...published,
      [field]: { ...servedSpecs[field] },
    } as unknown as DataApiEndpointDto;
    expect(draftVsServed(redrawn, known(publishedAs(published)))).toBe("in-step");
  });
});

describe("liveVersionLookup", () => {
  it("reports NOT KNOWN for an absent envelope — the first paint", () => {
    expect(liveVersionLookup(undefined)).toEqual({ known: false });
  });

  it("reports NOT KNOWN for any status but 200, even when the body IS a list", () => {
    // The status check and the shape check are separate guards, and a body
    // that happens to parse as an array — a gateway replaying a cached
    // response under a 503, say — gets past the shape one on its own.
    expect(liveVersionLookup({ status: 500, data: { message: "boom" } })).toEqual({ known: false });
    expect(liveVersionLookup({ status: 503, data: [makeVersion({ is_live: true })] })).toEqual({
      known: false,
    });
  });

  it("reports KNOWN with no version when the endpoint has published nothing", () => {
    expect(liveVersionLookup({ status: 200, data: [] })).toEqual({ known: true, version: undefined });
  });

  it("picks the row the SERVER marked live, not the newest", () => {
    const newest = makeVersion({ id: "ver-2", version: 2, is_live: false });
    const serving = makeVersion({ id: "ver-1", version: 1, is_live: true });
    expect(liveVersionLookup({ status: 200, data: [newest, serving] })).toEqual({
      known: true,
      version: serving,
    });
  });
});

describe("endpointHeaderStateLabel", () => {
  it("prints version 0 rather than swallowing it into a bare `Live`", () => {
    // Latent — `COALESCE(MAX(version), 0) + 1` starts at 1 — but a truthiness
    // gate on a NUMBER prints the most confident label the pill has for a
    // version it actually knows. `== null` is the gate that cannot.
    expect(endpointHeaderStateLabel({ state: "live", live_version_id: "v" }, { liveVersion: 0, served: "in-step" })).toBe(
      "Live · v0",
    );
  });

  it("defaults to saying it does not know, not to saying everything is fine", () => {
    // The fail-honest default: a caller that forgets to pass `served` gets the
    // unknown label, never the confident one.
    expect(endpointHeaderStateLabel({ state: "live", live_version_id: "v" }, { liveVersion: 2 })).toBe(
      "Live · v2 · publish state unknown",
    );
  });
});
