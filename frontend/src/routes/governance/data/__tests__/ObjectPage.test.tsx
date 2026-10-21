import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Me } from "@/api/hooks";
import type { BrowseResponse, GrantView, ObjectLineage, ObjectTags, RuleView, Scope } from "@/api/generated/model";
import { AuthContext } from "@/auth/useAuth";
import { TooltipProvider } from "@/components/ui/tooltip";
import DataRoutes from "@/routes/governance/data/DataRoutes";
import { ObjectHeader } from "@/routes/governance/data/ObjectHeader";
import { scopeToDataLink } from "@/routes/governance/openInData";
import { browseItems, browseNotListed, catalog, serveData, type FakeData } from "./fakeDataServer";
import { handwrittenAnswer } from "./handwrittenFixture";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

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

function pathRule(id: string, scope: Omit<Scope, "catalog_id">, kind: RuleView["kind"], catalogId = "c-ice"): RuleView {
  return {
    id,
    name: id,
    kind,
    selector: { type: "path", catalog_id: catalogId, ...scope },
    exempt: { include: [], exclude: [] },
    catalog_name: catalogId === "c-ice" ? "ice" : "pg",
  };
}
/** The read surface resolves the tag's name (`null` when the tag is gone); the fixture tags are named `KEY`. */
function tagRule(id: string, key: string, kind: RuleView["kind"], tagName: string | null = key.toUpperCase()): RuleView {
  return { id, name: id, kind, selector: { type: "tag", key }, exempt: { include: [], exclude: [] }, tag_name: tagName };
}
const HASH = { type: "mask", mask_type: "MASK_HASH" } as const;
const NULLIFY = { type: "mask", mask_type: "MASK_NULL" } as const;
const REDACT = { type: "mask", mask_type: "MASK" } as const;
const US_ONLY = { type: "row_filter", filter_expr: "region = 'us'" } as const;

/** Rules around `ice.sales.orders` (tags: CONFIDENTIAL on the table, PII on `email`). Unsorted on purpose. */
const R = {
  colPath: pathRule("hash_email", { schema: "sales", table: "orders", column: "email" }, HASH),
  tableFilter: { ...pathRule("us_only", { schema: "sales", table: "orders" }, US_ONLY), exempt: { include: [{ kind: "group", ref: "auditors" }, { kind: "user", ref: "alice" }], exclude: [] } },
  schemaMask: pathRule("null_sales", { schema: "sales" }, NULLIFY),
  catalogFilter: pathRule("ice_filter", {}, US_ONLY),
  pattern: pathRule("star_orders", { schema: "sales", table: "ord*" }, REDACT),
  tagOnColumn: tagRule("redact_pii", "pii", REDACT),
  tagOnTable: tagRule("redact_confidential", "confidential", REDACT),
  // PII is on a column only: a row filter is a table request, and a column's tag does not reach up to it.
  tagFilterOnColumn: tagRule("pii_filter", "pii", US_ONLY),
  otherTable: pathRule("hash_customers", { schema: "sales", table: "customers", column: "email" }, HASH),
  otherCatalog: pathRule("pg_mask", { schema: "sales", table: "orders", column: "email" }, HASH, "c-pg"),
  otherTag: tagRule("redact_secret", "secret", REDACT),
};
const ALL_RULES = [
  R.tagOnColumn,
  R.otherTable,
  R.colPath,
  R.tagFilterOnColumn,
  R.schemaMask,
  R.otherCatalog,
  R.pattern,
  R.tableFilter,
  R.otherTag,
  R.tagOnTable,
  R.catalogFilter,
];

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
    rules: ok([]),
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

afterEach(() => {
  vi.unstubAllGlobals();
  toast.success.mockClear();
  toast.error.mockClear();
});

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
    const tags = await within(header()).findByRole("list", { name: "Classifications" });
    // The table's own classification only; a column's is not the table's.
    expect(within(tags).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["CONFIDENTIAL"]);
    expect(within(header()).queryByRole("list", { name: "Tags" })).not.toBeInTheDocument();
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
    expect(within(header()).queryByRole("list", { name: "Classifications" })).not.toBeInTheDocument();
    expect(within(header()).queryByText(/tag|classification/i)).not.toBeInTheDocument();
    expect(tagCalls).toEqual([]);
  });

  it("shows no tag section for an object in a catalog that is not egret's, and never asks for its tags", async () => {
    const { tagCalls } = serveData(
      fixture({
        browse: { "c-pg|s": ok(withEngines(browseItems([{ name: "t", kind: "table" }]))), "c-pg|s|t": ok(ORDERS_COLUMNS) },
      }),
    );
    renderPage("/governance/data/pg/s/t");
    expect(await within(header()).findByText("Table")).toBeInTheDocument();
    await screen.findByRole("cell", { name: "amount" });
    expect(within(header()).queryByRole("list", { name: "Classifications" })).not.toBeInTheDocument();
    expect(within(header()).queryByText(/tag|classification/i)).not.toBeInTheDocument();
    // The Overview's columns carry no Tags column either, and nothing says "loading".
    const columns = within(panel()).getByRole("table", { name: "Columns" });
    expect(within(columns).queryByRole("columnheader", { name: "Classifications" })).not.toBeInTheDocument();
    expect(within(panel()).queryByText("Loading classifications")).not.toBeInTheDocument();
    expect(tagCalls).toEqual([]);
  });

  it("says the kind is unknown when the schema cannot be listed, and asks for no view facts", async () => {
    for (const answer of [
      boom,
      ok(browseNotListed("engine_error", "The engine refused the listing.")),
      ok(withEngines(browseItems([{ name: "orders", kind: "table" }]))),
    ]) {
      const { lineageCalls } = serveData(fixture({ browse: { ...fixture().browse, "c-ice|sales": answer } }));
      const { unmount } = renderPage("/governance/data/ice/sales/v_orders");
      expect(await within(header()).findByText("Kind unknown")).toBeInTheDocument();
      // The columns still load; the view's facts are never asked for.
      await within(panel()).findByRole("table", { name: "Columns" });
      expect(within(header()).queryByText("View")).not.toBeInTheDocument();
      expect(within(panel()).queryByText("Security and lineage")).not.toBeInTheDocument();
      expect(lineageCalls).toEqual([]);
      unmount();
      vi.unstubAllGlobals();
    }
  });

  it("shows a skeleton for the kind while the schema is being listed", async () => {
    serveData(fixture({ browse: { ...fixture().browse, "c-ice|sales": "never" } }));
    renderPage("/governance/data/ice/sales/orders");
    expect(await within(header()).findByText("Loading kind")).toBeInTheDocument();
    expect(within(header()).queryByText("Kind unknown")).not.toBeInTheDocument();
  });

  it("says when the tags could not be loaded, and keeps the rest of the header", async () => {
    serveData(fixture({ tags: { "c-ice|sales|orders": boom } }));
    renderPage("/governance/data/ice/sales/orders");
    expect(await within(header()).findByText("Classifications could not be loaded.")).toBeInTheDocument();
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
      renderHeader({ ...STEWARD, permissions: ["policy:update", "catalog_registry:update", "policy:read"] }, all);
      expect(buttons()).toEqual(["Grant access", "Add classification", "Who can read?"]);
    });

    // `POST /perm/grants` requires `policy:update`, not `policy:create`.
    it("gates Grant access on policy:update, the permission its route requires", () => {
      renderHeader({ ...STEWARD, permissions: ["policy:create", "policy:read"] }, all);
      expect(buttons()).toEqual(["Who can read?"]);
    });

    it("hides an action the viewer lacks the permission for", () => {
      renderHeader({ ...STEWARD, permissions: ["policy:read"] }, all);
      expect(buttons()).toEqual(["Who can read?"]);
    });

    it("hides Add classification without catalog_registry:update, even with tag permissions", () => {
      renderHeader({ ...STEWARD, permissions: ["policy:update", "tag:read", "tag:manage"] }, all);
      expect(buttons()).toEqual(["Grant access"]);
    });

    it("gives a platform admin every action", () => {
      renderHeader({ ...STEWARD, platform_roles: ["platform_admin"], permissions: [] }, all);
      expect(buttons()).toEqual(["Grant access", "Add classification", "Who can read?"]);
    });

    it("hides an action no handler is wired for", () => {
      renderHeader({ ...STEWARD, platform_roles: ["platform_admin"] }, { onWhoCanRead: all.onWhoCanRead });
      expect(buttons()).toEqual(["Who can read?"]);
    });

    it("calls the action's handler", async () => {
      const onGrant = vi.fn();
      renderHeader({ ...STEWARD, permissions: ["policy:update"] }, { onGrant });
      await userEvent.setup().click(screen.getByRole("button", { name: "Grant access" }));
      expect(onGrant).toHaveBeenCalledTimes(1);
    });
  });
});

