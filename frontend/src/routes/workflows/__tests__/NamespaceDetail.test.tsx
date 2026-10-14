import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import WorkflowsRoutes from "@/routes/workflows/WorkflowsRoutes";
import { NS_WS, nsExecution, nsFlow, nsRow, serveNamespaces, type NamespaceWorld } from "./fakeNamespaceServer";

/**
 * The namespace detail through the area's real routes (W7d spec §4.2): the
 * frame is a lazy chunk, which vitest transforms cold on first import (see
 * `WorkflowsRoutes.test.tsx`), hence the longer first waits.
 */
const LAZY = { timeout: 4000 };

const ROWS = [
  nsRow("company.team.etl", { flows: 1 }),
  nsRow("companyx", { flows: 1 }),
  nsRow("company.team", { flows: 2, files: 5, kv_keys: 1 }),
  nsRow("company.team.alpha", { kv_keys: 3 }),
  nsRow("company.data", { flows: 1 }),
  nsRow("company", { own: false }),
];

const FLOWS = [
  nsFlow("companyx", "outside"),
  nsFlow("company.team.etl", "load"),
  nsFlow("company.team", "sync"),
  nsFlow("company.data", "beside"),
  nsFlow("company.team", "extract"),
];

/** Twelve executions under `company.team`, newest first, and one outside it. */
const EXECUTIONS = [
  ...Array.from({ length: 12 }, (_, i) =>
    nsExecution(`aaaaaaaa-0000-4000-8000-${String(i).padStart(12, "0")}`, i % 2 ? "company.team.etl" : "company.team", "sync"),
  ),
  nsExecution("bbbbbbbb-0000-4000-8000-000000000001", "companyx", "outside"),
];

function Where() {
  const l = useLocation();
  return <output data-testid="where">{`${l.pathname}${l.search}`}</output>;
}

function renderAt(path: string, world: Partial<NamespaceWorld> = {}) {
  const requests = serveNamespaces({ namespaces: ROWS, flows: FLOWS, executions: EXECUTIONS, ...world });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } } });
  render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[path]}>
          <Routes>
            <Route
              path="/explore/:workspace/workflows/*"
              element={
                <>
                  <WorkflowsRoutes />
                  <Where />
                </>
              }
            />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
  return requests;
}

const at = (ns: string, tab: string) => `/explore/${NS_WS}/workflows/namespaces/${ns}/${tab}`;
const where = () => screen.getByTestId("where").textContent;

afterEach(() => vi.unstubAllGlobals());

