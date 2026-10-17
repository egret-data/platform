import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Me } from "@/api/hooks";
import type { BrowseResponse, ObjectLineage, ObjectTags } from "@/api/generated/model";
import { AuthContext } from "@/auth/useAuth";
import { TooltipProvider } from "@/components/ui/tooltip";
import DataRoutes from "@/routes/governance/data/DataRoutes";
import { ObjectHeader } from "@/routes/governance/data/ObjectHeader";
import { browseItems, browseNotListed, catalog, serveData, type FakeData } from "./fakeDataServer";

const ICE = {
  ...catalog("c-ice", "ice", "iceberg", true),
  warehouseName: "ice_wh",
  engines: ["trino-1"],
  // Unsorted on purpose: the page orders by name.
  workspaces: [
    { id: "w2", name: "Sales", slug: "sales" },
    { id: "w1", name: "Finance", slug: "finance" },
  ],
};
const PG = { ...catalog("c-pg", "pg", "hive"), engines: ["trino-2"] };

const ENGINES: BrowseResponse["engines"] = [
  { name: "trino-1", type: "trino", origin: "provisioned", status: "running" },
  { name: "sr-1", type: "starrocks", origin: "provisioned", status: "failed" },
];

const withEngines = (b: BrowseResponse): BrowseResponse => ({ ...b, engines: ENGINES });

const ICE_SCHEMAS = withEngines(browseItems([{ name: "sales", kind: "schema" }, { name: "hr", kind: "schema" }]));
const SALES_OBJECTS = withEngines(
  browseItems([
    { name: "v_orders", kind: "view" },
    { name: "orders", kind: "table" },
    { name: "mv_daily", kind: "materialized_view" },
  ]),
);
const ORDERS_COLUMNS: BrowseResponse = {
  source: { kind: "egret" },
  engines: ENGINES,
  columns: [
    { name: "id", type: "long" },
    { name: "amount", type: "decimal(10,2)" },
    { name: "email", type: "string" },
  ],
};

function tag(key: string) {
  return {
    id: `t-${key}`,
    key,
    name: key.toUpperCase(),
    description: "",
    color: "#336699",
    assignment_count: 0,
    created_at: "2026-10-01T00:00:00Z",
  };
}

const ORDERS_TAGS: ObjectTags = {
  supported: true,
  assignments: [
    { schema: "sales", table: "orders", column: "", tag: tag("confidential") },
    { schema: "sales", table: "orders", column: "email", tag: tag("pii") },
  ],
};

const V_ORDERS_LINEAGE: ObjectLineage = {
  readers: [],
  view: {
    securityMode: "definer",
    owner: "svc_reports",
    ownerCleared: false,
    lineageState: "failed",
    lineageDetail: "The view's definition could not be parsed.",
    bases: [],
  },
};

const ok = (body: unknown) => ({ status: 200, body });
const boom = { status: 500, body: { error: "internal error" } };

function fixture(over: Partial<FakeData> = {}): FakeData {
  return {
    catalogs: ok([PG, ICE]),
    browse: {
      "c-ice": ok(ICE_SCHEMAS),
      "c-ice|sales": ok(SALES_OBJECTS),
      "c-ice|hr": ok(withEngines(browseItems([]))),
      "c-ice|sales|orders": ok(ORDERS_COLUMNS),
      "c-ice|sales|v_orders": ok({ ...ORDERS_COLUMNS, columns: [{ name: "id", type: "long" }] }),
      "c-ice|sales|mv_daily": ok({ ...ORDERS_COLUMNS, columns: [] }),
      "c-pg": ok(browseNotListed("engine_not_running", "No engine egret provisioned for this catalog is running.")),
    },
    tags: {
      "c-ice|sales|orders": ok(ORDERS_TAGS),
      "c-ice|sales|v_orders": ok({ supported: true, assignments: [] }),
      "c-ice|sales|mv_daily": ok({ supported: true, assignments: [] }),
    },
    lineage: {
      "c-ice|sales|v_orders": ok(V_ORDERS_LINEAGE),
      "c-ice|sales|mv_daily": ok({ readers: [], view: null }),
    },
    ...over,
  };
}

