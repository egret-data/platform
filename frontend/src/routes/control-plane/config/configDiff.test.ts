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
