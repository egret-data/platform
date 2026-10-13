import { describe, expect, it, vi } from "vitest";
import { layout } from "@/components/workflows/topology/layout";
import { NESTED } from "./fixtures";

// dagre throwing, as it does when its recursion overflows the stack (layout.ts, DAGRE_LIMIT).
vi.mock("@dagrejs/dagre", async (importOriginal) => {
  const real = await importOriginal<typeof import("@dagrejs/dagre")>();
  return {
    ...real,
    layout: vi.fn(() => {
      throw new RangeError("Maximum call stack size exceeded");
    }),
  };
});

describe("layout when dagre throws", () => {
  it.each(["LR", "TB"] as const)("lays the graph out with the fallback in %s", (orientation) => {
    const { nodes, edges } = layout(NESTED, { orientation, collapsed: new Set() });
    expect(nodes.map((n) => n.id).sort()).toEqual(
      ["cluster:tasks.s", "cluster:tasks.s.p", "end:flow", "end:tasks.s", "end:tasks.s.p", "start:flow", "start:tasks.s", "start:tasks.s.p", "tasks.after", "tasks.s", "tasks.s.p", "tasks.s.p.z"].sort(),
    );
    for (const e of edges) expect(e.points.length).toBeGreaterThanOrEqual(2);
  });
});