const STEWARD: Me = { user_id: "u1", email: "s@egret.local", platform_roles: [], permissions: ["policy:read"] };

function Where() {
  const l = useLocation();
  return <output data-testid="where">{`${l.pathname}${l.search}`}</output>;
}

function renderPage(url: string, me: Me | null = STEWARD) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
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
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

const header = () => screen.getByRole("region", { name: "Object" });
const tabNames = () => screen.getAllByRole("tab").map((t) => t.textContent);
const selectedTab = () => screen.getAllByRole("tab").find((t) => t.getAttribute("aria-selected") === "true")?.textContent;
const where = () => screen.getByTestId("where").textContent;
const panel = () => screen.getByRole("tabpanel");
/** The value a `<dt>` in the open panel names. */
const term = (name: string) => within(panel()).getByText(name, { selector: "dt" }).nextElementSibling?.textContent;

afterEach(() => vi.unstubAllGlobals());

describe("ObjectHeader", () => {
  it("shows a table's name, kind, path, attached engines and its own tags", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales/orders");
    expect(within(header()).getByRole("heading", { level: 1, name: "orders" })).toBeInTheDocument();
    expect(within(header()).getByText("ice.sales.orders")).toBeInTheDocument();
    expect(await within(header()).findByText("Table")).toBeInTheDocument();
    await waitFor(() =>
      expect(
        within(within(header()).getByRole("list", { name: "Engines" }))
          .getAllByRole("listitem")
          .map((li) => li.textContent),
      ).toEqual([
        "sr-1 · StarRocks · failed",
        "trino-1 · Trino",
      ]),
    );
    const tags = await within(header()).findByRole("list", { name: "Tags" });
    // The table's own tag only; a column's tag is not the table's.
    expect(within(tags).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["CONFIDENTIAL"]);
  });

  it("takes an object's kind from its schema's listing", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales/mv_daily");
    expect(await within(header()).findByText("Materialized view")).toBeInTheDocument();
  });

  it("shows no tag section for a catalog or a schema, which cannot be tagged", async () => {
    const { tagCalls } = serveData(fixture());
    renderPage("/governance/data/ice/sales");
    expect(await within(header()).findByText("Schema")).toBeInTheDocument();
    expect(within(header()).queryByRole("list", { name: "Tags" })).not.toBeInTheDocument();
    expect(within(header()).queryByText(/tag/i)).not.toBeInTheDocument();
    expect(tagCalls).toEqual([]);
  });

  it("shows no tag section for an object in a catalog that is not egret's", async () => {
    serveData(
      fixture({
        browse: { "c-pg|s": ok(withEngines(browseItems([{ name: "t", kind: "table" }]))), "c-pg|s|t": ok(ORDERS_COLUMNS) },
        tags: { "c-pg|s|t": ok({ supported: false, assignments: null }) },
      }),
    );
    renderPage("/governance/data/pg/s/t");
    expect(await within(header()).findByText("Table")).toBeInTheDocument();
    await screen.findByRole("cell", { name: "amount" });
    expect(within(header()).queryByRole("list", { name: "Tags" })).not.toBeInTheDocument();
    expect(within(header()).queryByText(/no tags/i)).not.toBeInTheDocument();
  });

  it("says when the tags could not be loaded, and keeps the rest of the header", async () => {
    serveData(fixture({ tags: { "c-ice|sales|orders": boom } }));
    renderPage("/governance/data/ice/sales/orders");
    expect(await within(header()).findByText("Tags could not be loaded.")).toBeInTheDocument();
    expect(within(header()).getByRole("heading", { level: 1, name: "orders" })).toBeInTheDocument();
    expect(within(header()).getByText("Table")).toBeInTheDocument();
  });

  it("falls back to the catalog's engine names when the browse call fails", async () => {
    serveData(fixture({ browse: { ...fixture().browse, "c-ice|sales|orders": boom } }));
    renderPage("/governance/data/ice/sales/orders");
    await waitFor(() =>
      expect(
        within(within(header()).getByRole("list", { name: "Engines" }))
          .getAllByRole("listitem")
          .map((li) => li.textContent),
      ).toEqual(["trino-1"]),
    );
    // The browse call did fail: the Overview says so.
    expect(await within(panel()).findByText("The columns could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
  });

  describe("actions", () => {
    const base = { path: { catalog: "ice", schema: "s", object: "t" }, kind: "table" as const, engines: [], tags: null };
    const all = { onGrant: vi.fn(), onAddTag: vi.fn(), onWhoCanRead: vi.fn() };
    const renderHeader = (me: Me | null, handlers: Partial<typeof all>) =>
      render(
        <AuthContext.Provider value={{ me, isLoading: false }}>
          <ObjectHeader {...base} {...handlers} />
        </AuthContext.Provider>,
      );
    const buttons = () => screen.queryAllByRole("button").map((b) => b.textContent);

    it("shows each action only with its permission", () => {
      renderHeader({ ...STEWARD, permissions: ["policy:create", "catalog_registry:update", "policy:read"] }, all);
      expect(buttons()).toEqual(["Grant access", "Add tag", "Who can read?"]);
    });

    it("hides an action the viewer lacks the permission for", () => {
      renderHeader({ ...STEWARD, permissions: ["policy:read"] }, all);
      expect(buttons()).toEqual(["Who can read?"]);
    });

    it("hides Add tag without catalog_registry:update, even with tag permissions", () => {
      renderHeader({ ...STEWARD, permissions: ["policy:create", "tag:read", "tag:manage"] }, all);
      expect(buttons()).toEqual(["Grant access"]);
    });

    it("gives a platform admin every action", () => {
      renderHeader({ ...STEWARD, platform_roles: ["platform_admin"], permissions: [] }, all);
      expect(buttons()).toEqual(["Grant access", "Add tag", "Who can read?"]);
    });

    it("hides an action no handler is wired for", () => {
      renderHeader({ ...STEWARD, platform_roles: ["platform_admin"] }, { onWhoCanRead: all.onWhoCanRead });
      expect(buttons()).toEqual(["Who can read?"]);
    });

    it("calls the action's handler", async () => {
      const onGrant = vi.fn();
      renderHeader({ ...STEWARD, permissions: ["policy:create"] }, { onGrant });
      await userEvent.setup().click(screen.getByRole("button", { name: "Grant access" }));
      expect(onGrant).toHaveBeenCalledTimes(1);
    });
  });
});

