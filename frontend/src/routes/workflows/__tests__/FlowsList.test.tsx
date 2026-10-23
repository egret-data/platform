import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { TooltipProvider } from "@/components/ui/tooltip";

// No msw in this repo: the page's data source is mocked at the module, as
// `Endpoints.test.tsx` does. `@/api/members` is mocked too, and only so the
// test can prove the page never calls it (spec §7.4).
vi.mock("@/api/generated/workflow/workflow");
vi.mock("@/api/members");

import * as api from "@/api/generated/workflow/workflow";
import * as members from "@/api/members";
import type { FlowListItemDto } from "@/api/generated/model";
import FlowsList, { nsCollapseKey } from "@/routes/workflows/FlowsList";
import { ApiError } from "@/api/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const ALICE = "11111111-1111-1111-1111-111111111111";

function flow(namespace: string, flow_id: string, over: Partial<FlowListItemDto> = {}): FlowListItemDto {
  return {
    id: `id-${namespace}-${flow_id}`,
    namespace,
    flow_id,
    state: "draft",
    created_by: ALICE,
    created_by_name: "Alice Nguyen",
    created_at: "2026-10-01T00:00:00Z",
    updated_at: "2026-10-02T00:00:00Z",
    ...over,
  };
}

/**
 * UNSORTED on purpose (FE-TEST-1): a child namespace arrives before its
 * parent, flows within a namespace arrive in reverse, and `companyx` shares
 * `company`'s prefix without being inside it.
 */
const FLOWS: FlowListItemDto[] = [
  flow("company.team", "sync", { state: "live" }),
  flow("companyx", "other", { state: "disabled" }),
  flow("company", "root-b"),
  flow("company.team", "etl"),
  flow("company", "root-a", { state: "live" }),
];

function mockList(over: Partial<{ data: unknown; isLoading: boolean; isError: boolean; error: unknown }> = {}) {
  vi.mocked(api.useWorkflowListFlows).mockReturnValue({
    data: undefined,
    isLoading: false,
    isError: false,
    error: null,
    ...over,
  } as unknown as ReturnType<typeof api.useWorkflowListFlows>);
}

function mockFlows(flows: FlowListItemDto[]) {
  mockList({ data: { status: 200, data: flows } });
}

/** Prints the router's current search string, so a test can read the URL. */
function LocationProbe() {
  const loc = useLocation();
  return <output data-testid="search">{loc.search}</output>;
}

function renderAt(search = "") {
  // The refusal state names a workspace to open, from `/me/workspaces`.
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false, enabled: false } } });
  return render(
    <QueryClientProvider client={qc}>
    <TooltipProvider>
      <MemoryRouter initialEntries={[`/explore/eng/workflows/flows${search}`]}>
        <Routes>
          <Route
            path="/explore/:workspace/workflows/flows"
            element={
              <>
                <FlowsList />
                <LocationProbe />
              </>
            }
          />
        </Routes>
      </MemoryRouter>
    </TooltipProvider>
    </QueryClientProvider>,
  );
}

function searchParams(): URLSearchParams {
  return new URLSearchParams(screen.getByTestId("search").textContent ?? "");
}

/** The flow ids on screen, top to bottom, as their links read. */
function flowLinks(): string[] {
  return within(screen.getByRole("table"))
    .queryAllByRole("link")
    .map((a) => a.textContent ?? "");
}

/** The namespace header rows on screen, top to bottom. */
function headers(): string[] {
  return screen.queryAllByRole("rowheader").map((h) => within(h).getByRole("button").getAttribute("aria-label") ?? "");
}

beforeEach(() => {
  localStorage.clear();
});

afterEach(() => vi.clearAllMocks());

