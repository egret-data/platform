import { describe, expect, test } from "vitest";
import { scopeToDataLink } from "../openInData";

const cats = new Map([["cat-1", "ice"]]);

describe("scopeToDataLink", () => {
  test("a schema scope opens the schema page on the chosen tab", () => {
    expect(scopeToDataLink({ catalog_id: "cat-1", schema: "sales" }, cats, "permissions")).toEqual({
      to: "/governance/data/ice/sales?tab=permissions",
      stoppedAtPattern: false,
    });
  });
  test("a column scope opens the table with ?column=", () => {
    expect(
      scopeToDataLink({ catalog_id: "cat-1", schema: "sales", table: "orders", column: "email" }, cats, "masks")?.to,
    ).toBe("/governance/data/ice/sales/orders?column=email&tab=masks");
  });
  test("a pattern table stops at the schema", () => {
    expect(scopeToDataLink({ catalog_id: "cat-1", schema: "sales", table: "*" }, cats, "permissions")).toEqual({
      to: "/governance/data/ice/sales?tab=permissions",
      stoppedAtPattern: true,
    });
  });
  test("a pattern column stops at the table; a pattern schema at the catalog", () => {
    expect(
      scopeToDataLink({ catalog_id: "cat-1", schema: "sales", table: "orders", column: "e*" }, cats, "masks")?.to,
    ).toBe("/governance/data/ice/sales/orders?tab=masks");
    expect(scopeToDataLink({ catalog_id: "cat-1", schema: "s?", table: "orders" }, cats, "masks")?.to).toBe(
      "/governance/data/ice?tab=masks",
    );
  });
  test("a catalog outside the list has no link", () => {
    expect(scopeToDataLink({ catalog_id: "gone" }, cats, "permissions")).toBeNull();
  });
});
