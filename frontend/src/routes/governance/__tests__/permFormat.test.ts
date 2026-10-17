import { describe, expect, it } from "vitest";
import { formatScope, isPattern } from "@/routes/governance/permFormat";

describe("formatScope", () => {
  it("marks an unset level with a bare *", () => {
    expect(formatScope({ catalog_id: "c" }, "ice")).toBe("ice.*");
    expect(formatScope({ catalog_id: "c", schema: "sales" }, "ice")).toBe("ice.sales.*");
    expect(formatScope({ catalog_id: "c", schema: "sales", table: "orders", column: "email" }, "ice")).toBe(
      "ice.sales.orders.email",
    );
  });

  it("quotes a level set to a pattern, so it never reads as an unset level", () => {
    expect(formatScope({ catalog_id: "c", schema: "sales", table: "*" }, "ice")).toBe("ice.sales.“*”");
    expect(formatScope({ catalog_id: "c", schema: "sales" }, "ice")).not.toBe(
      formatScope({ catalog_id: "c", schema: "sales", table: "*" }, "ice"),
    );
    expect(formatScope({ catalog_id: "c", schema: "sal*" }, "ice")).toBe("ice.“sal*”.*");
    expect(formatScope({ catalog_id: "c", schema: "sales", table: "orders", column: "e?ail" }, "ice")).toBe(
      "ice.sales.orders.“e?ail”",
    );
  });
});

describe("isPattern", () => {
  it.each([
    ["*", true],
    ["ord*", true],
    ["order?", true],
    ["orders", false],
    ["", false],
  ])("%j is %s", (value, expected) => {
    expect(isPattern(value)).toBe(expected);
  });
});
