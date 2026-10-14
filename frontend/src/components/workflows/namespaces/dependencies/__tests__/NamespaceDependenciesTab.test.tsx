import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { NamespaceDependenciesDto } from "@/api/generated/model";
import NamespaceDependenciesTab from "@/components/workflows/namespaces/NamespaceDependenciesTab";
import { installXyflowJsdom } from "@/components/workflows/topology/__tests__/xyflowJsdom";
import { DEPS, chainOf } from "./fixtures";

const WS = "acme";
const NS = "company.team";

type Reply = { status: number; body: unknown };

function serve(reply: Reply) {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string) => {
      calls.push(String(input));
      return new Response(JSON.stringify(reply.body), { status: reply.status, headers: { "content-type": "application/json" } });
    }),
  );
  return calls;
}

function Where() {
  const l = useLocation();
  return <output data-testid="where">{l.pathname}</output>;
}

function renderTab(reply: Reply) {
  const calls = serve(reply);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } } });
  const detail = { namespace: NS, flows: 3, files: 0, kv_keys: 0, own: true, can_manage: true };
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[`/explore/${WS}/workflows/namespaces/${NS}/dependencies`]}>
        <Routes>
          <Route
            path="/explore/:workspace/workflows/namespaces/:namespace/:tab"
            element={
              <div style={{ width: 800, height: 600 }}>
                <NamespaceDependenciesTab ws={WS} namespace={NS} detail={detail} canManage />
              </div>
            }
          />
          <Route path="/explore/:workspace/workflows/flows/:id/edit" element={<p>The flow page</p>} />
        </Routes>
        <Where />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return calls;
}

const ok = (body: NamespaceDependenciesDto): Reply => ({ status: 200, body });

/** A flow's card: the link carrying its id. */
async function card(id: string): Promise<HTMLElement> {
  const canvas = await screen.findByRole("application", { name: "Dependencies of the namespace" }, { timeout: 4000 });
  const found = within(canvas)
    .queryAllByRole("link")
    .find((a) => a.getAttribute("data-flow") === id);
  if (!found) throw new Error(`no card for ${id}`);
  return found;
}

/** Lets xyflow's measuring and its queued view run. */
const settle = () => act(() => new Promise((r) => setTimeout(r, 20)));

beforeAll(installXyflowJsdom);
afterEach(() => vi.unstubAllGlobals());

