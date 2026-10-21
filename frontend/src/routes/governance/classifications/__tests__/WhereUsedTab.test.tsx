import { afterEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { TagUsage } from "@/api/generated/model";
import { ADMIN, fail, ok, openArea, serve, tag, TAG_READER } from "./harness";

afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };
const PII = tag({ id: "t-pii", key: "pii", name: "PII" });
const URL_PII = "/governance/classifications/t-pii";
const USAGE = "/classifications/t-pii/usage";

function usage(over: { data?: Partial<TagUsage["data"]>; bi?: Partial<TagUsage["bi"]> } = {}): TagUsage {
  return {
    tag: { id: "t-pii", key: "pii", name: "PII" },
    data: { tables: [], tableCount: 0, columnCount: 0, orphaned: 0, ...over.data },
    bi: { readable: [], unreadable: { count: 0 }, deleted: 0, checked: 0, remaining: 0, ...over.bi },
  };
}

/** Not in display order: catalogs, schemas and tables all arrive shuffled. */
const TABLES: TagUsage["data"]["tables"] = [
  { catalogId: "c-z", catalogName: "zeta", warehouseId: "w-z", schema: "s", table: "t9", onTable: true, columns: [] },
  { catalogId: "c-i", catalogName: "ice", warehouseId: "w-i", schema: "sales", table: "orders", onTable: false, columns: ["phone", "email"] },
  { catalogId: "c-i", catalogName: "ice", warehouseId: "w-i", schema: "crm", table: "people", onTable: true, columns: ["email"] },
  { catalogId: "c-i", catalogName: "ice", warehouseId: "w-i", schema: "crm", table: "accounts", onTable: true, columns: [] },
];

const open = (u: TagUsage | "error" | "never", as = TAG_READER) => {
  const served = serve({
    "/tags": ok([PII]),
    [USAGE]: u === "error" ? fail(500, "database unavailable") : u === "never" ? "never" : ok(u),
  });
  openArea(URL_PII, as);
  return served;
};
const data = () => screen.getByRole("region", { name: "Data" });
const bi = () => screen.getByRole("region", { name: "BI" });

describe("Where used: Data", () => {
  it("groups tables by catalog, then schema, each sorted", async () => {
    open(usage({ data: { tables: TABLES, tableCount: 3, columnCount: 3 } }));
    await screen.findByRole("link", { name: "accounts" }, T);
    const groups = within(data()).getAllByRole("heading", { level: 3 }).map((h) => h.textContent);
    expect(groups).toEqual(["ice.crm", "ice.sales", "zeta.s"]);
    const crm = within(data()).getByRole("table", { name: "ice.crm" });
    expect(within(crm).getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["Table", "Attached to"]);
    expect(within(data()).queryByRole("columnheader", { name: "Carries it" })).not.toBeInTheDocument();
    expect(within(crm).getAllByRole("row").slice(1).map((r) => within(r).getAllByRole("cell")[0].textContent)).toEqual([
      "accounts",
      "people",
    ]);
  });

  it("says what carries the tag and links the table and each column to Data", async () => {
    open(usage({ data: { tables: TABLES, tableCount: 3, columnCount: 3 } }));
    await screen.findByRole("link", { name: "accounts" }, T);
    const people = within(data()).getByRole("link", { name: "people" }).closest("tr") as HTMLElement;
    expect(within(people).getByRole("link", { name: "people" })).toHaveAttribute("href", "/governance/data/ice/crm/people");
    expect(within(people).getAllByRole("cell")[1].textContent).toBe("Table · Columns: email");
    expect(within(people).getByRole("link", { name: "email" })).toHaveAttribute(
      "href",
      "/governance/data/ice/crm/people?column=email",
    );
    const orders = within(data()).getByRole("link", { name: "orders" }).closest("tr") as HTMLElement;
    expect(within(orders).getAllByRole("cell")[1].textContent).toBe("Columns: email, phone");
  });

  it("counts assignments in a warehouse no catalog names", async () => {
    open(usage({ data: { tables: TABLES.slice(0, 1), tableCount: 1, orphaned: 2 } }));
    expect(await screen.findByText("2 assignments are in a warehouse no catalog names.", {}, T)).toBeInTheDocument();
  });

  it("says so when no table or column carries it, and where to attach it", async () => {
    open(usage(), ADMIN);
    expect(await screen.findByText("No table or column carries this classification.", {}, T)).toBeInTheDocument();
    expect(within(data()).getByRole("link", { name: "Attach in Data" })).toHaveAttribute("href", "/governance/data");
    expect(within(data()).getByText("Open a table or a column and use Add classification.")).toBeInTheDocument();
  });

  it("without catalog_registry:update, promises no Add classification and says what attaching needs", async () => {
    open(usage());
    expect(await screen.findByText("No table or column carries this classification.", {}, T)).toBeInTheDocument();
    expect(within(data()).getByText("Attaching needs the catalog_registry:update permission.")).toBeInTheDocument();
    expect(within(data()).queryByText(/use Add classification/)).not.toBeInTheDocument();
    expect(within(data()).queryByRole("link", { name: "Attach in Data" })).not.toBeInTheDocument();
    expect(within(data()).getByRole("link", { name: "Open Data" })).toHaveAttribute("href", "/governance/data");
  });
});

