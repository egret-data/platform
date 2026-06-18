import { describe, expect, it } from "vitest";
import { csvUrl, qk } from "@/api/sqllab";

describe("sqllab api helpers", () => {
  it("builds the csv download url", () => {
    expect(csvUrl("eng", "abc")).toBe("/api/ws/eng/queries/abc/results.csv");
  });
  it("namespaces query keys by workspace", () => {
    expect(qk.saved("eng")).toEqual(["sqllab", "eng", "saved"]);
    expect(qk.schema("eng", "db1", "catalogs")).toEqual(["sqllab", "eng", "schema", "db1", "catalogs"]);
  });
});