describe("NamespaceDependenciesTab: the graph", () => {
  it("asks for the namespace's dependencies, the namespace encoded", async () => {
    const calls = renderTab(ok(DEPS));
    await card("f1");
    expect(calls).toContain(`/api/ws/${WS}/workflows/namespaces/company.team/dependencies`);
  });

  it("draws each flow as a card with its id and namespace", async () => {
    renderTab(ok(DEPS));
    const extract = await card("f1");
    expect(within(extract).getByText("extract")).toBeInTheDocument();
    expect(within(extract).getByText("company.team")).toBeInTheDocument();
    expect(within(await card("f2")).getByText("company.team.etl")).toBeInTheDocument();
  });

  it("draws subflow edges solid and flow_trigger edges dashed, and says which is which", async () => {
    renderTab(ok(DEPS));
    await card("f1");
    const path = (s: string, t: string) => document.querySelector(`path[data-edge="${s} -> ${t}"]`) as SVGPathElement | null;
    expect(path("f 5", "f1")?.style.strokeDasharray).toBe("");
    expect(path("f3", "f2")?.style.strokeDasharray).toBe("");
    expect(path("f1", "f2")?.style.strokeDasharray).toBe("6 4");
    const legend = screen.getByRole("list", { name: "Edges" });
    expect(within(legend).getByText("Subflow: the flow calls the next")).toBeInTheDocument();
    expect(within(legend).getByText("Flow trigger: the flow's end starts the next")).toBeInTheDocument();
  });

  it("mutes a flow outside the namespace and says so in words", async () => {
    renderTab(ok(DEPS));
    const caller = await card("f 5");
    expect(caller.closest("[data-flow-card]")).toHaveAttribute("data-outside", "true");
    expect(within(caller).getByText("Outside this namespace")).toBeInTheDocument();
    // A child namespace's flow is inside.
    expect((await card("f2")).closest("[data-flow-card]")).not.toHaveAttribute("data-outside");
  });

  it("shows a flow's dynamic subflows, disabled state and unparsable draft", async () => {
    renderTab(ok(DEPS));
    const sync = await card("f3");
    expect(within(sync).getByText("2 dynamic subflows")).toBeInTheDocument();
    expect(within(sync).getByText("Disabled")).toBeInTheDocument();
    expect(within(await card("f2")).getByText("1 dynamic subflow")).toBeInTheDocument();
    expect(within(await card("f4")).getByText("Unparsable draft")).toBeInTheDocument();
  });

  it("says how many drafts in the workspace do not parse", async () => {
    renderTab(ok(DEPS));
    await card("f1");
    expect(screen.getByText(/3 flows in this workspace have a draft that does not parse/)).toBeInTheDocument();
  });

  it("says nothing about unparsable drafts when there are none", async () => {
    renderTab(ok({ ...DEPS, unparsable: 0 }));
    await card("f1");
    expect(screen.queryByText(/does not parse/)).toBeNull();
  });

  it("selecting a flow opens it, its id encoded", async () => {
    const user = userEvent.setup();
    renderTab(ok(DEPS));
    await user.click(await card("f 5"));
    expect(await screen.findByText("The flow page")).toBeInTheDocument();
    expect(screen.getByTestId("where").textContent).toBe(`/explore/${WS}/workflows/flows/f%205/edit`);
  });

  it("has one tab stop; the arrows walk the layout's order, Home and End go to the ends, Enter opens", async () => {
    const user = userEvent.setup();
    renderTab(ok(DEPS));
    await card("f1");
    await settle();
    const canvas = screen.getByRole("application", { name: "Dependencies of the namespace" });
    // Every card is a link with its flow's id; xyflow's attribution is a link too.
    const cards = () => within(canvas).getAllByRole("link").filter((a) => a.hasAttribute("data-flow"));
    const stops = cards().filter((a) => a.tabIndex === 0);
    expect(stops).toHaveLength(1);
    act(() => stops[0].focus());
    const seen = [document.activeElement?.getAttribute("data-flow")];
    for (let i = 0; i < 4; i++) {
      await user.keyboard("{ArrowRight}");
      seen.push(document.activeElement?.getAttribute("data-flow"));
    }
    expect(seen[0]).toBe("f 5");
    expect(new Set(seen.slice(1, 3))).toEqual(new Set(["f1", "f3"]));
    expect(seen.slice(3)).toEqual(["f2", "f4"]);
    // The focus moved the one tab stop with it.
    expect(cards().filter((a) => a.tabIndex === 0).map((a) => a.getAttribute("data-flow"))).toEqual(["f4"]);
    await user.keyboard("{Home}");
    expect(document.activeElement?.getAttribute("data-flow")).toBe("f 5");
    await user.keyboard("{End}{ArrowLeft}");
    expect(document.activeElement?.getAttribute("data-flow")).toBe("f2");
    await user.keyboard("{Enter}");
    expect(await screen.findByText("The flow page")).toBeInTheDocument();
    expect(screen.getByTestId("where").textContent).toBe(`/explore/${WS}/workflows/flows/f2/edit`);
  });

  it("zooms in, zooms out and fits", async () => {
    renderTab(ok(DEPS));
    await card("f1");
    for (const name of ["Zoom in", "Zoom out", "Fit view"]) expect(screen.getByRole("button", { name })).toBeInTheDocument();
  });
});

