import { afterEach, describe, expect, it, vi } from "vitest";
import userEvent from "@testing-library/user-event";
import { render, screen, within } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Me } from "@/api/hooks";
import type { ObjectLineage } from "@/api/generated/model";
import { AuthContext } from "@/auth/useAuth";
import { TooltipProvider } from "@/components/ui/tooltip";
import DataRoutes from "@/routes/governance/data/DataRoutes";
import { browseItems, catalog, serveData, type FakeData } from "./fakeDataServer";

const ICE = { ...catalog("c-ice", "ice", "iceberg", true), engines: ["trino-1"] };
const PG = catalog("c-pg", "pg", "hive");
const ok = (body: unknown) => ({ status: 200, body });
const ME: Me = { user_id: "u1", email: "s@egret.local", platform_roles: [], permissions: ["policy:read"] };

const SALES = browseItems([
  { name: "v_orders", kind: "view" },
  { name: "orders", kind: "table" },
]);

function fixture(lineage: Record<string, unknown>): FakeData {
  return {
    catalogs: ok([PG, ICE]),
    browse: {
      "c-ice": ok(browseItems([{ name: "sales", kind: "schema" }])),
      "c-ice|sales": ok(SALES),
      "c-ice|sales|orders": ok({ source: { kind: "egret" }, engines: [], columns: [] }),
      "c-ice|sales|v_orders": ok({ source: { kind: "egret" }, engines: [], columns: [] }),
    },
    tags: {
      "c-ice|sales|orders": ok({ supported: true, assignments: [] }),
      "c-ice|sales|v_orders": ok({ supported: true, assignments: [] }),
    },
    lineage: Object.fromEntries(Object.entries(lineage).map(([k, v]) => [k, v as { status: number; body: unknown }])),
    rules: ok([]),
  };
}

function open(url: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <AuthContext.Provider value={{ me: ME, isLoading: false }}>
      <QueryClientProvider client={client}>
        <TooltipProvider>
          <MemoryRouter initialEntries={[url]}>
            <Routes>
              <Route path="/governance/data/*" element={<DataRoutes />} />
            </Routes>
          </MemoryRouter>
        </TooltipProvider>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}
const panel = () => screen.getByRole("tabpanel");
const TABLE = "/governance/data/ice/sales/orders?tab=lineage";
const VIEW = "/governance/data/ice/sales/v_orders?tab=lineage";

const reader = (over: Partial<NonNullable<ObjectLineage["readers"]>[number]> = {}) => ({
  catalog: "ice",
  schema: "sales",
  name: "v_orders",
  kind: "view",
  matchedBy: "lineage",
  ...over,
});

afterEach(() => vi.unstubAllGlobals());