describe("Where used: BI", () => {
  const READABLE: TagUsage["bi"]["readable"] = [
    { kind: "saved_query", id: "q1", name: "Weekly pull", workspaceSlug: "w1" },
    { kind: "dashboard", id: "d1", name: "Revenue", workspaceSlug: "w1" },
    { kind: "folder", id: "f1", name: "Finance folder", workspaceSlug: "w2" },
    { kind: "chart", id: "c1", name: "Churn", workspaceSlug: "w2" },
    { kind: "dataset", id: "ds1", name: "Customers", workspaceSlug: "w1" },
    { kind: "app", id: "a1", name: "Portal", workspaceSlug: "w1" },
  ];

  it("lists readable objects with kind and workspace, linking those with a page (CL13)", async () => {
    open(usage({ bi: { readable: READABLE, checked: 6 } }));
    await screen.findByRole("link", { name: "Revenue" }, T);
    expect(within(bi()).getByRole("link", { name: "Revenue" })).toHaveAttribute("href", "/explore/w1/dashboards/d1");
    expect(within(bi()).getByRole("link", { name: "Churn" })).toHaveAttribute("href", "/explore/w2/charts/c1");
    expect(within(bi()).getByRole("link", { name: "Customers" })).toHaveAttribute("href", "/explore/w1/datasets/ds1");
    expect(within(bi()).getByRole("link", { name: "Portal" })).toHaveAttribute("href", "/explore/w1/apps/a1");
    expect(within(bi()).queryByRole("link", { name: "Weekly pull" })).not.toBeInTheDocument();
    expect(within(bi()).queryByRole("link", { name: "Finance folder" })).not.toBeInTheDocument();
    const query = within(bi()).getByText("Weekly pull").closest("tr") as HTMLElement;
    expect(query).toHaveTextContent("Saved query");
    expect(query).toHaveTextContent("w1");
    const folder = within(bi()).getByText("Finance folder").closest("tr") as HTMLElement;
    expect(folder).toHaveTextContent("Folder");
    expect(folder).toHaveTextContent("w2");
  });

  it("counts objects the reader cannot read, naming no workspace (Q5)", async () => {
    open(usage({ bi: { unreadable: { count: 3 }, checked: 3 } }));
    const line = await screen.findByText("3 objects you cannot read.", {}, T);
    expect(line.textContent).toBe("3 objects you cannot read.");
    expect(bi().textContent).not.toMatch(/workspace|w1|w2/i);
  });

  it("writes the singular for one unreadable object", async () => {
    open(usage({ bi: { unreadable: { count: 1 }, checked: 1 } }));
    expect(await screen.findByText("1 object you cannot read.", {}, T)).toBeInTheDocument();
    expect(screen.queryByText("No BI object carries this classification.")).not.toBeInTheDocument();
  });

  it("counts assignments to deleted objects", async () => {
    open(usage({ bi: { deleted: 2 } }));
    expect(await screen.findByText("2 assignments point at deleted objects.", {}, T)).toBeInTheDocument();
  });

  it("says when not every object was checked", async () => {
    open(usage({ bi: { unreadable: { count: 500 }, checked: 500, remaining: 7 } }));
    expect(await screen.findByText("Not all BI objects were checked: 7 remain.", {}, T)).toBeInTheDocument();
    expect(screen.queryByText("No BI object carries this classification.")).not.toBeInTheDocument();
  });

  it("says so when no BI object carries it", async () => {
    open(usage());
    expect(await screen.findByText("No BI object carries this classification.", {}, T)).toBeInTheDocument();
  });
});