describe("ObjectTabs", () => {
  it("gives each level the tabs of spec §2.2", async () => {
    serveData(fixture());
    const { unmount } = renderPage("/governance/data/ice");
    expect(tabNames()).toEqual(["Overview", "Permissions", "Rules", "Details"]);
    unmount();
    renderPage("/governance/data/ice/sales");
    expect(tabNames()).toEqual(["Overview", "Permissions", "Rules"]);
  });

  it("gives a table and a view the same tabs", async () => {
    serveData(fixture());
    const { unmount } = renderPage("/governance/data/ice/sales/orders");
    expect(tabNames()).toEqual(["Overview", "Permissions", "Rules", "Lineage", "Details"]);
    unmount();
    renderPage("/governance/data/ice/sales/v_orders");
    expect(tabNames()).toEqual(["Overview", "Permissions", "Rules", "Lineage", "Details"]);
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
    // Not `?column=`: that opens the column panel, a modal over the tabs (its own test keeps `?tab=` on close).
    renderPage("/governance/data/ice/sales/orders?from=tree");
    await user.click(screen.getByRole("tab", { name: "Details" }));
    expect(where()).toBe("/governance/data/ice/sales/orders?from=tree&tab=details");
    await user.click(screen.getByRole("tab", { name: "Overview" }));
    expect(where()).toBe("/governance/data/ice/sales/orders?from=tree");
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
  it("stacks the columns table into labelled cards on a narrow screen", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales/orders");
    const columns = await within(panel()).findByRole("table", { name: "Columns" });
    expect(columns.className).toContain("max-sm:block");
    const email = within(columns).getByRole("link", { name: "email" }).closest("tr") as HTMLElement;
    expect([...email.querySelectorAll("td[data-label]")].map((td) => td.getAttribute("data-label"))).toEqual([
      "Type",
      "Classifications",
      "Rules",
    ]);
  });

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
    const emailTags = within(within(rows[2]).getByRole("list", { name: "Classifications of email" })).getAllByRole("listitem");
    expect(emailTags.map((li) => li.textContent)).toEqual(["PII", "CONFIDENTIAL from table"]);
    const idTags = within(within(rows[0]).getByRole("list", { name: "Classifications of id" })).getAllByRole("listitem");
    expect(idTags.map((li) => li.textContent)).toEqual(["CONFIDENTIAL from table"]);
  });

  it("lists each column's masking rules by name and kind, by path or by tag, and says nothing of who sees what", async () => {
    const exempted = {
      ...R.colPath,
      exempt: { include: [{ kind: "user" as const, ref: "bob" }, { kind: "workspace_members" as const, ref: "w2" }], exclude: [] },
    };
    serveData(fixture({ rules: ok(ALL_RULES.map((r) => (r === R.colPath ? exempted : r))) }));
    renderPage("/governance/data/ice/sales/orders");
    const columns = await within(panel()).findByRole("table", { name: "Columns" });
    const email = await within(columns).findByRole("list", { name: "Rules of email" });
    const items = (list: HTMLElement) => within(list).getAllByRole("listitem").map((li) => li.textContent);
    expect(within(columns).getAllByRole("columnheader").map((h) => h.textContent)).toEqual([
      "Name",
      "Type",
      "Classifications",
      "Rules",
    ]);
    // By name; a row filter never shows on a column; nothing from another table, catalog or tag.
    expect(items(email)).toEqual([
      "hash_email Mask · Hash · except user bob, members of workspace Sales",
      "null_sales Mask · Nullify",
      "redact_confidential Mask · Redact",
      "redact_pii Mask · Redact",
      "star_orders Mask · Redact",
    ]);
    // `id` carries only the table's tag, which reaches it.
    expect(items(within(columns).getByRole("list", { name: "Rules of id" }))).toEqual([
      "null_sales Mask · Nullify",
      "redact_confidential Mask · Redact",
      "star_orders Mask · Redact",
    ]);
    expect(within(panel()).queryByText(/sees|can see|masked for/i)).not.toBeInTheDocument();
  });

  it("matches only path rules on a catalog that is not egret's, and never asks for its tags", async () => {
    const { tagCalls } = serveData(
      fixture({
        browse: { ...fixture().browse, "c-pg|public": ok(browseItems([{ name: "orders", kind: "table" }])), "c-pg|public|orders": ok(ORDERS_COLUMNS) },
        rules: ok([pathRule("pg_mask", { schema: "public", table: "orders", column: "email" }, HASH, "c-pg"), R.tagOnTable]),
      }),
    );
    renderPage("/governance/data/pg/public/orders");
    const columns = await within(panel()).findByRole("table", { name: "Columns" });
    const email = await within(columns).findByRole("list", { name: "Rules of email" });
    expect(within(email).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["pg_mask Mask · Hash"]);
    expect(tagCalls).toEqual([]);
  });

  it("says the masking rules could not be loaded, and keeps the columns", async () => {
    serveData(fixture({ rules: boom }));
    renderPage("/governance/data/ice/sales/orders");
    expect(await within(panel()).findByText("The rules could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(within(panel()).getByRole("table", { name: "Columns" })).toBeInTheDocument();
  });

  it("shows no masking rules without policy:read, and never asks for them", async () => {
    const { permCalls } = serveData(fixture({ rules: ok(ALL_RULES) }));
    renderPage("/governance/data/ice/sales/orders", { ...STEWARD, permissions: ["tag:read"] });
    const columns = await within(panel()).findByRole("table", { name: "Columns" });
    expect(within(columns).queryByRole("columnheader", { name: "Rules" })).not.toBeInTheDocument();
    expect(permCalls).toEqual([]);
  });

  it("matches a column's tags and tag rules to the column without regard to case", async () => {
    const tags: ObjectTags = { supported: true, assignments: [{ schema: "sales", table: "orders", column: "EMAIL", tag: tag("pii") }] };
    serveData(fixture({ tags: { ...fixture().tags, "c-ice|sales|orders": ok(tags) }, rules: ok([R.tagOnColumn]) }));
    renderPage("/governance/data/ice/sales/orders");
    const columns = await within(panel()).findByRole("table", { name: "Columns" });
    const tagList = await within(columns).findByRole("list", { name: "Classifications of email" });
    expect(within(tagList).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["PII"]);
    const rules = await within(columns).findByRole("list", { name: "Rules of email" });
    expect(within(rules).getAllByRole("listitem").map((li) => li.textContent)).toEqual(["redact_pii Mask · Redact"]);
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

  // Live check D1/D2: an engine answers an empty list for a name it cannot match.
  it("says an engine listed an empty level, never that it is empty, and keeps the bare fact for egret's listings", async () => {
    const viaEngine = { kind: "engine", name: "trino-2" } as const;
    serveData(
      fixture({
        browse: {
          ...fixture().browse,
          "c-pg": ok(browseItems([], viaEngine)),
          "c-pg|mixs": ok(browseItems([], viaEngine)),
          "c-pg|s": ok(browseItems([{ name: "t", kind: "table" }], viaEngine)),
          "c-pg|s|t": ok({ source: viaEngine, engines: [], columns: [] }),
        },
      }),
    );
    const caveat = "An engine can return an empty list for names it cannot match (for example mixed-case names in Trino JDBC catalogs).";
    const first = renderPage("/governance/data/pg/mixs");
    expect(await within(panel()).findByText("trino-2 listed no tables or views")).toBeInTheDocument();
    expect(within(panel()).getByText(caveat)).toBeInTheDocument();
    first.unmount();
    const second = renderPage("/governance/data/pg");
    expect(await within(panel()).findByText("trino-2 listed no schemas")).toBeInTheDocument();
    expect(within(panel()).getByText(caveat)).toBeInTheDocument();
    second.unmount();
    renderPage("/governance/data/pg/s/t");
    expect(await within(panel()).findByText("trino-2 listed no columns")).toBeInTheDocument();
    expect(within(panel()).getByText(caveat)).toBeInTheDocument();
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

describe("PermissionsTab", () => {
  const EDITOR: Me = { ...STEWARD, permissions: ["policy:read", "policy:update"] };

  function g(id: string, scope: Omit<Scope, "catalog_id">, over: Partial<GrantView> = {}, catalogId = "c-ice"): GrantView {
    return {
      id,
      effect: "allow",
      actions: ["read"],
      scope: { catalog_id: catalogId, ...scope },
      principals: { include: [{ kind: "role", ref: "analyst" }], exclude: [] },
      origin: { type: "explicit" },
      catalog_name: catalogId === "c-ice" ? "ice" : "pg",
      ...over,
    };
  }

  const ON_CATALOG = g("g-cat", {});
  const BY_ASSIGNMENT = g(
    "g-ws",
    {},
    { principals: { include: [{ kind: "workspace_members", ref: "w1" }], exclude: [] }, origin: { type: "workspace_assignment", workspace_id: "w1" } },
  );
  const ON_SCHEMA = g("g-schema", { schema: "sales" }, { effect: "deny", principals: { include: [{ kind: "group", ref: "contractors" }], exclude: [] } });
  const ON_TABLE = g("g-table", { schema: "sales", table: "orders" }, { actions: ["read", "write"], principals: { include: [{ kind: "user", ref: "alice" }], exclude: [] } });
  const ON_COLUMN = g("g-col", { schema: "sales", table: "orders", column: "email" }, { effect: "deny" });
  const UNRELATED = [
    g("u-schema", { schema: "hr" }),
    g("u-table", { schema: "sales", table: "customers" }),
    g("u-col", { schema: "sales", table: "customers", column: "email" }),
    g("u-catalog", { schema: "sales", table: "orders" }, {}, "c-pg"),
  ];
  const ALL = [ON_COLUMN, ON_TABLE, ...UNRELATED, ON_SCHEMA, BY_ASSIGNMENT, ON_CATALOG];

  const rowsOf = (name: string) =>
    within(within(panel()).getByRole("table", { name }))
      .getAllByRole("row")
      .slice(1)
      .map((r) => within(r).getAllByRole("cell").map((c) => c.textContent));

  it("lists the grants that reach a table: here, inherited with their level, and on its columns, deny marked", async () => {
    const { permCalls } = serveData(fixture({ grants: ok(ALL) }));
    renderPage("/governance/data/ice/sales/orders?tab=permissions", EDITOR);
    await within(panel()).findByRole("table", { name: "Here" });

    // The hand-written section (#4b) comes last; its count is the section's own concern.
    const headings = within(panel()).getAllByRole("heading", { level: 2 }).map((h) => h.textContent ?? "");
    expect(headings.at(-1)).toMatch(/^Manual policies/);
    expect(headings.slice(0, -1)).toEqual([
      "Here",
      "Inherited",
      "On columns",
    ]);
    expect(rowsOf("Here")).toEqual([["user alice can read and write ice.sales.orders", "Allow", ""]]);
    // Catalog before schema; the assignment grant names its workspace, in its sentence too (live check D5).
    expect(rowsOf("Inherited")).toEqual([
      ["members of workspace Finance can read ice.*Set by workspace assignment · Finance", "Catalog", "Allow", ""],
      ["role analyst can read ice.*", "Catalog", "Allow", ""],
      ["group contractors cannot read ice.sales.*", "Schema", "Deny", ""],
    ]);
    expect(rowsOf("On columns")).toEqual([["role analyst cannot read ice.sales.orders.email", "email", "Deny", ""]]);
    // Nothing unrelated, and nothing from another catalog.
    // On text content, not `queryByText`: a sentence's principal names are links, so no one
    // element carries the whole sentence and `queryByText` would pass whatever the panel lists.
    expect(panel().textContent).not.toMatch(/customers|hr\./);
    expect(panel().textContent).not.toMatch(/role analyst can read pg/);
    expect(permCalls.map((c) => c.call)).toEqual(["GET /perm/grants"]);
  });

  it("links each principal name to its People page; a workspace's members stay text (spec P14)", async () => {
    serveData(fixture({ grants: ok(ALL) }));
    renderPage("/governance/data/ice/sales/orders?tab=permissions", EDITOR);
    const here = await within(panel()).findByRole("table", { name: "Here" });
    expect(within(here).getByRole("link", { name: "alice" })).toHaveAttribute("href", "/governance/people/users/by-name/alice");
    const inherited = within(panel()).getByRole("table", { name: "Inherited" });
    expect(within(inherited).getByRole("link", { name: "analyst" })).toHaveAttribute("href", "/governance/people/roles/analyst");
    expect(within(inherited).getByRole("link", { name: "contractors" })).toHaveAttribute(
      "href",
      "/governance/people/groups/contractors",
    );
    expect(within(inherited).queryByRole("link", { name: /Finance/ })).not.toBeInTheDocument();
  });

  it("gives an explicit grant Edit and Delete, and a workspace-assignment grant neither", async () => {
    serveData(fixture({ grants: ok([ON_CATALOG, BY_ASSIGNMENT]) }));
    renderPage("/governance/data/ice?tab=permissions", EDITOR);
    await within(panel()).findByRole("table", { name: "Here" });
    expect(within(panel()).getByRole("button", { name: "Edit role analyst can read ice.*" })).toBeInTheDocument();
    expect(within(panel()).getByRole("button", { name: "Delete role analyst can read ice.*" })).toBeInTheDocument();
    expect(within(panel()).queryByRole("button", { name: /workspace_members/ })).not.toBeInTheDocument();
    expect(within(panel()).getByText("Set by workspace assignment · Finance")).toBeInTheDocument();
  });

  it("offers no Edit or Delete without policy:update", async () => {
    serveData(fixture({ grants: ok([ON_CATALOG]) }));
    renderPage("/governance/data/ice?tab=permissions");
    await within(panel()).findByRole("table", { name: "Here" });
    expect(within(panel()).queryByRole("button")).not.toBeInTheDocument();
  });

  it("lists the grants on a catalog, then those on its schemas, tables and columns, each named", async () => {
    serveData(fixture({ grants: ok(ALL) }));
    renderPage("/governance/data/ice?tab=permissions", EDITOR);
    await within(panel()).findByRole("table", { name: "Here" });
    expect(rowsOf("Here").map((r) => r[0])).toEqual([
      "members of workspace Finance can read ice.*Set by workspace assignment · Finance",
      "role analyst can read ice.*",
    ]);
    expect(within(panel()).queryByRole("table", { name: "Inherited" })).not.toBeInTheDocument();
    // Below the catalog, by what each is on; nothing from another catalog.
    expect(rowsOf("Inside this catalog").map((r) => [r[0], r[1]])).toEqual([
      ["role analyst can read ice.hr.*", "hr"],
      ["group contractors cannot read ice.sales.*", "sales"],
      ["role analyst can read ice.sales.customers", "sales.customers"],
      ["role analyst can read ice.sales.customers.email", "sales.customers.email"],
      ["user alice can read and write ice.sales.orders", "sales.orders"],
      ["role analyst cannot read ice.sales.orders.email", "sales.orders.email"],
    ]);
  });

  // Live check D4: a grant made from a schema's page on a table, or on `table = "*"`, was listed nowhere on it.
  it("lists on a schema the grants on its tables and columns, and on a table pattern below it", async () => {
    const star = g("g-star", { schema: "sales", table: "*" });
    serveData(fixture({ grants: ok([...ALL, star]) }));
    renderPage("/governance/data/ice/sales?tab=permissions", EDITOR);
    await within(panel()).findByRole("table", { name: "Here" });
    // The hand-written section (#4b) comes last; its count is the section's own concern.
    const headings = within(panel()).getAllByRole("heading", { level: 2 }).map((h) => h.textContent ?? "");
    expect(headings.at(-1)).toMatch(/^Manual policies/);
    expect(headings.slice(0, -1)).toEqual([
      "Here",
      "Inherited",
      "Inside this schema",
      "Matching a pattern",
    ]);
    expect(rowsOf("Here").map((r) => r[0])).toEqual(["group contractors cannot read ice.sales.*"]);
    expect(rowsOf("Inside this schema").map((r) => [r[0], r[1]])).toEqual([
      ["role analyst can read ice.sales.customers", "customers"],
      ["role analyst can read ice.sales.customers.email", "customers.email"],
      ["user alice can read and write ice.sales.orders", "orders"],
      ["role analyst cannot read ice.sales.orders.email", "orders.email"],
    ]);
    expect(rowsOf("Matching a pattern")).toEqual([["role analyst can read ice.sales.“*”", "table “*”", "Allow", ""]]);
    // The pattern grant can be edited and deleted where it is listed.
    expect(within(panel()).getByRole("button", { name: "Delete role analyst can read ice.sales.“*”" })).toBeInTheDocument();
    expect(within(panel()).queryByText(/ice\.hr/)).not.toBeInTheDocument();
  });

  it("lists a pattern grant on the page its Open in Data link opens", async () => {
    const star = g("g-star", { schema: "sales", table: "*" });
    const link = scopeToDataLink(star.scope, new Map([["c-ice", "ice"]]), "permissions");
    expect(link).toEqual({ to: "/governance/data/ice/sales?tab=permissions", stoppedAtPattern: true });
    serveData(fixture({ grants: ok([star]) }));
    renderPage(link!.to);
    expect(await within(panel()).findByRole("table", { name: "Matching a pattern" })).toBeInTheDocument();
    expect(rowsOf("Matching a pattern").map((r) => r[0])).toEqual(["role analyst can read ice.sales.“*”"]);
  });

  const EGRET_ONLY =
    "egret grants are above; manual policies below. To see who can actually read, use Who can read?";

  it("says no egret grant reaches the object, and points at the manual policies below", async () => {
    serveData(fixture({ grants: ok(UNRELATED) }));
    renderPage("/governance/data/ice/sales/orders?tab=permissions");
    expect(await within(panel()).findByText("No egret grant reaches this table")).toBeInTheDocument();
    expect(within(panel()).getByText(EGRET_ONLY)).toBeInTheDocument();
  });

  it("says, below the grants it lists, that manual policies follow", async () => {
    serveData(fixture({ grants: ok([ON_TABLE]) }));
    renderPage("/governance/data/ice/sales/orders?tab=permissions");
    await within(panel()).findByRole("table", { name: "Here" });
    expect(within(panel()).getByText(EGRET_ONLY)).toBeInTheDocument();
  });

  // I1: the engines' service-defs ignore case on every resource level.
  it("lists on the orders page a deny grant typed on sales.Orders", async () => {
    const upper = g("g-upper", { schema: "sales", table: "Orders" }, { effect: "deny" });
    serveData(fixture({ grants: ok([upper]) }));
    renderPage("/governance/data/ice/sales/orders?tab=permissions");
    await within(panel()).findByRole("table", { name: "Here" });
    expect(rowsOf("Here")).toEqual([["role analyst cannot read ice.sales.Orders", "Deny"]]);
    expect(within(panel()).queryByText(/No egret grant reaches/)).not.toBeInTheDocument();
  });

  it("says the grants could not be loaded, never that there are none", async () => {
    serveData(fixture({ grants: boom }));
    renderPage("/governance/data/ice/sales/orders?tab=permissions");
    expect(await within(panel()).findByText("The grants could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(within(panel()).queryByText(/No egret grant reaches/)).not.toBeInTheDocument();
  });

  it("tells a viewer without policy:read why, and never asks for the grants", async () => {
    const { permCalls } = serveData(fixture({ grants: ok(ALL) }));
    renderPage("/governance/data/ice/sales/orders?tab=permissions", { ...STEWARD, permissions: ["tag:read"] });
    expect(
      await within(panel()).findByText("Reading grants needs the policy:read permission, which this account does not have."),
    ).toBeInTheDocument();
    expect(permCalls).toEqual([]);
  });

  it("opens Grant access on this object's scope", async () => {
    serveData(fixture({ grants: ok([]) }));
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders", EDITOR);
    await user.click(await within(header()).findByRole("button", { name: "Grant access" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "New grant" })).toBeInTheDocument();
    // The picker names the catalog from the Governance list, which policy:read can read.
    await waitFor(() => expect(within(dialog).getByRole("combobox", { name: "Catalog" })).toHaveTextContent("ice"));
    expect(within(dialog).getByLabelText("Schema")).toHaveValue("sales");
    expect(within(dialog).getByLabelText("Table")).toHaveValue("orders");
    expect(within(dialog).getByLabelText("Column")).toHaveValue("");
    expect(within(dialog).getByText(/Preview: read on ice\.sales\.orders/)).toBeInTheDocument();
  });

  it("opens Grant access on a catalog with only the catalog set", async () => {
    serveData(fixture({ grants: ok([]) }));
    const user = userEvent.setup();
    renderPage("/governance/data/ice", EDITOR);
    await user.click(await within(header()).findByRole("button", { name: "Grant access" }));
    const dialog = await screen.findByRole("dialog");
    await waitFor(() => expect(within(dialog).getByRole("combobox", { name: "Catalog" })).toHaveTextContent("ice"));
    expect(within(dialog).getByLabelText("Schema")).toHaveValue("");
  });

  it("shows no Grant access until the catalog is known", async () => {
    serveData(fixture({ catalogs: "never" }));
    renderPage("/governance/data/ice/sales/orders", EDITOR);
    expect(within(header()).queryByRole("button", { name: "Grant access" })).not.toBeInTheDocument();
  });

  it("edits an explicit grant in GrantDialog, then refetches the tab and confirms", async () => {
    const edited = { ...ON_TABLE, effect: "deny" as const };
    const { permCalls } = serveData(
      fixture({
        grants: [ok([ON_TABLE]), ok([edited])],
        preview: ok({ affected: [], compile: { services: [], created: 0, updated: 0, deleted: 0, unchanged: 0, skipped: [] } }),
      }),
    );
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?tab=permissions", EDITOR);
    await user.click(await within(panel()).findByRole("button", { name: "Edit user alice can read and write ice.sales.orders" }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "Edit grant" })).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Table")).toHaveValue("orders");
    await user.click(within(dialog).getByRole("button", { name: "Preview" }));
    const save = within(dialog).getByRole("button", { name: "Save changes" });
    await waitFor(() => expect(save).toBeEnabled());
    await user.click(save);

    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(permCalls.find((c) => c.call === "PATCH /perm/grants/g-table")?.body).toEqual({
      effect: "allow",
      actions: ["read", "write"],
      scope: ON_TABLE.scope,
      principals: ON_TABLE.principals,
    });
    expect(toast.success).toHaveBeenCalledWith("Grant updated.");
    // The tab read the grants again and shows what the server now holds.
    expect(await within(panel()).findByText("Deny")).toBeInTheDocument();
    expect(permCalls.filter((c) => c.call === "GET /perm/grants")).toHaveLength(2);
  });

  it("deletes an explicit grant after confirmation, then refetches the tab and confirms", async () => {
    const { permCalls } = serveData(fixture({ grants: [ok([ON_TABLE]), ok([])] }));
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?tab=permissions", EDITOR);
    await user.click(await within(panel()).findByRole("button", { name: "Delete user alice can read and write ice.sales.orders" }));

    const dialog = await screen.findByRole("alertdialog");
    await user.type(within(dialog).getByRole("textbox"), "delete");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));

    expect(await within(panel()).findByText("No egret grant reaches this table")).toBeInTheDocument();
    expect(permCalls.map((c) => c.call)).toEqual(["GET /perm/grants", "DELETE /perm/grants/g-table", "GET /perm/grants"]);
    expect(toast.success).toHaveBeenCalledWith("Grant deleted.");
  });
  it("keeps a pattern grant apart from an inherited one, in its group and in its sentence", async () => {
    const pattern = g("g-star", { schema: "sales", table: "*" });
    serveData(fixture({ grants: ok([pattern, g("g-sales", { schema: "sales" })]) }));
    renderPage("/governance/data/ice/sales/orders?tab=permissions", EDITOR);
    await within(panel()).findByRole("table", { name: "Matching a pattern" });

    expect(within(panel()).queryByRole("table", { name: "Here" })).not.toBeInTheDocument();
    expect(rowsOf("Inherited")).toEqual([["role analyst can read ice.sales.*", "Schema", "Allow", ""]]);
    expect(rowsOf("Matching a pattern")).toEqual([["role analyst can read ice.sales.“*”", "table “*”", "Allow", ""]]);
  });

  it("says how far a pattern grant reaches before deleting it", async () => {
    serveData(fixture({ grants: ok([g("g-star", { schema: "sales", table: "ord*" })]) }));
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?tab=permissions", EDITOR);
    await user.click(await within(panel()).findByRole("button", { name: "Delete role analyst can read ice.sales.“ord*”" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/This grant covers every table matching “ord\*”, not one object\./)).toBeInTheDocument();
  });

  it("reads a grant's excluded principals into its sentence", async () => {
    serveData(
      fixture({
        grants: ok([
          g("g-ex", { schema: "sales", table: "orders" }, { principals: { include: [{ kind: "role", ref: "analyst" }], exclude: [{ kind: "user", ref: "bob" }] } }),
        ]),
      }),
    );
    renderPage("/governance/data/ice/sales/orders?tab=permissions");
    await within(panel()).findByRole("table", { name: "Here" });
    expect(rowsOf("Here")[0][0]).toBe("role analyst can read ice.sales.orders except user bob");
  });

  it.each([
    [{ status: 403, body: { error: "forbidden" } }, "This account cannot do that: it needs the policy:update permission."],
    [{ status: 409, body: { error: "A rule overlap would follow from this delete." } }, "A rule overlap would follow from this delete."],
  ])("shows a refused delete's reason and keeps the grant (%j)", async (refusal, message) => {
    const { permCalls } = serveData(fixture({ grants: ok([ON_TABLE]), grantWrite: refusal }));
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?tab=permissions", EDITOR);
    await user.click(await within(panel()).findByRole("button", { name: "Delete user alice can read and write ice.sales.orders" }));
    const dialog = await screen.findByRole("alertdialog");
    await user.type(within(dialog).getByRole("textbox"), "delete");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));

    await waitFor(() => expect(toast.error).toHaveBeenCalledWith(message));
    expect(toast.success).not.toHaveBeenCalledWith("Grant deleted.");
    expect(permCalls.map((c) => c.call)).toContain("DELETE /perm/grants/g-table");
    expect(within(panel()).getByRole("table", { name: "Here" })).toBeInTheDocument();
  });

  it("creates a grant from Grant access, then refetches the tab and confirms", async () => {
    const created = g("g-new", { schema: "sales", table: "orders" });
    const { permCalls } = serveData(
      fixture({
        grants: [ok([]), ok([created])],
        preview: ok({ affected: [], compile: { services: [], created: 0, updated: 0, deleted: 0, unchanged: 0, skipped: [] } }),
        grantWrite: { status: 201, body: { id: "g-new" } },
      }),
    );
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?tab=permissions", EDITOR);
    expect(await within(panel()).findByText("No egret grant reaches this table")).toBeInTheDocument();

    await user.click(await within(header()).findByRole("button", { name: "Grant access" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("combobox", { name: "Include kind" }));
    await user.click(await screen.findByRole("option", { name: "role" }));
    await user.type(within(dialog).getByRole("textbox", { name: "Include role name" }), "analyst");
    await user.click(within(dialog).getByRole("button", { name: "Add" }));
    await user.click(within(dialog).getByRole("button", { name: "Preview" }));
    const create = within(dialog).getByRole("button", { name: "Create" });
    await waitFor(() => expect(create).toBeEnabled());
    await user.click(create);

    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(permCalls.find((c) => c.call === "POST /perm/grants")?.body).toEqual({
      effect: "allow",
      actions: ["read"],
      scope: { catalog_id: "c-ice", schema: "sales", table: "orders" },
      principals: { include: [{ kind: "role", ref: "analyst" }], exclude: [] },
    });
    expect(toast.success).toHaveBeenCalledWith("Grant created.");
    await within(panel()).findByRole("table", { name: "Here" });
    expect(rowsOf("Here")[0][0]).toBe("role analyst can read ice.sales.orders");
    expect(permCalls.filter((c) => c.call === "GET /perm/grants")).toHaveLength(2);
  });
});

describe("MasksFiltersTab", () => {
  const EDITOR: Me = { ...STEWARD, permissions: ["policy:read", "policy:update"] };
  const rowsOf = (name: string) =>
    within(within(panel()).getByRole("table", { name }))
      .getAllByRole("row")
      .slice(1)
      .map((r) => within(r).getAllByRole("cell").map((c) => c.textContent));

  it("lists the masks and row filters that select a table, grouped by how they reach it", async () => {
    const { permCalls } = serveData(fixture({ rules: ok(ALL_RULES) }));
    renderPage("/governance/data/ice/sales/orders?tab=masks", EDITOR);
    await within(panel()).findByRole("table", { name: "Here" });

    // The hand-written section (#4b) comes last; its count is the section's own concern.
    const headings = within(panel()).getAllByRole("heading", { level: 2 }).map((h) => h.textContent ?? "");
    expect(headings.at(-1)).toMatch(/^Manual policies/);
    expect(headings.slice(0, -1)).toEqual([
      "Here",
      "Inherited",
      "On columns",
      "By classification",
      "Matching a pattern",
    ]);
    // Each row names its exemptions ("—" for none).
    expect(rowsOf("Here")).toEqual([["us_onlyice.sales.orders", "Row filter · region = 'us'", "group auditors, user alice", ""]]);
    const here = within(panel()).getByRole("table", { name: "Here" });
    expect(within(here).getByRole("link", { name: "auditors" })).toHaveAttribute("href", "/governance/people/groups/auditors");
    expect(within(here).getByRole("link", { name: "alice" })).toHaveAttribute("href", "/governance/people/users/by-name/alice");
    // Masks before row filters; then catalog before schema.
    expect(rowsOf("Inherited")).toEqual([
      ["null_salesice.sales.*", "Mask · Nullify", "Schema", "—", ""],
      ["ice_filterice.*", "Row filter · region = 'us'", "Catalog", "—", ""],
    ]);
    expect(rowsOf("On columns")).toEqual([["hash_emailice.sales.orders.email", "Mask · Hash", "email", "—", ""]]);
    // A tag on the table reaches the table; a mask by a column's tag reaches that column; a row filter by it reaches nothing.
    expect(rowsOf("By classification")).toEqual([
      ["redact_confidentialClassification CONFIDENTIAL", "Mask · Redact", "On the table", "—", ""],
      ["redact_piiClassification PII", "Mask · Redact", "On column email", "—", ""],
    ]);
    expect(rowsOf("Matching a pattern")).toEqual([["star_ordersice.sales.“ord*”", "Mask · Redact", "table “ord*”", "—", ""]]);
    expect(within(panel()).queryByText(/pii_filter|hash_customers|pg_mask|redact_secret/)).not.toBeInTheDocument();
    // The rules, then whether the listed ones are enforced (one request for every row).
    expect(permCalls.map((c) => c.call)).toEqual(["GET /perm/rules", "GET /perm/rules/enforcement"]);
  });

  it("says on a rule's row that no engine enforces it, once the server says so", async () => {
    const { permCalls } = serveData(
      fixture({
        rules: ok(ALL_RULES),
        ruleEnforcement: ok(ALL_RULES.map((r) => ({ ruleId: r.id, enforcedBy: r.name === "us_only" ? 0 : 1, notEnforced: r.name === "us_only" ? "no_profile" : null }))),
      }),
    );
    renderPage("/governance/data/ice/sales/orders?tab=masks", EDITOR);
    const here = await within(panel()).findByRole("table", { name: "Here" });
    await waitFor(() => expect(within(here).getAllByRole("row")[1]).toHaveTextContent("Not enforced yet — its engines have no policy profile"));
    expect(within(within(panel()).getByRole("table", { name: "Inherited" })).queryByText(/Not enforced/)).not.toBeInTheDocument();
    expect(permCalls.filter((c) => c.call === "GET /perm/rules/enforcement")).toHaveLength(1);
  });

  it("links each rule to its page, with the way back to this Data page", async () => {
    serveData(fixture({ rules: ok(ALL_RULES) }));
    renderPage("/governance/data/ice/sales/orders?tab=masks", EDITOR);
    const here = await within(panel()).findByRole("table", { name: "Here" });
    const link = within(here).getByRole("link", { name: "us_only" });
    const href = link.getAttribute("href") ?? "";
    expect(href.startsWith("/governance/rules/")).toBe(true);
    const from = new URLSearchParams(href.split("?")[1]).get("from");
    expect(from).toBe(`data:${encodeURIComponent("/governance/data/ice/sales/orders?tab=masks")}`);
  });

  it("lists a rule on another tag that shares the tag's name, since the engine matches names", async () => {
    // The rule selects key `pii_eu`, also named PII; `email` carries key `pii`, named PII.
    serveData(fixture({ rules: ok([tagRule("redact_pii_eu", "pii_eu", REDACT, "PII")]) }));
    renderPage("/governance/data/ice/sales/orders?tab=masks");
    await within(panel()).findByRole("table", { name: "By classification" });
    expect(rowsOf("By classification")).toEqual([["redact_pii_euClassification PII", "Mask · Redact", "On column email", "—"]]);
  });

  it("names a tag rule whose tag no longer exists, rather than dropping it silently", async () => {
    serveData(fixture({ rules: ok([tagRule("orphan", "pii", REDACT, null), R.tableFilter]) }));
    renderPage("/governance/data/ice/sales/orders?tab=masks");
    expect(await within(panel()).findByText("Rule orphan uses a classification that no longer exists, so it applies to nothing.")).toBeInTheDocument();
    expect(within(panel()).queryByRole("table", { name: "By classification" })).not.toBeInTheDocument();
  });

  it("lists the rules on a catalog, then those inside it and on a pattern below it, and no tag rule", async () => {
    const { tagCalls } = serveData(fixture({ rules: ok(ALL_RULES) }));
    renderPage("/governance/data/ice?tab=masks", EDITOR);
    await within(panel()).findByRole("table", { name: "Here" });
    expect(rowsOf("Here").map((r) => r[0])).toEqual(["ice_filterice.*"]);
    // The hand-written section (#4b) comes last; its count is the section's own concern.
    const headings = within(panel()).getAllByRole("heading", { level: 2 }).map((h) => h.textContent ?? "");
    expect(headings.at(-1)).toMatch(/^Manual policies/);
    expect(headings.slice(0, -1)).toEqual([
      "Here",
      "Inside this catalog",
      "Matching a pattern",
    ]);
    // Masks before row filters, then by what each is on.
    expect(rowsOf("Inside this catalog").map((r) => [r[0], r[2]])).toEqual([
      ["null_salesice.sales.*", "sales"],
      ["hash_customersice.sales.customers.email", "sales.customers.email"],
      ["hash_emailice.sales.orders.email", "sales.orders.email"],
      ["us_onlyice.sales.orders", "sales.orders"],
    ]);
    expect(rowsOf("Matching a pattern").map((r) => r[0])).toEqual(["star_ordersice.sales.“ord*”"]);
    expect(within(panel()).queryByText(/pg_mask|redact_/)).not.toBeInTheDocument();
    expect(tagCalls).toEqual([]);
    // M1: and says where the tag rules are.
    expect(within(panel()).getByText("Rules on classifications are listed on each table's page.")).toBeInTheDocument();
  });

  it("lists on a schema the rules on its tables and columns", async () => {
    serveData(fixture({ rules: ok(ALL_RULES) }));
    renderPage("/governance/data/ice/sales?tab=masks", EDITOR);
    await within(panel()).findByRole("table", { name: "Inside this schema" });
    expect(rowsOf("Inside this schema").map((r) => [r[0], r[2]])).toEqual([
      ["hash_customersice.sales.customers.email", "customers.email"],
      ["hash_emailice.sales.orders.email", "orders.email"],
      ["us_onlyice.sales.orders", "orders"],
    ]);
    expect(within(panel()).getByText("Rules on classifications are listed on each table's page.")).toBeInTheDocument();
  });

  it("names a workspace exemption by the workspace's name", async () => {
    const exempted = { ...R.tableFilter, exempt: { include: [{ kind: "workspace_members" as const, ref: "w1" }], exclude: [] } };
    serveData(fixture({ rules: ok([exempted]) }));
    renderPage("/governance/data/ice/sales/orders?tab=masks");
    await within(panel()).findByRole("table", { name: "Here" });
    expect(rowsOf("Here")).toEqual([["us_onlyice.sales.orders", "Row filter · region = 'us'", "members of workspace Finance"]]);
    // The workspace is named, not linked; the only link is the rule's own page.
    expect(within(within(panel()).getByRole("table", { name: "Here" })).getAllByRole("link").map((a) => a.textContent)).toEqual(["us_only"]);
    // A table page lists its tag rules itself, so it does not point elsewhere.
    expect(within(panel()).queryByText("Rules on classifications are listed on each table's page.")).not.toBeInTheDocument();
  });

  it("lists no tag rule on a table of a catalog that is not egret's, and never asks for its tags", async () => {
    const { tagCalls } = serveData(
      fixture({
        browse: { ...fixture().browse, "c-pg|public": ok(browseItems([{ name: "orders", kind: "table" }])), "c-pg|public|orders": ok(ORDERS_COLUMNS) },
        rules: ok([pathRule("pg_filter", { schema: "public", table: "orders" }, US_ONLY, "c-pg"), R.tagOnTable]),
      }),
    );
    renderPage("/governance/data/pg/public/orders?tab=masks", EDITOR);
    await within(panel()).findByRole("table", { name: "Here" });
    expect(rowsOf("Here").map((r) => r[0])).toEqual(["pg_filterpg.public.orders"]);
    expect(within(panel()).queryByRole("table", { name: "By classification" })).not.toBeInTheDocument();
    expect(tagCalls).toEqual([]);
  });

  it("says no rule selects the object", async () => {
    serveData(fixture({ rules: ok([R.otherTable, R.otherCatalog, R.otherTag]) }));
    renderPage("/governance/data/ice/sales/orders?tab=masks");
    expect(await within(panel()).findByText("No rule applies to this table.")).toBeInTheDocument();
  });

  it("says the rules could not be loaded, never that there are none", async () => {
    serveData(fixture({ rules: boom }));
    renderPage("/governance/data/ice/sales/orders?tab=masks");
    expect(await within(panel()).findByText("The rules could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(within(panel()).queryByText(/No rule applies/)).not.toBeInTheDocument();
  });

  it("says the tags could not be loaded, and still lists the path rules", async () => {
    serveData(fixture({ rules: ok(ALL_RULES), tags: { "c-ice|sales|orders": boom } }));
    renderPage("/governance/data/ice/sales/orders?tab=masks");
    expect(await within(panel()).findByText("The classifications could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(within(panel()).getByRole("table", { name: "Here" })).toBeInTheDocument();
    expect(within(panel()).queryByRole("table", { name: "By classification" })).not.toBeInTheDocument();
  });

  it("tells a viewer without policy:read why, and never asks for the rules", async () => {
    const { permCalls } = serveData(fixture({ rules: ok(ALL_RULES) }));
    renderPage("/governance/data/ice/sales/orders?tab=masks", { ...STEWARD, permissions: ["tag:read"] });
    expect(
      await within(panel()).findByText("Reading rules needs the policy:read permission, which this account does not have."),
    ).toBeInTheDocument();
    expect(permCalls).toEqual([]);
  });

  it("offers no Add rule, Edit or Delete without policy:update", async () => {
    serveData(fixture({ rules: ok(ALL_RULES) }));
    renderPage("/governance/data/ice/sales/orders?tab=masks");
    await within(panel()).findByRole("table", { name: "Here" });
    expect(within(panel()).queryByRole("button")).not.toBeInTheDocument();
  });

  it("opens Add rule in RuleDialog on a classification its columns carry, the table's path one click away (spec C8.3, Task 13)", async () => {
    serveData(fixture({ rules: ok([]) }));
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?tab=masks", EDITOR);
    await user.click(await within(panel()).findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "New rule" })).toBeInTheDocument();
    // `email` carries PII: the rule starts on it. The table's whole-table classification is never offered.
    expect(within(dialog).getByRole("combobox", { name: "Kind" })).toHaveTextContent("Mask a column");
    expect(within(dialog).getByRole("combobox", { name: "Applies to" })).toHaveTextContent("Classification");
    expect(within(dialog).getByLabelText(/^Name/)).toHaveValue("Mask PII");
    expect(within(dialog).queryByRole("button", { name: /CONFIDENTIAL/ })).not.toBeInTheDocument();
    // The table alone: a mask on its path, its Column left empty, which masks every column.
    await user.click(within(dialog).getByRole("button", { name: "Or apply it to table orders only" }));
    expect(within(dialog).getByRole("combobox", { name: "Applies to" })).toHaveTextContent("Path");
    await waitFor(() => expect(within(dialog).getByRole("combobox", { name: /^Catalog/ })).toHaveTextContent("ice"));
    expect(within(dialog).getByLabelText("Schema")).toHaveValue("sales");
    expect(within(dialog).getByLabelText(/^Table/)).toHaveValue("orders");
    expect(within(dialog).getByLabelText("Column")).toHaveValue("");
    expect(within(dialog).getByLabelText("Column")).toHaveAccessibleDescription("Leave Column empty to mask every column of the table.");
    expect(within(dialog).getByText("Applies to ice.sales.orders")).toBeInTheDocument();
  });

  it("opens Add rule on a schema as a mask, with only the schema set", async () => {
    serveData(fixture({ rules: ok([]) }));
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales?tab=masks", EDITOR);
    await user.click(await within(panel()).findByRole("button", { name: "Add rule" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("combobox", { name: "Kind" })).toHaveTextContent("Mask a column");
    expect(within(dialog).getByLabelText("Schema")).toHaveValue("sales");
    expect(within(dialog).getByLabelText("Table")).toHaveValue("");
  });

  it("edits a rule in RuleDialog", async () => {
    serveData(fixture({ rules: ok([R.tableFilter]) }));
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?tab=masks", EDITOR);
    await user.click(await within(panel()).findByRole("button", { name: "Edit us_only" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "Edit rule" })).toBeInTheDocument();
    expect(within(dialog).getByLabelText(/^Name/)).toHaveValue("us_only");
  });

  it("deletes a rule after confirmation, then refetches the tab and confirms", async () => {
    const { permCalls } = serveData(fixture({ rules: [ok([R.tableFilter]), ok([])] }));
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?tab=masks", EDITOR);
    await user.click(await within(panel()).findByRole("button", { name: "Delete us_only" }));
    const dialog = await screen.findByRole("alertdialog");
    // A rule's delete asks for its name (spec C11, the one delete convention).
    await user.type(within(dialog).getByRole("textbox"), "us_only");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));

    expect(await within(panel()).findByText("No rule applies to this table.")).toBeInTheDocument();
    // Enforcement reads (`GET /perm/rules/enforcement`) follow the list; this test is about the writes and the list.
    expect(permCalls.map((c) => c.call).filter((c) => c !== "GET /perm/rules/enforcement")).toEqual([
      "GET /perm/rules",
      "DELETE /perm/rules/us_only",
      "GET /perm/rules",
    ]);
    expect(toast.success).toHaveBeenCalledWith("Rule us_only deleted.");
  });

  it("says how far a pattern rule reaches before deleting it", async () => {
    serveData(fixture({ rules: ok([R.pattern]) }));
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?tab=masks", EDITOR);
    await user.click(await within(panel()).findByRole("button", { name: "Delete star_orders" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/This rule selects every table matching “ord\*”, not one object\./)).toHaveTextContent(
      "This rule selects every table matching “ord*”, not one object. It is permanently removed, and the engines that enforce it stop applying it.",
    );
    expect(dialog).not.toHaveTextContent(/profile|recompile/);
  });

  it("shows a refused delete's reason and keeps the rule", async () => {
    serveData(fixture({ rules: ok([R.tableFilter]), ruleWrite: { status: 403, body: { error: "forbidden" } } }));
    const user = userEvent.setup();
    renderPage("/governance/data/ice/sales/orders?tab=masks", EDITOR);
    await user.click(await within(panel()).findByRole("button", { name: "Delete us_only" }));
    const dialog = await screen.findByRole("alertdialog");
    // A rule's delete asks for its name (spec C11, the one delete convention).
    await user.type(within(dialog).getByRole("textbox"), "us_only");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith("This account cannot do that: it needs the policy:update permission."),
    );
    expect(within(panel()).getByRole("table", { name: "Here" })).toBeInTheDocument();
  });
});

/** #4b spec §6 (F12): the hand-written policies on the Data pages, from route A. */
describe("manual policies on an object's tabs", () => {
  const HW = { "c-ice|sales|orders|": handwrittenAnswer("c-ice") };
  const hwSection = () => within(panel()).getByRole("region", { name: /^Manual policies/ });

  it("Permissions lists the access rows only, below the grants", async () => {
    const calls = serveData(fixture({ grants: ok([]), handwritten: HW }));
    renderPage("/governance/data/ice/sales/orders?tab=permissions");
    expect(await within(panel()).findByRole("link", { name: "hw-access" })).toBeInTheDocument();
    expect(within(hwSection()).queryByRole("link", { name: "hw-mask" })).not.toBeInTheDocument();
    expect(within(hwSection()).queryByRole("link", { name: "hw-filter" })).not.toBeInTheDocument();
    expect(calls.handwrittenCalls).toContain("c-ice|sales|orders|");
  });

  it("Rules lists the masking and row-filter rows only", async () => {
    serveData(fixture({ rules: ok([]), handwritten: HW }));
    renderPage("/governance/data/ice/sales/orders?tab=masks");
    expect(await within(panel()).findByRole("link", { name: "hw-mask" })).toBeInTheDocument();
    expect(within(hwSection()).getByRole("link", { name: "hw-filter" })).toBeInTheDocument();
    expect(within(hwSection()).queryByRole("link", { name: "hw-access" })).not.toBeInTheDocument();
  });

  it("shows a failing read as an error, never as none", async () => {
    serveData(fixture({ grants: ok([]), handwritten: { "c-ice|sales|orders|": { status: 500, body: { error: "boom" } } } }));
    renderPage("/governance/data/ice/sales/orders?tab=permissions");
    expect(await within(panel()).findByText(/The manual policies could not be loaded/)).toBeInTheDocument();
    expect(within(panel()).queryByText("No manual policy reaches this object.")).not.toBeInTheDocument();
  });
});

describe("Remove a classification in place", () => {
  const EDITOR: Me = { ...STEWARD, permissions: ["policy:read", "tag:read", "catalog_registry:update"] };

  it("removes a column's own classification after a confirm, and refetches", async () => {
    const user = userEvent.setup();
    const { tagWrites, tagCalls } = serveData(fixture({ tagCatalogue: ok([]) }));
    renderPage("/governance/data/ice/sales/orders", EDITOR);
    const columns = await within(panel()).findByRole("table", { name: "Columns" });
    await user.click(await within(columns).findByRole("button", { name: "Remove PII from ice.sales.orders.email" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Remove PII from ice.sales.orders.email?" });
    const before = tagCalls.length;
    await user.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(tagWrites).toEqual([
        { method: "DELETE", warehouseId: "wh-c-ice", body: { schema: "sales", table: "orders", column: "email", tag_id: "t-pii" } },
      ]),
    );
    await waitFor(() => expect(tagCalls.length).toBeGreaterThan(before));
    expect(toast.success).toHaveBeenCalledWith("Removed PII from ice.sales.orders.email.");
  });

  it("offers no Remove without catalog_registry:update", async () => {
    serveData(fixture());
    renderPage("/governance/data/ice/sales/orders");
    const columns = await within(panel()).findByRole("table", { name: "Columns" });
    await within(columns).findByText("PII");
    expect(within(columns).queryByRole("button", { name: /^Remove / })).not.toBeInTheDocument();
    expect(within(header()).queryByRole("button", { name: /^Remove / })).not.toBeInTheDocument();
  });

  it("never offers Remove on a chip a column inherits from its table", async () => {
    serveData(fixture({ tagCatalogue: ok([]) }));
    renderPage("/governance/data/ice/sales/orders", EDITOR);
    const columns = await within(panel()).findByRole("table", { name: "Columns" });
    await within(columns).findByRole("button", { name: "Remove PII from ice.sales.orders.email" });
    expect(within(columns).queryByRole("button", { name: /^Remove CONFIDENTIAL/ })).not.toBeInTheDocument();
  });

  it("removes the table's classification from the header, naming the rules that stop applying and the columns", async () => {
    const user = userEvent.setup();
    const { tagWrites } = serveData(fixture({ tagCatalogue: ok([]), rules: ok(ALL_RULES) }));
    renderPage("/governance/data/ice/sales/orders", EDITOR);
    await user.click(await within(header()).findByRole("button", { name: "Remove CONFIDENTIAL from ice.sales.orders" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Remove CONFIDENTIAL from ice.sales.orders?" });
    expect(within(dialog).getByText("Rules that apply to CONFIDENTIAL stop applying here.")).toBeInTheDocument();
    expect(await within(dialog).findByText("redact_confidential")).toBeInTheDocument();
    expect(within(dialog).getByText("Its columns stop inheriting it.")).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(tagWrites).toEqual([
        { method: "DELETE", warehouseId: "wh-c-ice", body: { schema: "sales", table: "orders", column: "", tag_id: "t-confidential" } },
      ]),
    );
  });

  it("keeps the dialog open with the server's sentence when the remove fails", async () => {
    const user = userEvent.setup();
    serveData(fixture({ tagCatalogue: ok([]), tagWrite: { status: 403, body: { error: "forbidden" } } }));
    renderPage("/governance/data/ice/sales/orders", EDITOR);
    const columns = await within(panel()).findByRole("table", { name: "Columns" });
    await user.click(await within(columns).findByRole("button", { name: "Remove PII from ice.sales.orders.email" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Remove PII from ice.sales.orders.email?" });
    await user.click(within(dialog).getByRole("button", { name: "Remove" }));
    expect(await within(dialog).findByText("This account cannot do that: it needs the catalog_registry:update permission.")).toBeInTheDocument();
    expect(screen.getByRole("alertdialog", { name: "Remove PII from ice.sales.orders.email?" })).toBeInTheDocument();
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("moves focus to the header's Add classification after removing the table's classification", async () => {
    const user = userEvent.setup();
    serveData(fixture({ tagCatalogue: ok([]) }));
    renderPage("/governance/data/ice/sales/orders", EDITOR);
    await user.click(await within(header()).findByRole("button", { name: "Remove CONFIDENTIAL from ice.sales.orders" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Remove CONFIDENTIAL from ice.sales.orders?" });
    await user.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(within(header()).getByRole("button", { name: "Add classification" })).toHaveFocus());
  });

  it("moves focus to the column's name after removing its classification in the columns table", async () => {
    const user = userEvent.setup();
    serveData(fixture({ tagCatalogue: ok([]) }));
    renderPage("/governance/data/ice/sales/orders", EDITOR);
    const columns = await within(panel()).findByRole("table", { name: "Columns" });
    await user.click(await within(columns).findByRole("button", { name: "Remove PII from ice.sales.orders.email" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Remove PII from ice.sales.orders.email?" });
    await user.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(within(columns).getByRole("link", { name: "email" })).toHaveFocus());
  });

  it("keeps focus in the dialog when the remove fails", async () => {
    const user = userEvent.setup();
    serveData(fixture({ tagCatalogue: ok([]), tagWrite: { status: 500, body: { error: "internal error" } } }));
    renderPage("/governance/data/ice/sales/orders", EDITOR);
    const columns = await within(panel()).findByRole("table", { name: "Columns" });
    await user.click(await within(columns).findByRole("button", { name: "Remove PII from ice.sales.orders.email" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Remove PII from ice.sales.orders.email?" });
    await user.click(within(dialog).getByRole("button", { name: "Remove" }));
    expect(await within(dialog).findByText("internal error")).toBeInTheDocument();
    expect(dialog.contains(document.activeElement)).toBe(true);
  });

  it("returns focus to the Remove button on Cancel", async () => {
    const user = userEvent.setup();
    serveData(fixture({ tagCatalogue: ok([]) }));
    renderPage("/governance/data/ice/sales/orders", EDITOR);
    const columns = await within(panel()).findByRole("table", { name: "Columns" });
    const opener = await within(columns).findByRole("button", { name: "Remove PII from ice.sales.orders.email" });
    await user.click(opener);
    const dialog = await screen.findByRole("alertdialog", { name: "Remove PII from ice.sales.orders.email?" });
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(opener).toHaveFocus());
  });
});

