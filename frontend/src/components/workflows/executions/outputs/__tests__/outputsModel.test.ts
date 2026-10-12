import { describe, expect, it } from "vitest";
import { findFileUris, isFileUri, outputExpression } from "@/components/workflows/executions/outputs/outputsModel";

describe("findFileUris", () => {
  it("finds kestra:/// URIs at any depth of objects and arrays, with their paths, in document order", () => {
    const value = {
      uri: "kestra:///a/b/top.csv",
      count: 3,
      nested: { deeper: { file: "kestra:///a/b/deep.ion", note: "see kestra:///a/b/not-a-file" } },
      list: ["plain", "kestra:///a/b/first.txt", { inner: ["kestra:///a/b/in-array.json"] }],
      empty: null,
    };
    expect(findFileUris(value)).toEqual([
      { path: ["uri"], uri: "kestra:///a/b/top.csv" },
      { path: ["nested", "deeper", "file"], uri: "kestra:///a/b/deep.ion" },
      { path: ["list", 1], uri: "kestra:///a/b/first.txt" },
      { path: ["list", 2, "inner", 0], uri: "kestra:///a/b/in-array.json" },
    ]);
  });

  it("treats a top-level string as a file and other schemes as not", () => {
    expect(findFileUris("kestra:///x.csv")).toEqual([{ path: [], uri: "kestra:///x.csv" }]);
    expect(findFileUris(["kestra://x.csv", "s3://bucket/x.csv", "KESTRA:///x.csv"])).toEqual([]);
    expect(isFileUri(42)).toBe(false);
  });

  it("accepts only a well-formed URI: a non-empty path, no whitespace, no trailing slash", () => {
    expect(isFileUri("kestra:///a/b/c.ion")).toBe(true);
    expect(isFileUri("kestra:///x")).toBe(true);
    // The bare prefix, a directory, and log-like text that starts with a URI.
    expect(isFileUri("kestra:///")).toBe(false);
    expect(isFileUri("kestra:///a/b/")).toBe(false);
    expect(isFileUri("kestra:///a b c")).toBe(false);
    expect(isFileUri("kestra:///x.csv\nwritten")).toBe(false);
    expect(isFileUri("kestra:///x.csv\twritten")).toBe(false);
    expect(isFileUri("kestra:///x.csv ")).toBe(false);
  });
});

describe("outputExpression", () => {
  it("writes a dotted path for plain keys and indexes", () => {
    expect(outputExpression("t1", ["key"])).toBe("{{ outputs.t1.key }}");
    expect(outputExpression("t1", ["rows", 0, "id"])).toBe("{{ outputs.t1.rows[0].id }}");
    expect(outputExpression("t1", [])).toBe("{{ outputs.t1 }}");
  });

  it("brackets a key Pebble cannot read after a dot, task ids included", () => {
    expect(outputExpression("my-task", ["odd key", "2nd", "ok_1"])).toBe('{{ outputs["my-task"]["odd key"]["2nd"].ok_1 }}');
    // An object key that looks like an index is still a key.
    expect(outputExpression("t1", ["0", 0])).toBe('{{ outputs.t1["0"][0] }}');
  });
});
