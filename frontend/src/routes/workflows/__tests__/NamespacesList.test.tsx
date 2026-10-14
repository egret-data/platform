import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import NamespacesList from "@/routes/workflows/NamespacesList";
import { NS_WS, nsRow, serveNamespaces } from "./fakeNamespaceServer";

/**
 * UNSORTED on purpose (FE-TEST-1): a child before its parent, siblings in
 * reverse, and `companyx`, which starts with `company` without being inside
 * it. `company` and `company.team` are parents only.
 */
const ROWS = [
  nsRow("company.team.etl", { flows: 2, files: 5, kv_keys: 1 }),
  nsRow("companyx", { flows: 1 }),
  nsRow("company.team", { own: false }),
  nsRow("company.data", { flows: 0, files: 0, kv_keys: 7 }),
  nsRow("company", { own: false }),
];

function Where() {
  const l = useLocation();
  return <output data-testid="where">{`${l.pathname}${l.search}`}</output>;
}

function renderList(search = "") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[`/explore/${NS_WS}/workflows/namespaces${search}`]}>
          <Routes>
            <Route
              path="/explore/:workspace/workflows/namespaces"
              element={
                <>
                  <NamespacesList />
                  <Where />
                </>
              }
            />
            <Route path="*" element={<Where />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

const table = () => screen.getByTestId("namespaces-table");
const bodyRows = () => within(table()).getAllByRole("row").slice(1);
/** A row's cells as text: name cell first, then the counts. */
const rowText = (row: HTMLElement) => within(row).getAllByRole("gridcell").map((c) => c.textContent);

afterEach(() => vi.unstubAllGlobals());

describe("NamespacesList: the tree", () => {
  it("draws the flat list as a tree by dots, in order, with each node's own counts", async () => {
    serveNamespaces({ namespaces: ROWS });
    renderList();
    expect(screen.getByRole("heading", { level: 1, name: "Namespaces" })).toBeInTheDocument();
    await screen.findByTestId("namespaces-table");
    const headers = within(table()).getAllByRole("columnheader").map((h) => h.textContent);
    expect(headers).toEqual(["Namespace", "Flows", "Files", "KV keys"]);
    const links = bodyRows().map((r) => within(r).getByRole("link").getAttribute("aria-label"));
    expect(links).toEqual(["company", "company.data", "company.team", "company.team.etl", "companyx"]);
    // Each node shows its last segment, and its own counts, never rolled up.
    const etl = bodyRows()[3];
    expect(within(etl).getByRole("link")).toHaveTextContent(/^etl$/);
    expect(rowText(etl).slice(1)).toEqual(["2", "5", "1"]);
    expect(rowText(bodyRows()[1]).slice(1)).toEqual(["0", "0", "7"]);
  });

  it("says the counts are each namespace's own, as the table's caption", async () => {
    serveNamespaces({ namespaces: ROWS });
    renderList();
    await screen.findByTestId("namespaces-table");
    const grid = screen.getByRole("treegrid", { name: "Namespaces" });
    expect(grid).toBe(table());
    expect(grid).toHaveAccessibleDescription(/^Counted in this namespace only; each child namespace counts its own\./);
  });

  it("indents a child deeper than its parent", async () => {
    serveNamespaces({ namespaces: ROWS });
    renderList();
    await screen.findByTestId("namespaces-table");
    const depth = (i: number) => Number(bodyRows()[i].getAttribute("data-depth"));
    expect([0, 1, 2, 3, 4].map(depth)).toEqual([0, 1, 1, 2, 0]);
  });

  it("mutes a parent-only node and says why, in words as well as colour", async () => {
    serveNamespaces({ namespaces: ROWS });
    renderList();
    await screen.findByTestId("namespaces-table");
    const [company, data] = bodyRows();
    expect(within(company).getByText("Parent only")).toBeInTheDocument();
    expect(within(company).getByRole("link")).toHaveClass("text-muted-foreground");
    expect(within(data).queryByText("Parent only")).toBeNull();
    expect(within(data).getByRole("link")).not.toHaveClass("text-muted-foreground");
  });

  it("links each node to its detail's Overview, encoding the namespace", async () => {
    serveNamespaces({ namespaces: [nsRow("a", { own: false }), nsRow("a.", { own: false }), nsRow("a..b", { flows: 1 })] });
    renderList();
    await screen.findByTestId("namespaces-table");
    const hrefs = bodyRows().map((r) => within(r).getByRole("link").getAttribute("href"));
    expect(hrefs).toEqual([
      `/explore/${NS_WS}/workflows/namespaces/a/overview`,
      `/explore/${NS_WS}/workflows/namespaces/a./overview`,
      `/explore/${NS_WS}/workflows/namespaces/a..b/overview`,
    ]);
    // The empty segment has a label, and its name says which namespace it is.
    expect(within(bodyRows()[1]).getByRole("link")).toHaveTextContent("(empty segment)");
    expect(within(bodyRows()[1]).getByRole("link")).toHaveAccessibleName("a. (empty segment)");
    await userEvent.click(within(bodyRows()[2]).getByRole("link"));
    expect(screen.getByTestId("where")).toHaveTextContent(`/explore/${NS_WS}/workflows/namespaces/a..b/overview`);
  });

  it("counts the namespaces in the summary", async () => {
    serveNamespaces({ namespaces: ROWS });
    renderList();
    await screen.findByTestId("namespaces-table");
    expect(screen.getByTestId("namespaces-summary")).toHaveTextContent(/^5\s*namespaces$/);
  });
});

describe("NamespacesList: the prefix filter", () => {
  it("reads the filter from the URL and shows that namespace and below, dot-anchored", async () => {
    serveNamespaces({ namespaces: ROWS });
    renderList("?prefix=company");
    await screen.findByTestId("namespaces-table");
    const links = bodyRows().map((r) => within(r).getByRole("link").getAttribute("aria-label"));
    expect(links).toEqual(["company", "company.data", "company.team", "company.team.etl"]);
    expect(screen.getByLabelText("Namespace")).toHaveTextContent("company");
  });

  it("writes a picked namespace into the URL, and clears it again", async () => {
    serveNamespaces({ namespaces: ROWS });
    const user = userEvent.setup();
    renderList();
    await screen.findByTestId("namespaces-table");
    await user.click(screen.getByLabelText("Namespace"));
    const options = (await screen.findAllByRole("option")).map((o) => o.textContent);
    expect(options).toEqual(["All", "company", "company.data", "company.team", "company.team.etl", "companyx"]);
    await user.click(screen.getByRole("option", { name: "company.team" }));
    expect(screen.getByTestId("where")).toHaveTextContent("?prefix=company.team");
    expect(bodyRows().map((r) => within(r).getByRole("link").getAttribute("aria-label"))).toEqual([
      "company.team",
      "company.team.etl",
    ]);
    // A root under a filter shows its full name: its parent is not on screen.
    expect(within(bodyRows()[0]).getByRole("link")).toHaveTextContent(/^company\.team$/);
  });

  it("says when the filter matches nothing, and offers to clear it", async () => {
    serveNamespaces({ namespaces: ROWS });
    const user = userEvent.setup();
    renderList("?prefix=retired");
    expect(await screen.findByText("No namespaces match the current filter.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Clear filter" }));
    expect(screen.getByTestId("where")).not.toHaveTextContent("prefix");
    expect(await screen.findByTestId("namespaces-table")).toBeInTheDocument();
  });
});

