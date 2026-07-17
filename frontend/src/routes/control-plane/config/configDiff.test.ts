import { diffConfig } from "./configDiff";

test("diffs added/removed/changed leaf paths", () => {
  const before = { coordinator: { replicas: 1 }, logLevel: "INFO" };
  const after = { coordinator: { replicas: 2 }, queryMaxMemory: "4GB" };
  const d = diffConfig(before, after);
  expect(d).toContainEqual({
    path: "coordinator.replicas",
    kind: "changed",
    from: 1,
    to: 2,
  });
  expect(d).toContainEqual({ path: "logLevel", kind: "removed", from: "INFO" });
  expect(d).toContainEqual({
    path: "queryMaxMemory",
    kind: "added",
    to: "4GB",
  });
});

test("arrays are compared whole as leaves", () => {
  const before = { tags: ["a", "b"] };
  const after = { tags: ["a", "c"] };
  const d = diffConfig(before, after);
  expect(d).toContainEqual({
    path: "tags",
    kind: "changed",
    from: ["a", "b"],
    to: ["a", "c"],
  });
});

test("identical configs produce no diff", () => {
  const cfg = { coordinator: { replicas: 1 }, logLevel: "INFO" };
  expect(diffConfig(cfg, { ...cfg, coordinator: { replicas: 1 } })).toEqual([]);
});

test("object-key order inside an array leaf is not a change", () => {
  expect(diffConfig([{ a: 1, b: 2 }], [{ b: 2, a: 1 }])).toEqual([]);
});

test("a real value change inside an array leaf is still a change", () => {
  expect(diffConfig([{ a: 1 }], [{ a: 2 }]).length).toBe(1);
});

test("array element order remains significant", () => {
  expect(diffConfig([1, 2], [2, 1]).length).toBe(1);
});
