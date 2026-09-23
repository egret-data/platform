import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  MemoryRouter,
  Route,
  Routes,
  useSearchParams,
} from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import ControlPlaneEngines from "@/routes/control-plane/ControlPlaneEngines";
import * as api from "@/api/controlPlane";

/** Stub for the create route: echoes the `?type=` so navigation can be asserted. */
function CreateStub() {
  const [sp] = useSearchParams();
  return <div>CREATE:{sp.get("type")}</div>;
}

vi.mock("@/api/controlPlane");

const ENGINE: api.Engine = {
  id: "e1",
  name: "analytics-trino",
  type: "trino",
  namespace: "engines",
  warehouseId: "w1",
  status: "running",
  spec: {
    coordinatorReplicas: 1,
    workerReplicas: 2,
    cpuMin: "1",
    cpuMax: "2",
    memory: "4Gi",
    imageVersion: "latest",
  },
};

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <ControlPlaneEngines />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("ControlPlaneEngines", () => {
  beforeEach(() => {
    vi.mocked(api.listEngines).mockResolvedValue([ENGINE]);
    // `getEngine`'s return type intersects Engine (status: string) with a live
    // EngineStatus, so a plain literal can't satisfy it — cast for the mock.
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      status: { phase: "running", message: null, pods: [] },
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);
    vi.mocked(api.listWarehouses).mockResolvedValue([]);
  });

  test("renders the engine list", async () => {
    renderPage();
    expect(screen.getByText("Engines")).toBeInTheDocument();
    expect(await screen.findByText("analytics-trino")).toBeInTheDocument();
  });

  test("shows a live status badge", async () => {
    renderPage();
    // "running" also appears as a SummaryStrip label, so scope to the engine row.
    const row = (await screen.findByText("analytics-trino")).closest("tr")!;
    expect(within(row).getByText("running")).toBeInTheDocument();
  });

  /**
   * Whole-branch review, F6. `EnginesTab`'s `EngineHealthBadge` claims to keep
   * this page's variant mapping in step so the two never paint the same word
   * two colours; `phaseVariant` had no `deleting` arm and fell through to
   * `secondary` while the other page painted it `warning`. `deleting` is one of
   * the four `engine_status` values, and this badge falls back to that stored
   * column whenever the live poll has not answered.
   */
  test("a deleting engine is painted like the other in-flight phases, not neutral", async () => {
    vi.mocked(api.listEngines).mockResolvedValue([{ ...ENGINE, status: "deleting" }]);
    // The live poll never answers, so the badge shows the stored value — the
    // exact path the mapping had to cover.
    vi.mocked(api.getEngine).mockRejectedValue(new Error("no live status in this test"));
    renderPage();

    const row = (await screen.findByText("analytics-trino")).closest("tr")!;
    const deleting = within(row).getByText("deleting");
    // Compared against `provisioning`, the arm it shares, rather than against a
    // literal class string: the point is that the two agree, not what the
    // palette happens to be.
    vi.mocked(api.listEngines).mockResolvedValue([{ ...ENGINE, status: "provisioning" }]);
    cleanup();
    renderPage();
    const row2 = (await screen.findByText("analytics-trino")).closest("tr")!;
    expect(deleting.className).toBe(within(row2).getByText("provisioning").className);
  });

  test("Add engine navigates to the create page with the active engine type", async () => {
    const user = userEvent.setup();
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={["/"]}>
          <Routes>
            <Route path="/" element={<ControlPlaneEngines />} />
            <Route path="/control-plane/engines/new" element={<CreateStub />} />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>,
    );
    await screen.findByText("analytics-trino");

    // StarRocks tab → Add engine carries ?type=starrocks to the create page.
    await user.click(screen.getByRole("tab", { name: "StarRocks" }));
    await user.click(screen.getByRole("button", { name: /Add engine/i }));
    expect(await screen.findByText("CREATE:starrocks")).toBeInTheDocument();
  });
});
