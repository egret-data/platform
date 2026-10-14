import { describe, expect, it } from "vitest";
import type { WorkflowNamespaceDto } from "@/api/generated/model";
import {
  buildNamespaceRowTree,
  directChildNamespaces,
  namespaceSegments,
  type NamespaceRowNode,
} from "@/components/workflows/namespaceTree";

function row(namespace: string, over: Partial<WorkflowNamespaceDto> = {}): WorkflowNamespaceDto {
  return { namespace, flows: 0, files: 0, kv_keys: 0, own: true, ...over };
}

/** `path (label)` per node, depth-first, indented by two spaces a level. */
function shape(nodes: NamespaceRowNode[], depth = 0): string[] {
  return nodes.flatMap((n) => [`${"  ".repeat(depth)}${n.path} (${n.label})`, ...shape(n.children, depth + 1)]);
}

describe("buildNamespaceRowTree", () => {
  it("nests a flat, unsorted list by dots and sorts siblings", () => {
    // A child before its parent, siblings in reverse, and `companyx`, which starts
    // with `company` without being inside it.
    const rows = [
      row("company.team.etl"),
      row("companyx"),
      row("company.team", { own: false }),
      row("company.data"),
      row("company", { own: false }),
    ];
    expect(shape(buildNamespaceRowTree(rows))).toEqual([
      "company (company)",
      "  company.data (data)",
      "  company.team (team)",
      "    company.team.etl (etl)",
      "companyx (companyx)",
    ]);
  });

  it("keeps each node's own row, counts and parent-only flag included", () => {
    const [company] = buildNamespaceRowTree([row("company", { own: false }), row("company.team", { flows: 3, kv_keys: 2 })]);
    expect(company.row.own).toBe(false);
    expect(company.children[0].row).toEqual(row("company.team", { flows: 3, kv_keys: 2 }));
  });

  it("roots a node whose parent is not listed under its full name", () => {
    // A prefix filter answers `company.team` and below, without `company`.
    expect(shape(buildNamespaceRowTree([row("company.team.etl"), row("company.team")]))).toEqual([
      "company.team (company.team)",
      "  company.team.etl (etl)",
    ]);
  });

  it("gives an empty segment an empty label, under the parent the dots name", () => {
    expect(shape(buildNamespaceRowTree([row("a..b"), row("a.", { own: false }), row("a", { own: false })]))).toEqual([
      "a (a)",
      "  a. ()",
      "    a..b (b)",
    ]);
  });
});

describe("namespaceSegments", () => {
  it("names each segment with the namespace it ends", () => {
    expect(namespaceSegments("company.team.etl")).toEqual([
      { label: "company", path: "company" },
      { label: "team", path: "company.team" },
      { label: "etl", path: "company.team.etl" },
    ]);
  });

  it("keeps an empty segment", () => {
    expect(namespaceSegments("a..b")).toEqual([
      { label: "a", path: "a" },
      { label: "", path: "a." },
      { label: "b", path: "a..b" },
    ]);
  });
});

describe("directChildNamespaces", () => {
  it("answers the namespaces one dot below, sorted, and nothing deeper or beside", () => {
    const rows = [
      row("company.team.etl"),
      row("companyx"),
      row("company.team"),
      row("company.data"),
      row("company"),
    ];
    expect(directChildNamespaces(rows, "company").map((r) => r.namespace)).toEqual(["company.data", "company.team"]);
  });

  it("counts an empty segment as a child", () => {
    expect(directChildNamespaces([row("a..b"), row("a."), row("a")], "a").map((r) => r.namespace)).toEqual(["a."]);
  });
});
