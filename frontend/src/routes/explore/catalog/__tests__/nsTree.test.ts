import { describe, it, expect } from "vitest";
import { buildNamespaceTree } from "../nsTree";

describe("buildNamespaceTree", () => {
  it("folds a flat namespace list into a nested tree", () => {
    const tree = buildNamespaceTree([
      { levels: ["sale"] },
      { levels: ["sale", "2024"] },
      { levels: ["marketing"] },
    ]);
    expect(tree.map((n) => n.name)).toEqual(["marketing", "sale"]); // sorted
    const sale = tree.find((n) => n.name === "sale")!;
    expect(sale.dotted).toBe("sale");
    expect(sale.children.map((c) => c.name)).toEqual(["2024"]);
    expect(sale.children[0].dotted).toBe("sale.2024");
  });

  it("synthesizes intermediate parents not present in the input", () => {
    // A child whose parent namespace isn't itself listed must still nest.
    const tree = buildNamespaceTree([{ levels: ["a", "b", "c"] }]);
    expect(tree.map((n) => n.name)).toEqual(["a"]);
    expect(tree[0].children[0].name).toBe("b");
    expect(tree[0].children[0].children[0].dotted).toBe("a.b.c");
  });

  it("marks nodes that exist as real namespaces vs synthesized", () => {
    const tree = buildNamespaceTree([{ levels: ["a", "b"] }]);
    expect(tree[0].exists).toBe(false); // 'a' was only implied
    expect(tree[0].children[0].exists).toBe(true); // 'a.b' was listed
  });
});