describe("FlowsList — the three states", () => {
  it("shows a skeleton while loading", () => {
    mockList({ isLoading: true });
    renderAt();
    expect(screen.getByTestId("list-skeleton")).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  // KI-231: workflow:read missing is the shared refusal, not an error alert.
  it("renders the shared refusal when the server refuses the list", () => {
    mockList({ isError: true, error: new ApiError(403, "forbidden") });
    renderAt();
    expect(screen.getByRole("heading", { level: 1, name: "You don't have access to this page." })).toBeInTheDocument();
    expect(screen.getByText("Workflows need the workflow:read permission in this workspace.")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows the error, never the empty state, when the list fails", () => {
    mockList({ isError: true, error: new Error("upstream said no") });
    renderAt();
    expect(screen.getByRole("alert")).toHaveTextContent("upstream said no");
    expect(screen.queryByText("No flows yet")).not.toBeInTheDocument();
  });

  it("falls back to its own words when the error carries none", () => {
    mockList({ isError: true, error: new Error("") });
    renderAt();
    expect(screen.getByRole("alert")).toHaveTextContent(/flows could not be loaded/i);
  });

  it("shows EmptyState with New flow for an empty workspace", () => {
    mockFlows([]);
    renderAt();
    const empty = screen.getByText("No flows yet").parentElement as HTMLElement;
    expect(within(empty).getByRole("link", { name: "New flow" })).toHaveAttribute(
      "href",
      "/explore/eng/workflows/flows/new",
    );
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("says a filter matched nothing, and clears it on request, instead of the empty state", async () => {
    const user = userEvent.setup();
    mockFlows(FLOWS);
    renderAt("?q=nothing-is-called-this");
    expect(screen.getByText("No flows match the current filters.")).toBeInTheDocument();
    expect(screen.queryByText("No flows yet")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Clear filters" }));
    expect(searchParams().has("q")).toBe(false);
    expect(flowLinks()).toHaveLength(FLOWS.length);
  });

  it("puts New flow in the header, as a link to the create page", () => {
    mockFlows(FLOWS);
    renderAt();
    expect(screen.getByRole("heading", { level: 1, name: "Flows" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "New flow" })).toHaveAttribute("href", "/explore/eng/workflows/flows/new");
  });
});

describe("FlowsList — the namespace tree", () => {
  it("groups rows under the tree: a namespace's own flows, then its children, siblings sorted", () => {
    mockFlows(FLOWS);
    renderAt();
    expect(headers()).toEqual(["company (4)", "company.team (2)", "companyx (1)"]);
    expect(flowLinks()).toEqual(["root-a", "root-b", "etl", "sync", "other"]);
  });

  it("links each flow to its editor", () => {
    mockFlows(FLOWS);
    renderAt();
    expect(screen.getByRole("link", { name: "etl" })).toHaveAttribute(
      "href",
      "/explore/eng/workflows/flows/id-company.team-etl/edit",
    );
  });

  it("shows each flow's state chip, its creator by the server's name, and never its folder", () => {
    mockFlows([flow("a", "one", { state: "live", folder_id: "folder-secret" })]);
    renderAt();
    const row = screen.getByRole("link", { name: "one" }).closest("tr") as HTMLElement;
    // The chip says only whether the flow runs (the user's ruling, 2026-10-10).
    expect(within(row).getByText("Active")).toBeInTheDocument();
    expect(within(row).getByText("Alice Nguyen")).toBeInTheDocument();
    expect(screen.queryByText(/folder-secret/)).not.toBeInTheDocument();
    // The name comes with the row; workspace members are never fetched for it.
    expect(members.useWorkspaceMembers).not.toHaveBeenCalled();
  });

  it("shows the latest version as v{n} in its own column, and a dash with none", () => {
    mockFlows([flow("a", "one", { latest_version: 12 }), flow("a", "two", { latest_version: null })]);
    renderAt();
    expect(screen.getByRole("columnheader", { name: "Version" })).toBeInTheDocument();
    const cells = (name: string) => [...(screen.getByRole("link", { name }).closest("tr") as HTMLElement).querySelectorAll("td")];
    const versionColumn = screen.getAllByRole("columnheader").findIndex((h) => h.textContent === "Version");
    expect(cells("one")[versionColumn]).toHaveTextContent(/^v12$/);
    expect(cells("two")[versionColumn]).toHaveTextContent(/^—$/);
  });

  it("shows a dash, not an id, when the server has no name for the creator", () => {
    mockFlows([
      flow("a", "nobody", { created_by: null, created_by_name: null }),
      flow("a", "gone", { created_by: ALICE, created_by_name: null }),
    ]);
    renderAt();
    for (const id of ["nobody", "gone"]) {
      const row = screen.getByRole("link", { name: id }).closest("tr") as HTMLElement;
      expect(within(row).getAllByRole("cell").at(-1)).toHaveTextContent(/^—$/);
    }
    expect(screen.queryByText(ALICE.slice(0, 8))).not.toBeInTheDocument();
  });

  it("names an empty namespace segment instead of leaving a bare chevron", () => {
    mockFlows([flow("a..b", "deep"), flow("a.", "trailing")]);
    renderAt();
    // `a.` is the node for the empty segment between the dots, and the parent of `a..b`.
    const toggle = screen.getByRole("button", { name: "a. (2)" });
    expect(toggle).toHaveTextContent("(empty segment)");
    expect(within(toggle).getByText("(empty segment)")).toHaveClass("text-muted-foreground");
    expect(headers()).toEqual(["a (2)", "a. (2)", "a..b (1)"]);
  });

  it("collapsing a namespace hides its own flows AND everything beneath it", async () => {
    const user = userEvent.setup();
    mockFlows(FLOWS);
    renderAt();
    const toggle = screen.getByRole("button", { name: "company (4)" });
    expect(toggle).toHaveAttribute("aria-expanded", "true");

    await user.click(toggle);

    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(headers()).toEqual(["company (4)", "companyx (1)"]);
    expect(flowLinks()).toEqual(["other"]);
  });

  it("toggles from the keyboard", async () => {
    const user = userEvent.setup();
    mockFlows(FLOWS);
    renderAt();
    const toggle = screen.getByRole("button", { name: "company.team (2)" });
    toggle.focus();
    await user.keyboard("{Enter}");
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(flowLinks()).toEqual(["root-a", "root-b", "other"]);
    await user.keyboard(" ");
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(flowLinks()).toEqual(["root-a", "root-b", "etl", "sync", "other"]);
  });

  it("remembers what the viewer collapsed, per workspace, across a remount", async () => {
    const user = userEvent.setup();
    mockFlows(FLOWS);
    const first = renderAt();
    await user.click(screen.getByRole("button", { name: "company.team (2)" }));
    first.unmount();

    renderAt();
    expect(screen.getByRole("button", { name: "company.team (2)" })).toHaveAttribute("aria-expanded", "false");
    // The key the page wrote is the key the page exports, under this workspace.
    expect(JSON.parse(localStorage.getItem(nsCollapseKey("eng")) ?? "[]")).toEqual(["company.team"]);
  });

  it("still renders when localStorage throws", () => {
    const spy = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    mockFlows(FLOWS);
    renderAt();
    expect(flowLinks()).toHaveLength(FLOWS.length);
    spy.mockRestore();
  });
});

describe("FlowsList — collapse while filtering", () => {
  it("opens every namespace while a filter is on, and restores the viewer's collapse when it clears", async () => {
    const user = userEvent.setup();
    mockFlows(FLOWS);
    renderAt();
    await user.click(screen.getByRole("button", { name: "company.team (2)" }));
    expect(flowLinks()).not.toContain("etl");

    // A search that hits a flow inside the collapsed namespace shows it.
    await user.type(screen.getByLabelText("Search flows"), "etl");
    expect(flowLinks()).toEqual(["etl"]);
    expect(screen.getByRole("button", { name: "company.team (1)" })).toHaveAttribute("aria-expanded", "true");

    // Clearing the search brings the stored collapse back, untouched.
    await user.clear(screen.getByLabelText("Search flows"));
    expect(screen.getByRole("button", { name: "company.team (2)" })).toHaveAttribute("aria-expanded", "false");
    expect(flowLinks()).toEqual(["root-a", "root-b", "other"]);
    expect(JSON.parse(localStorage.getItem(nsCollapseKey("eng")) ?? "[]")).toEqual(["company.team"]);
  });

  it("folds a branch of the results without writing the viewer's stored collapse", async () => {
    const user = userEvent.setup();
    mockFlows(FLOWS);
    renderAt("?namespace=company");
    await user.click(screen.getByRole("button", { name: "company.team (2)" }));
    expect(flowLinks()).toEqual(["root-a", "root-b"]);
    expect(localStorage.getItem(nsCollapseKey("eng"))).toBeNull();
  });
});

describe("FlowsList — filters live in the URL", () => {
  it("writes the search to the URL as it is typed", async () => {
    const user = userEvent.setup();
    mockFlows(FLOWS);
    renderAt();
    await user.type(screen.getByLabelText("Search flows"), "et");
    expect(searchParams().get("q")).toBe("et");
    // `etl` by its id; nothing else contains "et".
    expect(flowLinks()).toEqual(["etl"]);
  });

  it("searches the namespace too, not only the flow id", async () => {
    const user = userEvent.setup();
    mockFlows(FLOWS);
    renderAt();
    await user.type(screen.getByLabelText("Search flows"), "companyx");
    expect(flowLinks()).toEqual(["other"]);
  });

  it("writes the namespace and state pickers to the URL, and a reload reads all three back", async () => {
    const user = userEvent.setup();
    mockFlows(FLOWS);
    const first = renderAt();

    await user.click(screen.getByLabelText("Namespace"));
    await user.click(await screen.findByRole("option", { name: "company" }));
    await user.click(screen.getByLabelText("State"));
    await user.click(await screen.findByRole("option", { name: "Active" }));
    await user.type(screen.getByLabelText("Search flows"), "s");

    const written = searchParams();
    expect(written.get("namespace")).toBe("company");
    expect(written.get("state")).toBe("active");
    expect(written.get("q")).toBe("s");
    first.unmount();

    // The reload: a fresh mount from nothing but the URL that was written.
    renderAt(`?${written.toString()}`);
    expect(screen.getByLabelText("Search flows")).toHaveValue("s");
    expect(screen.getByLabelText("Namespace")).toHaveTextContent("company");
    expect(screen.getByLabelText("State")).toHaveTextContent("Active");
    // Active, inside company's subtree, matching "s": only `sync`.
    expect(flowLinks()).toEqual(["sync"]);
  });

  it("filters a namespace to its whole subtree and not to a sibling sharing its prefix", () => {
    mockFlows(FLOWS);
    renderAt("?namespace=company");
    expect(flowLinks()).toEqual(["root-a", "root-b", "etl", "sync"]);
  });

  it("offers every ancestor namespace in the picker, not only the namespaces flows live in", async () => {
    const user = userEvent.setup();
    mockFlows([flow("x.y.z", "deep"), flow("b", "flat")]);
    renderAt();
    await user.click(screen.getByLabelText("Namespace"));
    const options = (await screen.findAllByRole("option")).map((o) => o.textContent);
    expect(options).toEqual(["All", "b", "x", "x.y", "x.y.z"]);
    // Choosing an ancestor selects its subtree.
    await user.click(screen.getByRole("option", { name: "x" }));
    expect(searchParams().get("namespace")).toBe("x");
    expect(flowLinks()).toEqual(["deep"]);
  });

  it("keeps a stale ?namespace= selectable and says the filter matches nothing", async () => {
    const user = userEvent.setup();
    mockFlows(FLOWS);
    renderAt("?namespace=retired.team");
    expect(screen.getByLabelText("Namespace")).toHaveTextContent("retired.team");
    expect(screen.getByText("No flows match the current filters.")).toBeInTheDocument();
    await user.click(screen.getByLabelText("Namespace"));
    expect(await screen.findByRole("option", { name: "retired.team" })).toBeInTheDocument();
  });

  it("filters by whether a flow runs: disabled alone, or draft and live together as active", () => {
    mockFlows(FLOWS);
    const first = renderAt("?state=disabled");
    expect(flowLinks()).toEqual(["other"]);
    first.unmount();
    renderAt("?state=active");
    expect(flowLinks()).toEqual(["root-a", "root-b", "etl", "sync"]);
  });

  it("counts what the filters leave, in total and by state", () => {
    mockFlows(FLOWS);
    renderAt("?namespace=company");
    const strip = screen.getByTestId("flows-summary");
    expect(strip).toHaveTextContent("4flows");
    expect(strip).toHaveTextContent("4active");
    expect(strip).toHaveTextContent("0disabled");
    expect(strip).not.toHaveTextContent(/live|draft/);
  });
});