describe("NamespaceDependenciesTab: the list view", () => {
  it("lists every flow with what it calls or triggers and what calls or triggers it", async () => {
    const user = userEvent.setup();
    renderTab(ok(DEPS));
    await card("f1");
    await user.click(screen.getByRole("button", { name: "List view" }));
    expect(screen.getByRole("button", { name: "List view" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByRole("application")).toBeNull();
    const table = screen.getByRole("table", { name: "Flows of the namespace and their dependencies" });
    const rows = within(table).getAllByRole("row").slice(1);
    // The server's order: namespace, then flow id.
    expect(rows.map((r) => within(r).getAllByRole("cell")[0].textContent)).toEqual(["caller", "broken", "extract", "sync", "load"]);
    const extract = rows[2];
    const cells = within(extract).getAllByRole("cell");
    expect(cells[1].textContent).toBe("company.team");
    expect(cells[2].textContent).toBe("company.team.etl.load (Flow trigger)");
    expect(cells[3].textContent).toBe("company.other.caller (Subflow)");
    expect(within(rows[3]).getAllByRole("cell")[4].textContent).toContain("2 dynamic subflows");
    expect(within(rows[0]).getAllByRole("cell")[4].textContent).toContain("Outside this namespace");
  });

  it("opens a flow from its row's link, and goes back to the graph", async () => {
    const user = userEvent.setup();
    renderTab(ok(DEPS));
    await card("f1");
    await user.click(screen.getByRole("button", { name: "List view" }));
    await user.click(screen.getByRole("button", { name: "List view" }));
    expect(await card("f1")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "List view" }));
    await user.click(screen.getByRole("link", { name: "sync" }));
    expect(screen.getByTestId("where").textContent).toBe(`/explore/${WS}/workflows/flows/f3/edit`);
  });
});

describe("NamespaceDependenciesTab: states", () => {
  it("shows a skeleton while loading", () => {
    vi.stubGlobal("fetch", vi.fn(() => new Promise(() => {})));
    const client = new QueryClient();
    const detail = { namespace: NS, flows: 0, files: 0, kv_keys: 0, own: true, can_manage: true };
    render(
      <QueryClientProvider client={client}>
        <MemoryRouter>
          <NamespaceDependenciesTab ws={WS} namespace={NS} detail={detail} canManage />
        </MemoryRouter>
      </QueryClientProvider>,
    );
    expect(screen.getByTestId("namespace-tab-dependencies").querySelector("[data-slot='skeleton'], .animate-pulse")).not.toBeNull();
  });

  it("an empty namespace says so", async () => {
    renderTab(ok({ nodes: [], edges: [], unparsable: 0 }));
    expect(await screen.findByText("No flows in this namespace")).toBeInTheDocument();
    expect(screen.queryByRole("application")).toBeNull();
  });

  it("a workspace over the limit shows the server's message and what to do", async () => {
    const message = "this workspace has more than 2000 flows; the dependencies graph reads every flow's draft and is limited to 2000";
    renderTab({ status: 422, body: { error: message, code: "dependencies_too_large" } });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(message);
    expect(alert).toHaveTextContent("The Flows tab still lists this namespace's flows.");
    expect(within(alert).queryByRole("button", { name: "Retry" })).toBeNull();
    expect(within(alert).getByRole("link", { name: "Open the Flows tab" })).toHaveAttribute("href", `/explore/${WS}/workflows/namespaces/company.team/flows`);
  });

  it("any other failure says so and offers to try again", async () => {
    renderTab({ status: 500, body: { error: "database is down" } });
    // A 5xx is asked once more before it is shown.
    const alert = await screen.findByRole("alert", {}, { timeout: 4000 });
    expect(alert).toHaveTextContent("The dependencies could not be loaded: database is down");
    expect(within(alert).getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });

  it("draws 2,000 flows, and lists them", { timeout: 120_000 }, async () => {
    const user = userEvent.setup();
    renderTab(ok(chainOf(2000)));
    await screen.findByRole("application", { name: "Dependencies of the namespace" }, { timeout: 60_000 });
    expect(screen.queryByText("This graph could not be drawn.")).toBeNull();
    await user.click(screen.getByRole("button", { name: "List view" }));
    const table = screen.getByRole("table", { name: "Flows of the namespace and their dependencies" });
    expect(within(table).getAllByRole("row")).toHaveLength(2001);
  });
});
