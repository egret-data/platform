import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/client";
import { otherTablesText, ruleOverlap, tableTagMaskConflict, tagInUse, tagKeyInvalid, tagKeyTaken, tagNameTaken, type TableTagMaskConflict } from "@/routes/governance/apiErrors";

const err = (status: number, body: Record<string, unknown>) =>
  new ApiError(status, String(body.error), body.code as string | undefined, body);

describe("apiErrors", () => {
  it("narrows each code to its body and nothing else", () => {
    const conflict = err(400, {
      error: "tag \"PII\" is attached…", code: "table_tag_mask_conflict", tag: "PII", tagId: "t1",
      warehouseId: "w1", schema: "analytics", table: "orders", mask: "m", maskOrigin: "rule", moreTables: false,
    });
    expect(tableTagMaskConflict(conflict)).toMatchObject({ tag: "PII", warehouseId: "w1", table: "orders" });
    expect(ruleOverlap(conflict)).toBeNull();
    expect(tableTagMaskConflict(new ApiError(400, "bad request: x"))).toBeNull();
    expect(tableTagMaskConflict(new Error("x"))).toBeNull();

    const overlap = err(409, { error: "…", code: "rule_overlap", rules: [{ id: "a", name: "A" }, { id: null, name: "P" }], coordinate: "c.s.t.email", kind: "mask" });
    expect(ruleOverlap(overlap)?.rules.map((r) => r.name)).toEqual(["A", "P"]);
    expect(tagInUse(err(409, { error: "…", code: "tag_in_use", rules: [{ id: "r1", name: "Hash" }] }))?.rules).toEqual([{ id: "r1", name: "Hash" }]);
    expect(tagNameTaken(err(409, { error: "…", code: "tag_name_taken", name: "pii", existing: { id: "t", key: "pii", name: "PII" } }))?.existing.key).toBe("pii");
    expect(tagKeyTaken(err(409, { error: "…", code: "tag_key_taken", key: "pii", existing: { id: "t", name: "PII" } }))?.existing.name).toBe("PII");
    expect(tagKeyInvalid(err(400, { error: "…", code: "tag_key_invalid", key: "p_i", allowed: "a-z 0-9 : -" }))?.key).toBe("p_i");
    expect(tagKeyInvalid(conflict)).toBeNull();
  });

  it("a coded error with no object body is not narrowed", () => {
    expect(tagInUse(new ApiError(409, "x", "tag_in_use"))).toBeNull();
    expect(tagInUse(new ApiError(409, "x", "tag_in_use", "not an object"))).toBeNull();
  });

  it("names every other table the tag is on, and says when the list is cut short", () => {
    const base = {
      error: "tag \"PII\" is attached…", code: "table_tag_mask_conflict" as const, tag: "PII", tagId: "t1", warehouseId: "w1",
      schema: "analytics", table: "orders", mask: "m", maskOrigin: "rule" as const,
    };
    const none: TableTagMaskConflict = { ...base, moreTables: false };
    const listed: TableTagMaskConflict = {
      ...base,
      moreTables: true,
      otherTables: [{ warehouseId: "w1", schema: "sales", table: "returns" }, { warehouseId: "w2", schema: "sales", table: "refunds" }],
      otherTablesTruncated: false,
    };
    const cut: TableTagMaskConflict = { ...listed, otherTablesTruncated: true };
    const older: TableTagMaskConflict = { ...base, moreTables: true };

    expect(otherTablesText(none)).toBeNull();
    expect(otherTablesText(listed)).toBe("Other tables carry it too: sales.returns, sales.refunds.");
    expect(otherTablesText(cut)).toBe("Other tables carry it too: sales.returns, sales.refunds, and more.");
    expect(otherTablesText(older)).toBe("Other tables carry it too.");
  });
});