describe("Where used: states", () => {
  it("shows skeletons while usage loads", async () => {
    open("never");
    await screen.findByRole("heading", { level: 1, name: "PII" }, T);
    expect(await screen.findAllByTestId("list-skeleton")).not.toHaveLength(0);
    expect(screen.queryByText("No table or column carries this classification.")).not.toBeInTheDocument();
  });

  it("shows a failure with Retry and neither empty sentence", async () => {
    const user = userEvent.setup();
    const served = open("error");
    const panel = await screen.findByRole("tabpanel", {}, T);
    expect(await within(panel).findByText(/The classification's usage could not be loaded/, {}, T)).toBeInTheDocument();
    expect(within(panel).getByText(/database unavailable/)).toBeInTheDocument();
    expect(screen.queryByText("No table or column carries this classification.")).not.toBeInTheDocument();
    expect(screen.queryByText("No BI object carries this classification.")).not.toBeInTheDocument();
    await user.click(within(panel).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(served.asked.filter((a) => a === `GET ${USAGE}`)).toHaveLength(2));
  });
});

describe("Where used: Remove", () => {
  const ORDERS = { catalogId: "c1", catalogName: "ice", warehouseId: "wh-1", schema: "sales", table: "orders", onTable: false, columns: ["email"] };

  it("removes a column's assignment through the row's warehouse after a confirm", async () => {
    const served = serve({
      "/tags": ok([PII]),
      "/perm/rules": ok([]),
      [USAGE]: ok(usage({ data: { tables: [ORDERS], tableCount: 1, columnCount: 1 } })),
      "DELETE /warehouses/wh-1/resource-tags": ok({}),
    });
    openArea(URL_PII, ADMIN);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Remove PII from ice.sales.orders.email" }, T));
    const dialog = await screen.findByRole("alertdialog", { name: "Remove PII from ice.sales.orders.email?" });
    await user.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(served.bodies["DELETE /warehouses/wh-1/resource-tags"]).toEqual([{ schema: "sales", table: "orders", column: "email", tag_id: "t-pii" }]),
    );
    // Where used is read again.
    await waitFor(() => expect(served.asked.filter((a) => a === `GET ${USAGE}`).length).toBeGreaterThan(1));    // The removed row's button is gone: focus lands on the Data heading, never on <body>.
    await waitFor(() => expect(screen.getByRole("heading", { level: 2, name: "Data" })).toHaveFocus());
  });

  it("removes a table-level assignment with an empty column", async () => {
    const served = serve({
      "/tags": ok([PII]),
      "/perm/rules": ok([]),
      [USAGE]: ok(usage({ data: { tables: [{ ...ORDERS, onTable: true, columns: [] }], tableCount: 1 } })),
      "DELETE /warehouses/wh-1/resource-tags": ok({}),
    });
    openArea(URL_PII, ADMIN);
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: "Remove PII from ice.sales.orders" }, T));
    const dialog = await screen.findByRole("alertdialog", { name: "Remove PII from ice.sales.orders?" });
    await user.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(served.bodies["DELETE /warehouses/wh-1/resource-tags"]).toEqual([{ schema: "sales", table: "orders", column: "", tag_id: "t-pii" }]),
    );
  });

  it("offers no Remove without catalog_registry:update", async () => {
    open(usage({ data: { tables: [ORDERS], tableCount: 1, columnCount: 1 } }));
    await screen.findByRole("link", { name: "orders" }, T);
    expect(screen.queryByRole("button", { name: /^Remove / })).not.toBeInTheDocument();
  });
});
