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
import { AuthContext } from "@/auth/useAuth";

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
    expect(within(row).getByText("Running")).toBeInTheDocument();
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
    const deleting = within(row).getByText("Deleting");
    // Compared against `provisioning`, the arm it shares, rather than against a
    // literal class string: the point is that the two agree, not what the
    // palette happens to be.
    vi.mocked(api.listEngines).mockResolvedValue([{ ...ENGINE, status: "provisioning" }]);
    cleanup();
    renderPage();
    const row2 = (await screen.findByText("analytics-trino")).closest("tr")!;
    expect(deleting.className).toBe(within(row2).getByText("Provisioning").className);
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

  describe("external engines", () => {
    const EXTERNAL: api.Engine = {
      ...ENGINE,
      id: "x1",
      name: "legacy-trino",
      namespace: "",
      origin: "external",
      host: "trino.corp.example",
      port: 8080,
      rangerPluginVersion: "2.5.0",
      rangerPluginVersionModelled: true,
    };

    function renderAs(perms: string[]) {
      const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
      return render(
        <AuthContext.Provider
          value={{ me: { user_id: "u", email: "u@x", platform_roles: [], permissions: perms }, isLoading: false }}
        >
          <QueryClientProvider client={qc}>
            <MemoryRouter>
              <ControlPlaneEngines />
            </MemoryRouter>
          </QueryClientProvider>
        </AuthContext.Provider>,
      );
    }

    test("an external engine shows the External chip and its address, and is not polled", async () => {
      vi.mocked(api.listEngines).mockResolvedValue([ENGINE, EXTERNAL]);
      renderAs(["engine:read"]);
      const row = (await screen.findByText("legacy-trino")).closest("tr")!;
      expect(within(row).getByText("External")).toBeInTheDocument();
      expect(within(row).getByText("trino.corp.example:8080")).toBeInTheDocument();
      expect(within(row).queryByText("Running")).not.toBeInTheDocument();
      expect(vi.mocked(api.getEngine)).not.toHaveBeenCalledWith("x1");
    });

    test("the summary does not count an external engine as running", async () => {
      vi.mocked(api.listEngines).mockResolvedValue([ENGINE, EXTERNAL]);
      renderAs(["engine:read"]);
      await screen.findByText("legacy-trino");
      // Both rows store status "running"; only the provisioned one means it.
      const figure = (text: string) =>
        screen.getByText((_, el) => el?.tagName === "SPAN" && el.className.includes("inline-flex") && el.textContent?.replace("·", "") === text);
      expect(figure("1running")).toBeInTheDocument();
      expect(figure("0not running")).toBeInTheDocument();
      expect(figure("1external")).toBeInTheDocument();
    });

    test("Register external engine links to the register page only for engine:create", async () => {
      renderAs(["engine:read", "engine:create"]);
      await screen.findByText("analytics-trino");
      expect(screen.getByRole("link", { name: "Register external engine" })).toHaveAttribute(
        "href",
        "/control-plane/engines/register",
      );
      cleanup();
      renderAs(["engine:read"]);
      await screen.findByText("analytics-trino");
      expect(screen.queryByRole("link", { name: "Register external engine" })).not.toBeInTheDocument();
    });
  });
});