describe("ObjectTabs", () => {
  it("gives each level the tabs of spec §2.2", async () => {
    serveData(fixture());
    const { unmount } = renderPage("/governance/data/ice");
    expect(tabNames()).toEqual(["Overview", "Permissions", "Masks & filters", "Details"]);
    unmount();
    renderPage("/governance/data/ice/sales");
    expect(tabNames()).toEqual(["Overview", "Permissions", "Masks & filters"]);
  });

  it("gives a table and a view the same tabs", async () => {
    serveData(fixture());
    const { unmount } = renderPage("/governance/data/ice/sales/orders");
    expect(tabNames()).toEqual(["Overview", "Permissions", "Masks & filters", "Lineage", "Details"]);
    unmount();
    renderPage("/governance/data/ice/sales/v_orders");
    expect(tabNames()).toEqual(["Overview", "Permissions", "Masks & filters", "Lineage", "Details"]);
  });

  it("selects the tab ?tab= names", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice?tab=details");
    expect(selectedTab()).toBe("Details");
    expect(await within(panel()).findByText("ice_wh")).toBeInTheDocument();
  });

  it("falls back to Overview for an unknown tab, and for a tab the level does not have", async () => {
    serveData(fixture());
    const { unmount } = renderPage("/governance/data/ice?tab=nope");
    expect(selectedTab()).toBe("Overview");
    unmount();
    renderPage("/governance/data/ice/sales?tab=details");
    expect(selectedTab()).toBe("Overview");
  });

  it("puts the chosen tab in the URL, keeping the rest of the search", async () => {
    serveData(fixture());
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?column=email");
    await user.click(screen.getByRole("tab", { name: "Details" }));
    expect(where()).toBe("/governance/data/ice/sales/orders?column=email&tab=details");
    await user.click(screen.getByRole("tab", { name: "Overview" }));
    expect(where()).toBe("/governance/data/ice/sales/orders?column=email");
  });

  it("moves between tabs with the arrow keys", async () => {
    serveData(fixture());
    const user = userEvent.setup();
    renderPage("/governance/data/ice");
    await user.click(screen.getByRole("tab", { name: "Overview" }));
    await user.keyboard("{ArrowRight}");
    // Radix moves focus in a timeout, and a focused tab selects itself.
    await waitFor(() => expect(where()).toBe("/governance/data/ice?tab=permissions"));
    expect(screen.getByRole("tab", { name: "Permissions" })).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    await waitFor(() => expect(where()).toBe("/governance/data/ice"));
    await user.keyboard("{ArrowLeft}");
    await waitFor(() => expect(where()).toBe("/governance/data/ice?tab=details"));
    expect(screen.getByRole("tab", { name: "Details" })).toHaveFocus();
  });
});