describe("Lineage tab", () => {
  it("lists a table's readers, each linking to its object page", async () => {
    serveData(fixture({ "c-ice|sales|orders": ok({ readers: [reader()] }) }));
    open(TABLE);
    const link = await within(panel()).findByRole("link", { name: "v_orders" });
    expect(link).toHaveAttribute("href", "/governance/data/ice/sales/v_orders");
    expect(within(panel()).queryByText("No view or materialized view reads this table")).not.toBeInTheDocument();
  });

  it("names a reader's catalog without a link when it is not in the catalog list", async () => {
    serveData(fixture({ "c-ice|sales|orders": ok({ readers: [reader({ catalog: "gone" })] }) }));
    open(TABLE);
    expect(await within(panel()).findByText("gone")).toBeInTheDocument();
    expect(within(panel()).queryByRole("link", { name: "gone" })).not.toBeInTheDocument();
    expect(within(panel()).queryByRole("link", { name: "v_orders" })).not.toBeInTheDocument();
    expect(within(panel()).getByText("v_orders")).toBeInTheDocument();
  });

  it("links a reader's catalog to its page", async () => {
    serveData(fixture({ "c-ice|sales|orders": ok({ readers: [reader()] }) }));
    open(TABLE);
    const link = await within(panel()).findByRole("link", { name: "ice" });
    expect(link).toHaveAttribute("href", "/governance/data/ice");
  });

  it("labels a reader matched by definition text", async () => {
    serveData(
      fixture({
        "c-ice|sales|orders": ok({ readers: [reader({ kind: "materialized_view", name: "mv", matchedBy: "text", engine: "sr-1" })] }),
      }),
    );
    open(TABLE);
    expect(await within(panel()).findByText(/Matched by definition text/)).toBeInTheDocument();
  });

  it("says no reader reads the table only when the list is complete", async () => {
    serveData(fixture({ "c-ice|sales|orders": ok({ readers: [] }) }));
    open(TABLE);
    expect(await within(panel()).findByText("No view or materialized view reads this table")).toBeInTheDocument();
  });

  it("warns that the list may be incomplete, and never calls an empty one complete", async () => {
    serveData(
      fixture({ "c-ice|sales|orders": ok({ readers: [], readersIncomplete: { detail: "A view's lineage failed in ice." } }) }),
    );
    open(TABLE);
    expect(await within(panel()).findByText(/A view's lineage failed in ice\./)).toBeInTheDocument();
    expect(within(panel()).getByText(/may be incomplete/)).toBeInTheDocument();
    expect(within(panel()).getByText("No reader is known.")).toBeInTheDocument();
    expect(within(panel()).queryByText("No view or materialized view reads this table")).not.toBeInTheDocument();
  });

  it("warns above a non-empty incomplete list", async () => {
    serveData(
      fixture({ "c-ice|sales|orders": ok({ readers: [reader()], readersIncomplete: { detail: "Views in pg are not tracked." } }) }),
    );
    open(TABLE);
    expect(await within(panel()).findByText(/Views in pg are not tracked\./)).toBeInTheDocument();
    expect(within(panel()).getByRole("link", { name: "v_orders" })).toBeInTheDocument();
  });

  it("shows a view's bases (linked), mode, owner, owner cleared and what its state means", async () => {
    serveData(
      fixture({
        "c-ice|sales|v_orders": ok({
          readers: [],
          view: {
            securityMode: "definer",
            owner: "svc_reports",
            ownerCleared: false,
            lineageState: "computed",
            bases: [
              { catalog: "ice", schema: "sales", table: "orders" },
              { catalog: "gone", schema: "x", table: "y" },
            ],
          },
        }),
      }),
    );
    open(VIEW);
    const link = await within(panel()).findByRole("link", { name: "orders" });
    expect(link).toHaveAttribute("href", "/governance/data/ice/sales/orders");
    expect(within(panel()).queryByRole("link", { name: "y" })).not.toBeInTheDocument();
    expect(within(panel()).getByText("y")).toBeInTheDocument();
    expect(within(panel()).getByText("DEFINER")).toBeInTheDocument();
    expect(within(panel()).getByText("svc_reports")).toBeInTheDocument();
    expect(within(panel()).getByText("Owner cleared").nextElementSibling).toHaveTextContent("No");
    expect(within(panel()).getByText(/Recorded: egret read the view's definition/)).toBeInTheDocument();
  });

  it("explains a failed and a missing lineage state, with the detail", async () => {
    serveData(
      fixture({
        "c-ice|sales|v_orders": ok({
          readers: [],
          view: { securityMode: "invoker", lineageState: "failed", lineageDetail: "Could not parse.", bases: [] },
        }),
      }),
    );
    open(VIEW);
    expect(await within(panel()).findByText(/Failed: egret could not work out what this view reads/)).toHaveTextContent("Could not parse.");
    expect(within(panel()).queryByText(/No base tables/)).not.toBeInTheDocument();
  });

  it("explains a missing lineage state", async () => {
    serveData(
      fixture({ "c-ice|sales|v_orders": ok({ readers: [], view: { securityMode: "invoker", lineageState: "missing", bases: [] } }) }),
    );
    open(VIEW);
    expect(await within(panel()).findByText(/Not recorded: egret has no lineage for this view/)).toBeInTheDocument();
  });

  it("says when egret holds no view facts", async () => {
    serveData(fixture({ "c-ice|sales|v_orders": ok({ readers: [], view: null }) }));
    open(VIEW);
    expect(await within(panel()).findByText("egret has no lineage recorded for this object.")).toBeInTheDocument();
  });

  it("shows an error with a retry, never an empty state", async () => {
    serveData(fixture({ "c-ice|sales|orders": { status: 504, body: { error: "timed out" } } }));
    open(TABLE);
    expect(
      await within(panel()).findByText("The lineage could not be loaded. Retry, or reload the page. timed out"),
    ).toBeInTheDocument();
    expect(within(panel()).getByRole("button", { name: "Retry" })).toBeInTheDocument();
    expect(within(panel()).queryByText("No view or materialized view reads this table")).not.toBeInTheDocument();
  });

  it("shows the server's message on a 503", async () => {
    serveData(fixture({ "c-ice|sales|orders": { status: 503, body: { error: "warehouse unavailable" } } }));
    open(TABLE);
    expect(await within(panel()).findByText(/warehouse unavailable/)).toBeInTheDocument();
  });

  it("asks for lineage once when Overview is opened and then the Lineage tab", async () => {
    const { lineageCalls } = serveData(fixture({ "c-ice|sales|v_orders": ok({ readers: [], view: null }) }));
    open("/governance/data/ice/sales/v_orders");
    await within(panel()).findByText("egret has no security mode or lineage recorded for this object.");
    await userEvent.setup().click(screen.getByRole("tab", { name: "Lineage" }));
    await within(panel()).findByText("egret has no lineage recorded for this object.");
    expect(lineageCalls).toEqual(["c-ice|sales|v_orders"]);
  });
});