describe("NamespaceDetail: the frame", () => {
  it("has vertical tabs in Kestra's order: Overview, Flows, Executions, Dependencies, Files, KV", async () => {
    renderAt(at("company.team", "overview"));
    const list = await screen.findByRole("tablist", {}, LAZY);
    expect(list).toHaveAttribute("aria-orientation", "vertical");
    expect(within(list).getAllByRole("tab").map((t) => t.textContent)).toEqual([
      "Overview",
      "Flows",
      "Executions",
      "Dependencies",
      "Files",
      "KV",
    ]);
    expect(within(list).getByRole("tab", { name: "Overview" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("heading", { level: 1, name: "company.team" })).toBeInTheDocument();
  });

  it("puts the tab in the URL, and the Back button walks it", async () => {
    const user = userEvent.setup();
    renderAt(at("company.team", "overview"));
    const list = await screen.findByRole("tablist", {}, LAZY);
    await user.click(within(list).getByRole("tab", { name: "Flows" }));
    expect(where()).toBe(at("company.team", "flows"));
    expect(within(list).getByRole("tab", { name: "Flows" })).toHaveAttribute("aria-selected", "true");
    await user.click(within(list).getByRole("tab", { name: "KV" }));
    expect(where()).toBe(at("company.team", "kv"));
  });

  it("sends an unknown tab to Overview", async () => {
    renderAt(at("company.team", "secrets"));
    await waitFor(() => expect(where()).toBe(at("company.team", "overview")), LAZY);
  });

  it("sends a bare namespace to Overview", async () => {
    renderAt(`/explore/${NS_WS}/workflows/namespaces/company.team`);
    await waitFor(() => expect(where()).toBe(at("company.team", "overview")), LAZY);
  });

  it("links each breadcrumb segment to its namespace, and the first to the list", async () => {
    renderAt(at("company.team.etl", "flows"));
    const crumbs = await screen.findByRole("navigation", { name: "breadcrumb" }, LAZY);
    expect(within(crumbs).getByRole("link", { name: "Namespaces" })).toHaveAttribute("href", `/explore/${NS_WS}/workflows/namespaces`);
    expect(within(crumbs).getByRole("link", { name: "company" })).toHaveAttribute("href", at("company", "overview"));
    expect(within(crumbs).getByRole("link", { name: "team" })).toHaveAttribute("href", at("company.team", "overview"));
    // The last segment is the page itself, not a link.
    expect(within(crumbs).queryByRole("link", { name: "etl" })).toBeNull();
    expect(within(crumbs).getByText("etl")).toHaveAttribute("aria-current", "page");
  });

  it("shows an empty segment's label in the breadcrumb, and links to it encoded", async () => {
    const requests = renderAt(at("a..b", "overview"), {
      namespaces: [nsRow("a", { own: false }), nsRow("a.", { own: false }), nsRow("a..b", { flows: 1 })],
    });
    const crumbs = await screen.findByRole("navigation", { name: "breadcrumb" }, LAZY);
    expect(within(crumbs).getByRole("link", { name: "a. (empty segment)" })).toHaveAttribute("href", at("a.", "overview"));
    expect(requests.some((r) => r.path === `/api/ws/${NS_WS}/workflows/namespaces/a..b`)).toBe(true);
  });

  it("shows 'not found' with a link back to the list for an unknown namespace", async () => {
    renderAt(at("retired.team", "overview"));
    expect(await screen.findByRole("heading", { level: 1, name: "Namespace not found" }, LAZY)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to namespaces" })).toHaveAttribute("href", `/explore/${NS_WS}/workflows/namespaces`);
    expect(screen.queryByRole("tablist")).toBeNull();
  });

  it("shows the server's message and a retry when the namespace cannot be read", async () => {
    renderAt(at("company.team", "overview"), { detailReply: { status: 500, body: { error: "the database is unavailable" } } });
    expect(await screen.findByText(/the database is unavailable/, {}, LAZY)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Namespace not found" })).toBeNull();
  });
});

describe("NamespaceDetail: Overview", () => {
  it("shows the namespace's own counts", async () => {
    renderAt(at("company.team", "overview"));
    const counts = await screen.findByTestId("namespace-counts", {}, LAZY);
    // SummaryStrip draws the number and its word as siblings, with no space between them in the text.
    expect(counts).toHaveTextContent(/2\s*flows/);
    expect(counts).toHaveTextContent(/5\s*files/);
    expect(counts).toHaveTextContent(/1\s*KV key(?!s)/);
  });

  it("lists the 10 most recent executions of the namespace and its children", async () => {
    const requests = renderAt(at("company.team", "overview"));
    const recent = await screen.findByTestId("namespace-recent-executions", {}, LAZY);
    await waitFor(() => expect(within(recent).getAllByRole("row")).toHaveLength(11));
    const sent = requests.filter((r) => r.path === `/api/ws/${NS_WS}/workflows/executions`).map((r) => r.query);
    expect(sent).toHaveLength(1);
    expect(sent[0].get("namespace_prefix")).toBe("company.team");
    expect(sent[0].get("namespace")).toBeNull();
    expect(sent[0].get("limit")).toBe("10");
    expect(within(recent).getByRole("link", { name: "See all executions" })).toHaveAttribute("href", at("company.team", "executions"));
  });

  it("lists the direct child namespaces, each linked, and nothing deeper or beside", async () => {
    const requests = renderAt(at("company", "overview"));
    const children = await screen.findByTestId("namespace-children", {}, LAZY);
    await waitFor(() => expect(within(children).getAllByRole("link")).toHaveLength(2));
    expect(within(children).getAllByRole("link").map((l) => l.getAttribute("href"))).toEqual([
      at("company.data", "overview"),
      at("company.team", "overview"),
    ]);
    const list = requests.filter((r) => r.path === `/api/ws/${NS_WS}/workflows/namespaces`);
    expect(list[0].query.get("prefix")).toBe("company");
  });

  it("says when there are no executions and no child namespaces", async () => {
    renderAt(at("companyx", "overview"), { executions: [] });
    expect(await screen.findByText("No executions in this namespace yet", {}, LAZY)).toBeInTheDocument();
    expect(await screen.findByText("This namespace has no child namespaces.")).toBeInTheDocument();
  });
});

describe("NamespaceDetail: Flows and Executions, pinned to the namespace", () => {
  it("Flows lists the namespace's flows and its children's, without a namespace picker or a page title", async () => {
    renderAt(at("company.team", "flows"));
    expect(await screen.findByRole("link", { name: "sync" }, LAZY)).toBeInTheDocument();
    for (const name of ["extract", "load"]) expect(screen.getByRole("link", { name })).toBeInTheDocument();
    for (const name of ["outside", "beside"]) expect(screen.queryByRole("link", { name })).toBeNull();
    expect(screen.queryByLabelText("Namespace")).toBeNull();
    expect(screen.getByLabelText("State")).toBeInTheDocument();
    // The namespace page owns the one h1.
    expect(screen.getAllByRole("heading", { level: 1 }).map((h) => h.textContent)).toEqual(["company.team"]);
  });

  it("Flows says when the namespace has no flow of its own or below", async () => {
    renderAt(at("company.team.alpha", "flows"));
    expect(await screen.findByText("No flows in this namespace yet", {}, LAZY)).toBeInTheDocument();
  });

  it("Executions sends namespace_prefix, shows no namespace picker, and keeps the flow picker", async () => {
    const requests = renderAt(at("company.team", "executions"));
    await screen.findByTestId("executions-table", {}, LAZY);
    const sent = requests.filter((r) => r.path === `/api/ws/${NS_WS}/workflows/executions`).map((r) => r.query);
    expect(sent.length).toBeGreaterThan(0);
    for (const q of sent) {
      expect(q.get("namespace_prefix")).toBe("company.team");
      expect(q.get("namespace")).toBeNull();
    }
    expect(screen.queryByLabelText("Namespace")).toBeNull();
    expect(screen.getByLabelText("Flow")).toBeInTheDocument();
    expect(screen.getAllByRole("heading", { level: 1 }).map((h) => h.textContent)).toEqual(["company.team"]);
  });

  it("Executions ignores a namespace left in the URL", async () => {
    const requests = renderAt(`${at("company.team", "executions")}?namespace=companyx&subns=1`);
    await screen.findByTestId("executions-table", {}, LAZY);
    const sent = requests.filter((r) => r.path === `/api/ws/${NS_WS}/workflows/executions`).map((r) => r.query);
    expect(sent.every((q) => q.get("namespace_prefix") === "company.team" && q.get("namespace") === null)).toBe(true);
  });

  it("Executions offers only the namespace's flows in its flow picker", async () => {
    const user = userEvent.setup();
    renderAt(at("company.team", "executions"));
    await screen.findByTestId("executions-table", {}, LAZY);
    await user.click(screen.getByLabelText("Flow"));
    const options = (await screen.findAllByRole("option")).map((o) => o.textContent);
    expect(options).toEqual(["All", "company.team.etl.load", "company.team.extract", "company.team.sync"]);
  });
});

describe("NamespaceDetail: the slots later tasks fill", () => {
  it.each([
    ["dependencies", "Dependencies"],
    ["files", "Files"],
  ])("renders the %s tab's slot", async (tab, label) => {
    renderAt(at("company.team", tab));
    const list = await screen.findByRole("tablist", {}, LAZY);
    expect(within(list).getByRole("tab", { name: label })).toHaveAttribute("aria-selected", "true");
    expect(await screen.findByTestId(`namespace-tab-${tab}`, {}, LAZY)).toBeInTheDocument();
  });
});

describe("NamespaceDetail: the KV tab", () => {
  it("gets the viewer's workflow:manage from the namespace detail: writes on", async () => {
    renderAt(at("company.team", "kv"), { canManage: true });
    expect(await screen.findByText("No KV keys yet", {}, LAZY)).toBeInTheDocument();
    for (const add of screen.getAllByRole("button", { name: "Add a key" })) expect(add).toBeEnabled();
  });

  it("gets the viewer's workflow:manage from the namespace detail: writes off, with the reason", async () => {
    renderAt(at("company.team", "kv"), { canManage: false });
    expect(await screen.findByText("No KV keys yet", {}, LAZY)).toBeInTheDocument();
    for (const add of screen.getAllByRole("button", { name: "Add a key" })) {
      expect(add).toBeDisabled();
      expect(add).toHaveAccessibleDescription("Changing KV keys needs the workflow:manage permission");
    }
  });
});