describe("OverviewTab", () => {
  it("shows a catalog's schemas, attached engines and workspaces", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice");
    const schemas = await within(panel()).findByRole("list", { name: "Schemas" });
    const links = within(schemas).getAllByRole("link");
    expect(links.map((a) => a.textContent)).toEqual(["hr", "sales"]);
    expect(links[1]).toHaveAttribute("href", "/governance/data/ice/sales");
    const engines = within(panel()).getByRole("table", { name: "Attached engines" });
    expect(within(engines).getAllByRole("row").slice(1).map((r) => r.textContent)).toEqual([
      "sr-1StarRocksProvisionedFailed",
      "trino-1TrinoProvisionedRunning",
    ]);
    const workspaces = within(panel()).getByRole("list", { name: "Workspaces" });
    expect(within(workspaces).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["Finance", "Sales"]);
  });

  it("says a catalog assigned to no workspace is assigned to none", async () => {
    serveData(fixture({ catalogs: ok([{ ...ICE, workspaces: [] }]) }));
    renderPage("/governance/data/ice");
    expect(await within(panel()).findByText("Not assigned to any workspace")).toBeInTheDocument();
  });

  it("shows a schema's tables, views and materialized views with their kind", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales");
    const objects = await within(panel()).findByRole("table", { name: "Tables and views" });
    expect(within(objects).getAllByRole("row").slice(1).map((r) => r.textContent)).toEqual([
      "mv_dailyMaterialized view",
      "ordersTable",
      "v_ordersView",
    ]);
    expect(within(objects).getByRole("link", { name: "orders" })).toHaveAttribute(
      "href",
      "/governance/data/ice/sales/orders",
    );
  });

  it("shows a table's columns with their type, own tags and the table's tags marked inherited", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales/orders");
    const columns = await within(panel()).findByRole("table", { name: "Columns" });
    await within(columns).findByText("PII");
    const rows = within(columns).getAllByRole("row").slice(1);
    expect(rows.map((r) => within(r).getAllByRole("cell")[0].textContent)).toEqual(["id", "amount", "email"]);
    expect(within(rows[1]).getByText("decimal(10,2)")).toBeInTheDocument();
    const emailTags = within(within(rows[2]).getByRole("list", { name: "Tags of email" })).getAllByRole("listitem");
    expect(emailTags.map((li) => li.textContent)).toEqual(["PII", "CONFIDENTIAL from table"]);
    const idTags = within(within(rows[0]).getByRole("list", { name: "Tags of id" })).getAllByRole("listitem");
    expect(idTags.map((li) => li.textContent)).toEqual(["CONFIDENTIAL from table"]);
  });

  it("shows a view's security mode, whether its owner is cleared, and its lineage state", async () => {
    const { lineageCalls } = serveData(fixture());
    renderPage("/governance/data/ice/sales/v_orders");
    await within(panel()).findByText("Security mode", { selector: "dt" });
    expect(term("Security mode")).toBe("DEFINER");
    expect(term("Owner")).toBe("svc_reports");
    expect(term("Owner cleared")).toBe("No");
    expect(term("Lineage")).toBe("Failed: The view's definition could not be parsed.");
    expect(lineageCalls).toEqual(["c-ice|sales|v_orders"]);
    // The columns are still there.
    expect(within(panel()).getByRole("table", { name: "Columns" })).toBeInTheDocument();
  });

  it("says when egret holds no view facts for a materialized view", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales/mv_daily");
    expect(await within(panel()).findByText("egret has no security mode or lineage recorded for this object.")).toBeInTheDocument();
  });

  it("never asks for lineage on a table", async () => {
    const { lineageCalls } = serveData(fixture());
    renderPage("/governance/data/ice/sales/orders");
    await within(panel()).findByRole("table", { name: "Columns" });
    expect(lineageCalls).toEqual([]);
  });

  it("says an empty level is empty", async () => {
    serveData(fixture());
    const { unmount } = renderPage("/governance/data/ice/sales/mv_daily");
    expect(await within(panel()).findByText("No columns returned")).toBeInTheDocument();
    unmount();
    renderPage("/governance/data/ice/hr");
    expect(await within(panel()).findByText("No tables or views")).toBeInTheDocument();
  });

  it("shows a skeleton while the level loads", async () => {
    serveData(fixture({ browse: { ...fixture().browse, "c-ice|sales|orders": "never" } }));
    renderPage("/governance/data/ice/sales/orders");
    expect(await within(panel()).findByTestId("list-skeleton")).toBeInTheDocument();
  });

  it("shows why a level was not listed", async () => {
    serveData(fixture());
    renderPage("/governance/data/pg");
    expect(await within(panel()).findByText("No engine egret provisioned for this catalog is running.")).toBeInTheDocument();
  });

  it("shows an error, and leaves the header and the other tabs working", async () => {
    serveData(fixture({ browse: { ...fixture().browse, "c-ice|sales|orders": boom } }));
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders");
    expect(await within(panel()).findByText("The columns could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(within(panel()).queryByText("No columns returned")).not.toBeInTheDocument();
    expect(within(header()).getByRole("heading", { level: 1, name: "orders" })).toBeInTheDocument();
    expect(await within(header()).findByText("CONFIDENTIAL")).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Details" }));
    expect(await within(panel()).findByText("ice_wh")).toBeInTheDocument();
  });

  it("shows a lineage error without hiding the columns", async () => {
    serveData(fixture({ lineage: { "c-ice|sales|v_orders": { status: 503, body: { error: "warehouse unavailable" } } } }));
    renderPage("/governance/data/ice/sales/v_orders");
    expect(await within(panel()).findByText("The view's security mode and lineage could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(within(panel()).getByRole("table", { name: "Columns" })).toBeInTheDocument();
  });
});

describe("DetailsTab", () => {
  const row = term;

  it("shows an object's full path, warehouse, workspaces and engines", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales/orders?tab=details");
    await within(panel()).findByText("ice_wh");
    expect(row("Full path")).toBe("ice.sales.orders");
    expect(row("Warehouse")).toBe("ice_wh");
    expect(row("Workspaces")).toBe("Finance, Sales");
    expect(row("Engines")).toBe("trino-1");
  });

  it("says a catalog that is not egret's has no warehouse", async () => {
    serveData(fixture());
    renderPage("/governance/data/pg?tab=details");
    await within(panel()).findByText("pg");
    expect(row("Warehouse")).toBe("None: not an egret catalog");
    expect(row("Workspaces")).toBe("None");
    expect(row("Engines")).toBe("trino-2");
  });

  it("says the catalog list could not be loaded", async () => {
    serveData(fixture({ catalogs: boom }));
    renderPage("/governance/data/ice?tab=details");
    expect(await within(panel()).findByText("The catalog could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(within(header()).getByRole("heading", { level: 1, name: "ice" })).toBeInTheDocument();
  });

  it("says a catalog the URL names does not exist", async () => {
    serveData(fixture());
    renderPage("/governance/data/nope?tab=details");
    expect(await screen.findAllByText("No catalog is named “nope”.")).not.toHaveLength(0);
  });
});
