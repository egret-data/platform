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
    expect(isFileUri("kestra:///")).toBe(true);
    expect(isFileUri(42)).toBe(false);
  });
});

describe("outputExpression", () => {
  it("writes a dotted path for plain keys and indexes", () => {
    expect(outputExpression("t1", null, ["key"])).toBe("{{ outputs.t1.key }}");
    expect(outputExpression("t1", undefined, ["rows", 0, "id"])).toBe("{{ outputs.t1.rows[0].id }}");
    expect(outputExpression("t1", null, [])).toBe("{{ outputs.t1 }}");
  });

  it("brackets a key Pebble cannot read after a dot, task ids included", () => {
    expect(outputExpression("my-task", null, ["odd key", "2nd", "ok_1"])).toBe('{{ outputs["my-task"]["odd key"]["2nd"].ok_1 }}');
  });

  it("reaches a run with a value through it, in Kestra's outputs.<taskId>[value] form", () => {
    expect(outputExpression("t1", "eu-west", ["key"])).toBe('{{ outputs.t1["eu-west"].key }}');
    expect(outputExpression("t1", 'say "hi"', [])).toBe('{{ outputs.t1["say \\"hi\\""] }}');
    // An empty value is still a value.
    expect(outputExpression("t1", "", ["k"])).toBe('{{ outputs.t1[""].k }}');
  });
});
