import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Outlet, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

vi.mock("@/api/generated/data-api/data-api");
vi.mock("@/api/dataApi");

import * as gen from "@/api/generated/data-api/data-api";
import * as wrapped from "@/api/dataApi";
import type { EndpointDetailContext } from "@/routes/explore/dataapi/EndpointDetail";
import VersionsSection from "@/routes/explore/dataapi/detail/Versions";

/**
 * The publish pre-flight (package 6).
 *
 * **What these assert, and what they deliberately do not.** The gate is the
 * server's: `EndpointStore::publish` recomputes the diff under the endpoint's
 * row lock and refuses without the acknowledgement, and that is covered by
 * `crates/egret-dataapi/tests/lifecycle.rs`. What is testable here is the
 * console's half — that an author is shown the breaks, that the acknowledgement
 * cannot be given by accident, and that the token sent back is the server's
 * own and not something this file computed.
 *
 * The blocked case asserts the mutation was **never called**, not that the
 * button carries `disabled`. A `toBeDisabled` assertion in this repo has passed
 * against a component with the guard deleted, because the matcher walks up to
 * an ancestor `fieldset[disabled]`; "the publish did not happen" cannot be
 * satisfied that way.
 */

const LIVE_VERSION = {
  id: "v-1",
  version: 3,
  note: "",
  is_live: true,
  created_by: "user-1",
  created_at: "2026-08-01T00:00:00Z",
  snapshot: { statements: [{ name: "main", sql: "SELECT 1", columns: [] }] },
  breaking_changes: null,
};

const REMOVED_COLUMN = {
  kind: "column_removed",
  subject: "column `n` of statement `main`",
  detail: "declared `bigint` and is gone",
};

/** A token the test never constructs a rule for — it must be echoed verbatim. */
const TOKEN = "3f9a1c00deadbeef";

function mockValidate(data: Record<string, unknown>) {
  const mutateAsync = vi.fn(async () => ({ status: 200, data }));
  vi.mocked(gen.useDataApiValidateEndpoint).mockReturnValue({
    mutateAsync,
    isPending: false,
  } as unknown as ReturnType<typeof gen.useDataApiValidateEndpoint>);
  return mutateAsync;
}

/** Just enough of the publish mutation's variables for the assertions below. */
type PublishVars = {
  workspace: string;
  id: string;
  data: { note?: string; acknowledge_breaking?: string };
};

function mockPublish() {
  const mutateAsync: ReturnType<typeof vi.fn> = vi.fn(async (_vars: PublishVars) => ({
    status: 200,
    data: { version: 4 },
  }));
  vi.mocked(wrapped.usePublishEndpoint).mockReturnValue({
    mutateAsync,
    isPending: false,
  } as unknown as ReturnType<typeof wrapped.usePublishEndpoint>);
  return mutateAsync;
}

function Harness() {
  const ctx: Partial<EndpointDetailContext> = {
    ws: "ws1",
    id: "ep-1",
    dirty: false,
    trashed: false,
    refetch: vi.fn(),
  };
  return <Outlet context={ctx} />;
}

function renderVersions() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/versions"]}>
        <Routes>
          <Route path="/versions" element={<Harness />}>
            <Route index element={<VersionsSection />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(gen.useDataApiListEndpointVersions).mockReturnValue({
    data: { status: 200, data: [LIVE_VERSION] },
    refetch: vi.fn(),
  } as unknown as ReturnType<typeof gen.useDataApiListEndpointVersions>);
  vi.mocked(wrapped.useRollbackEndpoint).mockReturnValue({
    mutateAsync: vi.fn(),
    isPending: false,
  } as unknown as ReturnType<typeof wrapped.useRollbackEndpoint>);
});

async function openPublish() {
  await userEvent.click(await screen.findByRole("button", { name: /publish saved draft/i }));
}

