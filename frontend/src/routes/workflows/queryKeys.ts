import type { WorkflowListFlowsParams } from "@/api/generated/model";

/**
 * Every workflow query key, prefixed `["workflows", …]` (FE-DATA-2, spec §8).
 *
 * The orval hooks are called with these keys (`query: { queryKey }`), so a
 * mutation invalidates through this factory and never through a URL-shaped
 * key it would have to rebuild. `all` reaches every one of them.
 *
 * **The workspace is part of every key that a workspace scopes.** The flow
 * list and the schema are per workspace (the schema is filtered by the
 * workspace's features), and the query cache outlives a workspace switch: a
 * key without the slug would hand workspace B the list it cached for A for
 * the whole of `queryClient`'s `staleTime`. The keys taking an id do not need
 * it — a flow, version or execution id is a UUID, unique across workspaces.
 *
 * The slug sits behind a literal `"ws"` so that no workspace slug can spell
 * the prefix of an id-keyed key: a workspace called `flow` would otherwise
 * share `["workflows", "flow"]` with every flow.
 */
export const workflowsKeys = {
  all: ["workflows"] as const,
  flows: (ws: string, params?: WorkflowListFlowsParams) =>
    params ? (["workflows", "ws", ws, "flows", params] as const) : (["workflows", "ws", ws, "flows"] as const),
  flow: (id: string) => ["workflows", "flow", id] as const,
  versions: (id: string) => ["workflows", "flow", id, "versions"] as const,
  version: (id: string, v: number) => ["workflows", "flow", id, "versions", v] as const,
  schema: (ws: string) => ["workflows", "ws", ws, "schema"] as const,
  execution: (id: string) => ["workflows", "execution", id] as const,
  /**
   * The direct children of an execution (`GET …/executions?parent=`), under
   * `execution(id)` so refreshing the execution refreshes them too.
   */
  executionChildren: (id: string) => ["workflows", "execution", id, "children"] as const,
  /**
   * The execution's graph with its states (`GET …/executions/{id}/graph`,
   * W7c spec §4), per set of expanded `Subflow`s, under `execution(id)`.
   */
  executionGraph: (id: string, subflows: readonly string[]) => ["workflows", "execution", id, "graph", subflows] as const,
  /**
   * One file of an execution (`path` is its `kestra:///` URI), under
   * `execution(id)`: every read of it (`metas`, a `preview` per row cap) is
   * keyed beneath this one.
   */
  executionFile: (id: string, path: string) => ["workflows", "execution", id, "file", path] as const,
  executionFileMetas: (id: string, path: string) => ["workflows", "execution", id, "file", path, "metas"] as const,
  executionFilePreview: (id: string, path: string, maxRows: number) =>
    ["workflows", "execution", id, "file", path, "preview", maxRows] as const,
  /**
   * A page of the executions list. `params` is what the list was asked for
   * (`ExecutionsList`'s filters, the pinned flow, the expanded namespaces), not
   * the request: a time preset is turned into `since` at fetch time, so the key
   * stays the same between polls. Without `params` it reaches every page.
   */
  /**
   * The namespaces list (`GET …/namespaces`, W7d spec §3.1), and with
   * `prefix` one subtree of it (the Overview's child namespaces). Without
   * `prefix` the key is a prefix of every subtree's, so invalidating it
   * reaches them all.
   */
  namespaces: (ws: string, prefix?: string) =>
    prefix === undefined
      ? (["workflows", "ws", ws, "namespaces"] as const)
      : (["workflows", "ws", ws, "namespaces", { prefix }] as const),
  /**
   * One namespace (`GET …/namespaces/{ns}`). Under `ws`: a namespace is a name,
   * not a UUID, and repeats across workspaces. Placed beside the list rather
   * than under it, so a subtree's key never collides with a namespace called
   * like a filter.
   */
  namespace: (ws: string, namespace: string) => ["workflows", "ws", ws, "namespace", namespace] as const,
  /**
   * A namespace's live KV keys (`GET …/namespaces/{ns}/kv`), under
   * `namespace(ws, ns)`: invalidating the namespace (its counts) refreshes
   * them, and invalidating this reaches every value and the inherited list
   * below it.
   */
  kv: (ws: string, namespace: string) => ["workflows", "ws", ws, "namespace", namespace, "kv"] as const,
  /** One key's latest value (`GET …/kv/{key}`). */
  kvValue: (ws: string, namespace: string, key: string) =>
    ["workflows", "ws", ws, "namespace", namespace, "kv", "value", key] as const,
  /** The keys inherited from the namespace's parents (`GET …/kv/inheritance`). */
  kvInherited: (ws: string, namespace: string) => ["workflows", "ws", ws, "namespace", namespace, "kv", "inherited"] as const,
  executions: (ws: string, params?: object) =>
    params
      ? (["workflows", "ws", ws, "executions", params] as const)
      : (["workflows", "ws", ws, "executions"] as const),
};
