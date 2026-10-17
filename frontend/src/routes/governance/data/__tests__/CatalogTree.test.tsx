import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import DataRoutes from "@/routes/governance/data/DataRoutes";
import { browseItems, browseNotListed, catalog, serveData, type FakeData } from "./fakeDataServer";

/**
 * The catalogs come UNSORTED (FE-TEST-1): the tree must order them, and a
 * test reading the first row must not pass because the fixture happened to
 * be in order already.
 */
const CATALOGS = [
  catalog("c-pg", "pg", "hive"),
  catalog("c-ice", "ice", "iceberg", true),
];

/** The ice catalog's schemas, and the `sales` schema's objects, also unsorted. */
const ICE_SCHEMAS = browseItems([
  { name: "sales", kind: "schema" },
  { name: "hr", kind: "schema" },
]);
const SALES_OBJECTS = browseItems([
  { name: "v_orders", kind: "view" },
  { name: "customers", kind: "table" },
  { name: "mv_daily", kind: "materialized_view" },
]);

function fixture(over: Partial<FakeData> = {}): FakeData {
  return {
    catalogs: { status: 200, body: CATALOGS },
    browse: {
      "c-ice": { status: 200, body: ICE_SCHEMAS },
      "c-ice|sales": { status: 200, body: SALES_OBJECTS },
      "c-ice|hr": { status: 200, body: browseItems([]) },
      "c-pg": { status: 200, body: browseNotListed("engine_not_running", "The engine trino-1 is not running.") },
    },
    ...over,
  };
}

function Where() {
  const l = useLocation();
  return <output data-testid="where">{`${l.pathname}${l.search}`}</output>;
}

function renderData(url = "/governance/data") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[url]}>
          <Routes>
            <Route
              path="/governance/data/*"
              element={
                <>
                  <DataRoutes />
                  <Where />
                </>
              }
            />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

const tree = () => screen.getByRole("treegrid", { name: "Catalogs" });
const rows = () => within(tree()).getAllByRole("row");
/** The row whose link names `path` (its full dotted path). */
const row = (path: string) => {
  const found = rows().find((r) => within(r).queryByRole("link", { name: path }));
  if (!found) throw new Error(`no row for ${path}; rows: ${rows().map((r) => r.textContent).join(" | ")}`);
  return found;
};
const shownPaths = () =>
  rows()
    .map((r) => within(r).queryByRole("link")?.getAttribute("aria-label"))
    .filter((x): x is string => !!x);
const where = () => screen.getByTestId("where").textContent;

afterEach(() => vi.unstubAllGlobals());