describe("the publish dialog's contract pre-flight", () => {
  it("refuses to publish a breaking change until it is acknowledged", async () => {
    mockValidate({
      statements: [],
      against_live: { breaking: [REMOVED_COLUMN], safe: [] },
      acknowledge_breaking: TOKEN,
    });
    const publish = mockPublish();
    renderVersions();
    await openPublish();

    // The break is named, in the words the author has to act on.
    expect(await screen.findByTestId("publish-breaking")).toHaveTextContent(
      "column `n` of statement `main`",
    );

    await userEvent.click(screen.getByRole("button", { name: /^publish$/i }));
    expect(publish).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("checkbox"));
    await userEvent.click(screen.getByRole("button", { name: /^publish$/i }));
    await waitFor(() => expect(publish).toHaveBeenCalledTimes(1));
    // The server's own token, echoed. Nothing here computes one.
    expect(publish.mock.calls[0]?.[0] as PublishVars).toMatchObject({
      workspace: "ws1",
      id: "ep-1",
      data: { acknowledge_breaking: TOKEN },
    });
  });

  it("sends no acknowledgement when nothing breaks", async () => {
    mockValidate({
      statements: [],
      against_live: { breaking: [], safe: [{ kind: "statement_definition_changed", subject: "statement `main`", detail: "its SQL changed" }] },
      acknowledge_breaking: null,
    });
    const publish = mockPublish();
    renderVersions();
    await openPublish();

    expect(await screen.findByTestId("publish-preflight-clean")).toBeInTheDocument();
    // No box to tick: a checkbox an author meets on every publish is one they
    // stop reading.
    expect(screen.queryByRole("checkbox")).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: /^publish$/i }));
    await waitFor(() => expect(publish).toHaveBeenCalledTimes(1));
    // Sending one when the publish breaks nothing is a 409 by design, so it
    // must be absent rather than null or empty.
    expect((publish.mock.calls[0]?.[0] as PublishVars).data).not.toHaveProperty(
      "acknowledge_breaking",
    );
  });

  it("says so, and still lets the server decide, when the check itself fails", async () => {
    const mutateAsync = vi.fn(async () => {
      throw new Error("network down");
    });
    vi.mocked(gen.useDataApiValidateEndpoint).mockReturnValue({
      mutateAsync,
      isPending: false,
    } as unknown as ReturnType<typeof gen.useDataApiValidateEndpoint>);
    const publish = mockPublish();
    renderVersions();
    await openPublish();

    expect(await screen.findByText(/could not check what this changes/i)).toBeInTheDocument();
    // Enabled, deliberately: a client that can block a publish the server would
    // accept is a client deciding the gate, and the gate is not the client's.
    await userEvent.click(screen.getByRole("button", { name: /^publish$/i }));
    await waitFor(() => expect(publish).toHaveBeenCalledTimes(1));
  });

  it("distinguishes a first publish from a comparison that found nothing", async () => {
    mockValidate({ statements: [], against_live: null, acknowledge_breaking: null });
    mockPublish();
    vi.mocked(gen.useDataApiListEndpointVersions).mockReturnValue({
      data: { status: 200, data: [] },
      refetch: vi.fn(),
    } as unknown as ReturnType<typeof gen.useDataApiListEndpointVersions>);
    renderVersions();
    await openPublish();

    expect(await screen.findByTestId("publish-preflight-first")).toBeInTheDocument();
    expect(screen.queryByTestId("publish-preflight-clean")).toBeNull();
  });
});

describe("the version history's record of what was accepted", () => {
  it("marks only the versions that actually broke something", async () => {
    mockValidate({ statements: [], against_live: null, acknowledge_breaking: null });
    mockPublish();
    vi.mocked(gen.useDataApiListEndpointVersions).mockReturnValue({
      data: {
        status: 200,
        data: [
          // Published before the comparison existed, or the first version:
          // nobody looked. Not a claim that nothing broke.
          { ...LIVE_VERSION, id: "v-1", version: 1, is_live: false, breaking_changes: null },
          // Compared, and broke nothing. Also not a chip — a badge on every
          // safe publish is a badge nobody reads.
          { ...LIVE_VERSION, id: "v-2", version: 2, is_live: false, breaking_changes: [] },
          {
            ...LIVE_VERSION,
            id: "v-3",
            version: 3,
            is_live: true,
            breaking_changes: [REMOVED_COLUMN, { ...REMOVED_COLUMN, subject: "parameter `region`" }],
          },
        ],
      },
      refetch: vi.fn(),
    } as unknown as ReturnType<typeof gen.useDataApiListEndpointVersions>);
    renderVersions();

    expect(await screen.findByTestId("version-breaking-v-3")).toHaveTextContent(
      "2 breaking changes",
    );
    // The two absences are the point: `null` and `[]` are different facts and
    // neither of them is "this broke something".
    expect(screen.queryByTestId("version-breaking-v-1")).toBeNull();
    expect(screen.queryByTestId("version-breaking-v-2")).toBeNull();
  });

  it("names the baseline when a rollback moved it, and stays quiet when it did not", async () => {
    mockValidate({ statements: [], against_live: null, acknowledge_breaking: null });
    mockPublish();
    vi.mocked(gen.useDataApiListEndpointVersions).mockReturnValue({
      data: {
        status: 200,
        data: [
          // v3 was published while v1 was live, because someone rolled back.
          // Its `[]` is true of v1's consumers and says nothing about v2's, and
          // with no chip this row reads as "publishing this broke nobody".
          {
            ...LIVE_VERSION,
            id: "v-3",
            version: 3,
            is_live: true,
            breaking_changes: [],
            compared_against_version: 1,
          },
          // The ordinary case: the baseline IS the version before, so saying so
          // adds nothing and a chip on every row is a chip nobody reads.
          {
            ...LIVE_VERSION,
            id: "v-2",
            version: 2,
            is_live: false,
            breaking_changes: [],
            compared_against_version: 1,
          },
          // A first publish compared against nothing and names nothing.
          {
            ...LIVE_VERSION,
            id: "v-1",
            version: 1,
            is_live: false,
            breaking_changes: null,
            compared_against_version: null,
          },
        ],
      },
      refetch: vi.fn(),
    } as unknown as ReturnType<typeof gen.useDataApiListEndpointVersions>);
    renderVersions();

    expect(await screen.findByTestId("version-baseline-v-3")).toHaveTextContent(
      "compared against v1",
    );
    expect(screen.queryByTestId("version-baseline-v-2")).toBeNull();
    expect(screen.queryByTestId("version-baseline-v-1")).toBeNull();
  });
});