describe("NamespacesList: states", () => {
  it("shows a skeleton while loading, then the tree", async () => {
    let open!: () => void;
    serveNamespaces({ namespaces: ROWS, listGate: new Promise<void>((r) => (open = r)) });
    renderList();
    expect(screen.getByTestId("list-skeleton")).toBeInTheDocument();
    expect(screen.queryByText(/Loading/)).toBeNull();
    open();
    expect(await screen.findByTestId("namespaces-table")).toBeInTheDocument();
    expect(screen.queryByTestId("list-skeleton")).toBeNull();
  });

  it("shows the empty state when the workspace has no namespace", async () => {
    serveNamespaces({ namespaces: [] });
    renderList();
    expect(await screen.findByText("No namespaces yet")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Go to flows" })).toHaveAttribute("href", `/explore/${NS_WS}/workflows/flows`);
    expect(screen.queryByTestId("namespaces-table")).toBeNull();
  });

  it("shows the server's message on failure, never the empty state", async () => {
    serveNamespaces({ namespaces: [], listReply: { status: 500, body: { error: "the database is unavailable" } } });
    renderList();
    expect(await screen.findByText(/the database is unavailable/)).toBeInTheDocument();
    expect(screen.queryByText("No namespaces yet")).toBeNull();
  });
});

/** The body row whose link names `ns`. */
const rowOf = (ns: string) => {
  const row = bodyRows().find((r) => within(r).getByRole("link").getAttribute("aria-label") === ns);
  if (!row) throw new Error(`no row for ${ns}`);
  return row;
};
/** The namespaces of the rows on screen, in order. */
const shown = () => bodyRows().map((r) => within(r).getByRole("link").getAttribute("aria-label"));

/**
 * WAI-ARIA's treegrid pattern: a table whose rows form a tree. One row takes
 * Tab; the arrows walk the rows; Right expands or enters, Left collapses or
 * goes to the parent.
 */
describe("NamespacesList: treegrid semantics and keys", () => {
  it("gives every row its level, its place among its siblings, and expanded only when it has children", async () => {
    serveNamespaces({ namespaces: ROWS });
    renderList();
    await screen.findByTestId("namespaces-table");
    const attrs = (ns: string) => {
      const r = rowOf(ns);
      return [r.getAttribute("aria-level"), r.getAttribute("aria-posinset"), r.getAttribute("aria-setsize"), r.getAttribute("aria-expanded")];
    };
    expect(attrs("company")).toEqual(["1", "1", "2", "true"]);
    expect(attrs("company.data")).toEqual(["2", "1", "2", null]);
    expect(attrs("company.team")).toEqual(["2", "2", "2", "true"]);
    expect(attrs("company.team.etl")).toEqual(["3", "1", "1", null]);
    expect(attrs("companyx")).toEqual(["1", "2", "2", null]);
    // The header row is not a tree node.
    expect(within(table()).getAllByRole("row")[0]).not.toHaveAttribute("aria-level");
  });

  it("takes one Tab stop: the first row, and no link inside the grid", async () => {
    serveNamespaces({ namespaces: ROWS });
    const user = userEvent.setup();
    renderList();
    await screen.findByTestId("namespaces-table");
    expect(bodyRows().map((r) => r.tabIndex)).toEqual([0, -1, -1, -1, -1]);
    for (const r of bodyRows()) expect(within(r).getByRole("link")).toHaveAttribute("tabindex", "-1");

    screen.getByLabelText("Namespace").focus();
    await user.tab();
    expect(rowOf("company")).toHaveFocus();
    await user.tab();
    expect(table()).not.toContainElement(document.activeElement as HTMLElement);
  });

  it("walks the rows with Down, Up, Home and End, and the Tab stop follows", async () => {
    serveNamespaces({ namespaces: ROWS });
    const user = userEvent.setup();
    renderList();
    await screen.findByTestId("namespaces-table");
    rowOf("company").focus();
    await user.keyboard("{ArrowDown}");
    expect(rowOf("company.data")).toHaveFocus();
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(rowOf("company.team.etl")).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(rowOf("company.team")).toHaveFocus();
    expect(bodyRows().map((r) => r.tabIndex)).toEqual([-1, -1, 0, -1, -1]);
    await user.keyboard("{End}");
    expect(rowOf("companyx")).toHaveFocus();
    await user.keyboard("{ArrowDown}");
    expect(rowOf("companyx")).toHaveFocus();
    await user.keyboard("{Home}");
    expect(rowOf("company")).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(rowOf("company")).toHaveFocus();
  });

  it("Right enters an expanded row; Left goes to the parent, then collapses; Right expands again", async () => {
    serveNamespaces({ namespaces: ROWS });
    const user = userEvent.setup();
    renderList();
    await screen.findByTestId("namespaces-table");
    rowOf("company.team").focus();
    await user.keyboard("{ArrowRight}");
    expect(rowOf("company.team.etl")).toHaveFocus();
    // A leaf: Right does nothing.
    await user.keyboard("{ArrowRight}");
    expect(rowOf("company.team.etl")).toHaveFocus();

    await user.keyboard("{ArrowLeft}");
    expect(rowOf("company.team")).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(rowOf("company.team")).toHaveAttribute("aria-expanded", "false");
    expect(rowOf("company.team")).toHaveFocus();
    expect(shown()).toEqual(["company", "company.data", "company.team", "companyx"]);
    await user.keyboard("{ArrowLeft}");
    expect(rowOf("company")).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(rowOf("company")).toHaveAttribute("aria-expanded", "false");
    expect(shown()).toEqual(["company", "companyx"]);
    // A collapsed row's Down skips what it hides.
    await user.keyboard("{ArrowDown}");
    expect(rowOf("companyx")).toHaveFocus();
    // A root leaf: Left does nothing.
    await user.keyboard("{ArrowLeft}");
    expect(rowOf("companyx")).toHaveFocus();

    await user.keyboard("{ArrowUp}{ArrowRight}");
    expect(rowOf("company")).toHaveAttribute("aria-expanded", "true");
    expect(rowOf("company")).toHaveFocus();
    // company.team was folded on its own and stays folded.
    expect(shown()).toEqual(["company", "company.data", "company.team", "companyx"]);
  });

  it("opens the focused row's namespace with Enter", async () => {
    serveNamespaces({ namespaces: ROWS });
    const user = userEvent.setup();
    renderList();
    await screen.findByTestId("namespaces-table");
    rowOf("company.data").focus();
    await user.keyboard("{Enter}");
    expect(screen.getByTestId("where")).toHaveTextContent(`/explore/${NS_WS}/workflows/namespaces/company.data/overview`);
  });

  it("collapses and expands from the chevron, for the mouse, without a Tab stop", async () => {
    serveNamespaces({ namespaces: ROWS });
    const user = userEvent.setup();
    renderList();
    await screen.findByTestId("namespaces-table");
    const toggle = within(rowOf("company.team")).getByRole("button", { name: "Collapse company.team" });
    expect(toggle).toHaveAttribute("tabindex", "-1");
    // Leaves have no chevron.
    expect(within(rowOf("companyx")).queryByRole("button")).toBeNull();
    await user.click(toggle);
    expect(rowOf("company.team")).toHaveAttribute("aria-expanded", "false");
    expect(shown()).not.toContain("company.team.etl");
    await user.click(within(rowOf("company.team")).getByRole("button", { name: "Expand company.team" }));
    expect(shown()).toContain("company.team.etl");
  });
});