describe("CatalogTree", () => {
  it("lists the registry's catalogs by name, each with its source", async () => {
    serveData(fixture());
    renderData();
    await waitFor(() => expect(shownPaths()).toEqual(["ice", "pg"]));
    expect(row("ice")).toHaveTextContent("Iceberg · egret");
    expect(row("pg")).toHaveTextContent("Hive · via engine");
    expect(row("ice")).toHaveAttribute("aria-level", "1");
    expect(row("ice")).toHaveAttribute("aria-expanded", "false");
  });

  it("loads a catalog's schemas only when it is expanded", async () => {
    const { browseCalls } = serveData(fixture());
    const user = userEvent.setup();
    renderData();
    await waitFor(() => expect(shownPaths()).toEqual(["ice", "pg"]));
    expect(browseCalls).toEqual([]);
    await user.click(within(row("ice")).getByRole("button", { name: "Expand ice" }));
    await waitFor(() => expect(shownPaths()).toEqual(["ice", "ice.hr", "ice.sales", "pg"]));
    expect(browseCalls).toEqual(["c-ice"]);
    expect(row("ice")).toHaveAttribute("aria-expanded", "true");
    expect(row("ice.sales")).toHaveAttribute("aria-level", "2");
    expect(within(row("ice.sales")).getByRole("img", { name: "Schema" })).toBeInTheDocument();
  });

  it("shows a schema's tables, views and materialized views with their kind", async () => {
    const { browseCalls } = serveData(fixture());
    const user = userEvent.setup();
    renderData();
    await waitFor(() => expect(shownPaths()).toContain("ice"));
    await user.click(within(row("ice")).getByRole("button", { name: "Expand ice" }));
    await waitFor(() => expect(shownPaths()).toContain("ice.sales"));
    await user.click(within(row("ice.sales")).getByRole("button", { name: "Expand ice.sales" }));
    await waitFor(() =>
      expect(shownPaths()).toEqual(["ice", "ice.hr", "ice.sales", "ice.sales.customers", "ice.sales.mv_daily", "ice.sales.v_orders", "pg"]),
    );
    expect(browseCalls).toEqual(["c-ice", "c-ice|sales"]);
    expect(within(row("ice.sales.customers")).getByRole("img", { name: "Table" })).toBeInTheDocument();
    expect(within(row("ice.sales.v_orders")).getByRole("img", { name: "View" })).toBeInTheDocument();
    expect(within(row("ice.sales.mv_daily")).getByRole("img", { name: "Materialized view" })).toBeInTheDocument();
    // An object is a leaf: no expand control, no aria-expanded.
    expect(row("ice.sales.customers")).not.toHaveAttribute("aria-expanded");
    expect(row("ice.sales.customers")).toHaveAttribute("aria-level", "3");
    expect(within(row("ice.sales.customers")).getByRole("link")).toHaveAttribute(
      "href",
      "/governance/data/ice/sales/customers",
    );
  });

  it("says an expanded level that holds nothing is empty", async () => {
    serveData(fixture());
    const user = userEvent.setup();
    renderData();
    await waitFor(() => expect(shownPaths()).toContain("ice"));
    await user.click(within(row("ice")).getByRole("button", { name: "Expand ice" }));
    await waitFor(() => expect(shownPaths()).toContain("ice.hr"));
    await user.click(within(row("ice.hr")).getByRole("button", { name: "Expand ice.hr" }));
    expect(await within(tree()).findByText("No tables or views")).toBeInTheDocument();
  });

  it("shows why a level was not listed, and a Go to path control, never an empty branch", async () => {
    serveData(fixture());
    const user = userEvent.setup();
    renderData();
    await waitFor(() => expect(shownPaths()).toContain("pg"));
    await user.click(within(row("pg")).getByRole("button", { name: "Expand pg" }));
    const reason = await within(tree()).findByText("The engine trino-1 is not running.");
    const notListedRow = reason.closest("tr")!;
    expect(notListedRow).toHaveAttribute("aria-level", "2");
    expect(within(tree()).queryByText(/^No schemas$/)).not.toBeInTheDocument();
    // The control opens the box with the catalog filled in, ready for the rest of the path.
    await user.click(within(notListedRow).getByRole("button", { name: "Go to path" }));
    const box = screen.getByRole("textbox", { name: "Go to path" });
    expect(box).toHaveValue("pg.");
    expect(box).toHaveFocus();
  });

  it("says a level could not be listed when browse itself fails", async () => {
    serveData(fixture({ browse: { "c-ice": { status: 500, body: { error: "boom" } } } }));
    const user = userEvent.setup();
    renderData();
    await waitFor(() => expect(shownPaths()).toContain("ice"));
    await user.click(within(row("ice")).getByRole("button", { name: "Expand ice" }));
    expect(await within(tree()).findByText(/could not be listed/)).toBeInTheDocument();
  });

  it("filters what is loaded by the search box, keeping a match's parents", async () => {
    serveData(fixture());
    const user = userEvent.setup();
    renderData();
    await waitFor(() => expect(shownPaths()).toContain("ice"));
    await user.click(within(row("ice")).getByRole("button", { name: "Expand ice" }));
    await waitFor(() => expect(shownPaths()).toContain("ice.sales"));
    await user.click(within(row("ice.sales")).getByRole("button", { name: "Expand ice.sales" }));
    await waitFor(() => expect(shownPaths()).toContain("ice.sales.customers"));
    await user.type(screen.getByRole("searchbox", { name: "Filter loaded items" }), "CUST");
    expect(shownPaths()).toEqual(["ice", "ice.sales", "ice.sales.customers"]);
    await user.clear(screen.getByRole("searchbox", { name: "Filter loaded items" }));
    await user.type(screen.getByRole("searchbox", { name: "Filter loaded items" }), "zzz");
    expect(screen.getByText("No loaded item matches the filter.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Clear filter" }));
    expect(shownPaths()).toContain("pg");
  });

  it("goes to a typed path, and opens the tree down to it", async () => {
    const { browseCalls } = serveData(fixture());
    const user = userEvent.setup();
    renderData();
    await waitFor(() => expect(shownPaths()).toContain("ice"));
    await user.type(screen.getByRole("textbox", { name: "Go to path" }), "ice.sales.customers{Enter}");
    await waitFor(() => expect(where()).toBe("/governance/data/ice/sales/customers"));
    await waitFor(() => expect(shownPaths()).toContain("ice.sales.customers"));
    expect(browseCalls).toEqual(expect.arrayContaining(["c-ice", "c-ice|sales"]));
    expect(row("ice.sales.customers")).toHaveAttribute("aria-selected", "true");
  });

  it("refuses a path whose catalog is not in the registry, beside the box", async () => {
    serveData(fixture());
    const user = userEvent.setup();
    renderData();
    await waitFor(() => expect(shownPaths()).toContain("ice"));
    await user.type(screen.getByRole("textbox", { name: "Go to path" }), "nope.a.b{Enter}");
    expect(await screen.findByText("No catalog is named “nope”.")).toBeInTheDocument();
    expect(where()).toBe("/governance/data");
  });

  it("is a keyboard treegrid with one Tab stop", async () => {
    const { browseCalls } = serveData(fixture());
    const user = userEvent.setup();
    renderData();
    await waitFor(() => expect(shownPaths()).toEqual(["ice", "pg"]));
    const tabStops = () => rows().filter((r) => r.tabIndex === 0);
    expect(tabStops()).toEqual([row("ice")]);

    row("ice").focus();
    await user.keyboard("{ArrowDown}");
    expect(row("pg")).toHaveFocus();
    expect(tabStops()).toEqual([row("pg")]);
    await user.keyboard("{ArrowUp}");
    expect(row("ice")).toHaveFocus();

    // Right expands; Right again enters the first child.
    await user.keyboard("{ArrowRight}");
    expect(row("ice")).toHaveAttribute("aria-expanded", "true");
    await waitFor(() => expect(shownPaths()).toContain("ice.hr"));
    expect(browseCalls).toEqual(["c-ice"]);
    await user.keyboard("{ArrowRight}");
    expect(row("ice.hr")).toHaveFocus();
    // Left on a closed child goes to its parent; Left on an open parent collapses it.
    await user.keyboard("{ArrowLeft}");
    expect(row("ice")).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(row("ice")).toHaveAttribute("aria-expanded", "false");
    expect(shownPaths()).toEqual(["ice", "pg"]);

    // Enter opens the focused node.
    await user.keyboard("{ArrowDown}{Enter}");
    expect(where()).toBe("/governance/data/pg");
  });

  it("says when listing catalogs needs a permission the user lacks", async () => {
    serveData(fixture({ catalogs: { status: 403, body: { error: "forbidden" } } }));
    renderData();
    expect(await screen.findByText(/needs the catalog_registry:read permission/)).toBeInTheDocument();
  });

  it("says the registry is empty when it is", async () => {
    serveData(fixture({ catalogs: { status: 200, body: [] } }));
    renderData();
    expect(await screen.findByText("No catalogs")).toBeInTheDocument();
  });
});

describe("DataPage", () => {
  it("asks to pick a catalog when nothing is selected", async () => {
    serveData(fixture());
    renderData();
    expect(screen.getByRole("heading", { level: 1, name: "Data" })).toBeInTheDocument();
    expect(screen.getByText(/Pick a catalog/)).toBeInTheDocument();
  });

  it("collapses the tree under a toggle on a narrow screen", async () => {
    serveData(fixture());
    const user = userEvent.setup();
    renderData();
    const toggle = screen.getByRole("button", { name: "Show catalogs" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await user.click(toggle);
    expect(screen.getByRole("button", { name: "Hide catalogs" })).toHaveAttribute("aria-expanded", "true");
  });

  it("opens an object's page from its URL, with the tree open down to it", async () => {
    serveData(fixture());
    renderData("/governance/data/ice/sales/customers");
    expect(screen.getByRole("heading", { level: 1, name: "customers" })).toBeInTheDocument();
    await waitFor(() => expect(shownPaths()).toContain("ice.sales.customers"));
  });

  it("sends an unknown deeper URL back to the Data index", async () => {
    serveData(fixture());
    renderData("/governance/data/a/b/c/d");
    await waitFor(() => expect(where()).toBe("/governance/data"));
  });
});
