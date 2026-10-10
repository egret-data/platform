import { describe, expect, it } from "vitest";
import type { FlowListItemDto } from "@/api/generated/model";
import {
  buildNamespaceTree,
  knownNamespaces,
  namespaceContains,
  type NamespaceNode,
} from "@/components/workflows/namespaceTree";

function flow(namespace: string, flow_id: string): FlowListItemDto {
  return {
    id: `${namespace}/${flow_id}`,
    namespace,
    flow_id,
    state: "draft",
    created_at: "2026-10-01T00:00:00Z",
    updated_at: "2026-10-01T00:00:00Z",
  };
}

/**
 * DELIBERATELY UNSORTED, on every axis the tree sorts: the deepest namespace
 * comes first, a child arrives before its parent, siblings arrive in reverse,
 * and flows inside one namespace arrive in reverse. A builder that kept input
 * order would pass a sorted fixture and fail this one.
 *
 * `companyx` is a sibling whose name STARTS WITH another sibling's name, so a
 * prefix test written as `startsWith("company")` cannot pass for a subtree
 * test written as `startsWith("company.")`.
 */
const FLOWS: FlowListItemDto[] = [
  flow("company.team.data", "nightly"),
  flow("zeta", "z1"),
  flow("company.team", "sync"),
  flow("companyx", "other"),
  flow("company", "root-b"),
  flow("company.team", "etl"),
  flow("company", "root-a"),
  flow("company.ops", "page"),
];

/** Pre-order walk as `path` strings: a readable picture of the whole tree. */
function paths(nodes: NamespaceNode[]): string[] {
  return nodes.flatMap((n) => [n.path, ...paths(n.children)]);
}

function find(nodes: NamespaceNode[], path: string): NamespaceNode {
  const all = (ns: NamespaceNode[]): NamespaceNode[] => ns.flatMap((n) => [n, ...all(n.children)]);
  const hit = all(nodes).find((n) => n.path === path);
  if (!hit) throw new Error(`no node ${path}`);
  return hit;
}

describe("buildNamespaceTree", () => {
  it("nests a, a.b and a.b.c under one another", () => {
    const tree = buildNamespaceTree(FLOWS);
    const company = find(tree, "company");
    const team = find(tree, "company.team");
    const data = find(tree, "company.team.data");
    expect(company.children).toContain(team);
    expect(team.children).toContain(data);
    expect(data.segment).toBe("data");
    expect(team.segment).toBe("team");
  });

  it("sorts siblings by segment whatever the input order", () => {
    expect(paths(buildNamespaceTree(FLOWS))).toEqual([
      "company",
      "company.ops",
      "company.team",
      "company.team.data",
      "companyx",
      "zeta",
    ]);
  });

  it("gives the same tree for the input reversed", () => {
    expect(buildNamespaceTree([...FLOWS].reverse())).toEqual(buildNamespaceTree(FLOWS));
  });

  it("holds a namespace's OWN flows on the node, sorted by flow id, apart from its children", () => {
    const company = find(buildNamespaceTree(FLOWS), "company");
    expect(company.flows.map((f) => f.flow_id)).toEqual(["root-a", "root-b"]);
    // The children carry their own flows; none of them leak onto the parent.
    expect(company.flows.every((f) => f.namespace === "company")).toBe(true);
  });

  it("counts every descendant flow in total, not only the node's own", () => {
    const tree = buildNamespaceTree(FLOWS);
    expect(find(tree, "company.team.data").total).toBe(1);
    expect(find(tree, "company.team").total).toBe(3);
    expect(find(tree, "company").total).toBe(6);
    expect(find(tree, "companyx").total).toBe(1);
  });

  it("makes a node for an intermediate namespace no flow lives in directly", () => {
    const tree = buildNamespaceTree([flow("a.b.c", "only")]);
    expect(paths(tree)).toEqual(["a", "a.b", "a.b.c"]);
    expect(find(tree, "a").flows).toEqual([]);
    expect(find(tree, "a").total).toBe(1);
  });

  it("returns no nodes for no flows", () => {
    expect(buildNamespaceTree([])).toEqual([]);
  });
});

describe("namespaceContains", () => {
  it("is true for the namespace itself and for its descendants", () => {
    expect(namespaceContains("company", "company")).toBe(true);
    expect(namespaceContains("company", "company.team.data")).toBe(true);
  });

  it("is false for a sibling that only shares a prefix, and for an ancestor", () => {
    expect(namespaceContains("company", "companyx")).toBe(false);
    expect(namespaceContains("company.team", "company")).toBe(false);
  });
});

describe("knownNamespaces", () => {
  it("lists every namespace and every ancestor of one, once each, sorted", () => {
    expect(knownNamespaces([flow("b.x", "f"), flow("a.b.c", "g"), flow("b.x", "h")])).toEqual([
      "a",
      "a.b",
      "a.b.c",
      "b",
      "b.x",
    ]);
  });
});
