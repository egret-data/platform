import { describe, expect, it } from "vitest";
import { matchQuery } from "@tanstack/react-query";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** Whether invalidating `prefix` reaches a query keyed `key`, by react-query's own matcher. */
function reaches(prefix: readonly unknown[], key: readonly unknown[]): boolean {
  return matchQuery({ queryKey: prefix }, { queryKey: key, queryHash: JSON.stringify(key) } as never);
}

describe("workflowsKeys", () => {
  it("prefixes every key with the domain", () => {
    for (const key of [
      workflowsKeys.flows("eng"),
      workflowsKeys.flows("eng", { state: "live" }),
      workflowsKeys.flow("f"),
      workflowsKeys.versions("f"),
      workflowsKeys.version("f", 2),
      workflowsKeys.schema("eng"),
      workflowsKeys.execution("e"),
      workflowsKeys.executions("eng"),
      workflowsKeys.executions("eng", { page: 2 }),
    ]) {
      expect(key[0]).toBe("workflows");
      expect(reaches(workflowsKeys.all, key)).toBe(true);
    }
  });

  it("keeps one workspace's list and schema apart from another's", () => {
    expect(reaches(workflowsKeys.flows("a"), workflowsKeys.flows("b"))).toBe(false);
    expect(reaches(workflowsKeys.schema("a"), workflowsKeys.schema("b"))).toBe(false);
    expect(reaches(workflowsKeys.executions("a"), workflowsKeys.executions("b", { page: 1 }))).toBe(false);
  });

  it("reaches every page and filter of a workspace's executions from its unfiltered key, and no execution", () => {
    expect(reaches(workflowsKeys.executions("eng"), workflowsKeys.executions("eng", { states: ["FAILED"], page: 3 }))).toBe(true);
    expect(reaches(workflowsKeys.executions("eng"), workflowsKeys.execution("e"))).toBe(false);
    expect(reaches(workflowsKeys.executions("eng"), workflowsKeys.flows("eng"))).toBe(false);
  });

  it("reaches every filtered list of a workspace from its unfiltered key", () => {
    expect(reaches(workflowsKeys.flows("eng"), workflowsKeys.flows("eng", { namespace: "x" }))).toBe(true);
  });

  it("reaches a flow's versions from the flow, and one version from the list", () => {
    expect(reaches(workflowsKeys.flow("f"), workflowsKeys.versions("f"))).toBe(true);
    expect(reaches(workflowsKeys.versions("f"), workflowsKeys.version("f", 3))).toBe(true);
    expect(reaches(workflowsKeys.flow("f"), workflowsKeys.flow("g"))).toBe(false);
  });

  it("cannot be spelled by a workspace slug: a workspace named `flow` does not share a flow's prefix", () => {
    expect(reaches(workflowsKeys.flow("flows"), workflowsKeys.flows("flow"))).toBe(false);
    expect(reaches(workflowsKeys.flows("flow"), workflowsKeys.flow("flows"))).toBe(false);
  });
});
