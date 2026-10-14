import type { NamespaceDependenciesDto, NamespaceDependencyEdgeDto, NamespaceDependencyNodeDto } from "@/api/generated/model";

export function flow(id: string, namespace: string, flowId: string, over: Partial<NamespaceDependencyNodeDto> = {}): NamespaceDependencyNodeDto {
  return { id, namespace, flow_id: flowId, disabled: false, in_namespace: true, dynamic_subflows: 0, unparsable: false, ...over };
}

export const edge = (source: string, target: string, kind: NamespaceDependencyEdgeDto["kind"]): NamespaceDependencyEdgeDto => ({ source, target, kind });

/**
 * `company.team`'s graph, in the server's order (namespace, then flow id):
 *
 * - `company.other.caller` (outside) calls `extract` through a `Subflow`;
 * - `extract`'s end triggers `company.team.etl.load` (a child namespace: inside);
 * - `sync` (disabled, two dynamic `Subflow`s) calls `load`;
 * - `broken`'s draft does not parse, and nothing reaches it.
 *
 * `caller`'s id needs encoding in a URL, so opening it proves the id is encoded.
 */
export const DEPS: NamespaceDependenciesDto = {
  nodes: [
    flow("f 5", "company.other", "caller", { in_namespace: false }),
    flow("f4", "company.team", "broken", { unparsable: true }),
    flow("f1", "company.team", "extract"),
    flow("f3", "company.team", "sync", { disabled: true, dynamic_subflows: 2 }),
    flow("f2", "company.team.etl", "load", { dynamic_subflows: 1 }),
  ],
  edges: [edge("f 5", "f1", "subflow"), edge("f1", "f2", "flow_trigger"), edge("f3", "f2", "subflow")],
  unparsable: 3,
};

/** `n` flows, each calling the next: one connected group too long for dagre. */
export function chainOf(n: number): NamespaceDependenciesDto {
  const nodes = Array.from({ length: n }, (_, i) => flow(`c${i}`, "big", `flow_${String(i).padStart(4, "0")}`));
  const edges = nodes.slice(1).map((m, i) => edge(nodes[i].id, m.id, "subflow"));
  return { nodes, edges, unparsable: 0 };
}
