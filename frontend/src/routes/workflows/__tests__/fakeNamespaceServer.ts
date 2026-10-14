import { vi } from "vitest";
import type { ExecutionListItemDto, FlowListItemDto, WorkflowNamespaceDto } from "@/api/generated/model";
import { namespaceContains } from "@/components/workflows/namespaceTree";

/**
 * The routes the namespace pages call, served from memory through a stubbed
 * `fetch`, so the pages run their real orval hooks, mutator and `apiFetch`
 * (FE-TEST-1). Every request is recorded, with its query, in `requests`.
 *
 * `GET …/namespaces?prefix=` filters dot-anchored, as the server does
 * (W7d spec §3.1); `GET …/namespaces/{ns}` answers the row or 404.
 */
export const NS_WS = "eng";

type Reply = { status: number; body: unknown };
export type Request = { path: string; query: URLSearchParams };

export function nsRow(namespace: string, over: Partial<WorkflowNamespaceDto> = {}): WorkflowNamespaceDto {
  return { namespace, flows: 0, files: 0, kv_keys: 0, own: true, ...over };
}

export function nsFlow(namespace: string, flow_id: string): FlowListItemDto {
  return {
    id: `id-${namespace}-${flow_id}`,
    namespace,
    flow_id,
    state: "draft",
    created_at: "2026-10-01T00:00:00Z",
    updated_at: "2026-10-02T00:00:00Z",
  };
}

export function nsExecution(id: string, namespace: string, flow_key: string): ExecutionListItemDto {
  return {
    id,
    flow_id: `id-${namespace}-${flow_key}`,
    namespace,
    flow_key,
    state: "SUCCESS",
    history: ["CREATED", "RUNNING", "SUCCESS"],
    inputs: {},
    seq: 1,
    execution_trigger: null,
    attempt_number: 1,
    origin_offset_ms: 0,
    started_at: "2026-10-11T10:00:05.000001Z",
    ended_at: "2026-10-11T10:01:10.000001Z",
    created_at: "2026-10-11T10:00:00.000001Z",
    updated_at: "2026-10-11T10:01:10.000001Z",
  };
}

export interface NamespaceWorld {
  namespaces: WorkflowNamespaceDto[];
  flows?: FlowListItemDto[];
  executions?: ExecutionListItemDto[];
  /** Answers every `GET …/namespaces` with this instead. */
  listReply?: Reply | null;
  /** Answers every `GET …/namespaces/{ns}` with this instead. */
  detailReply?: Reply | null;
  /** While set, `GET …/namespaces` waits for it. */
  listGate?: Promise<void> | null;
  /** The detail's `can_manage` (true when unset). */
  canManage?: boolean;
}

export function serveNamespaces(world: NamespaceWorld): Request[] {
  const requests: Request[] = [];
  const base = `/api/ws/${NS_WS}/workflows`;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string) => {
      const url = new URL(String(input), "http://x");
      // The path as sent, still encoded: the namespace segment is checked raw.
      const rawPath = String(input).split("?")[0];
      requests.push({ path: rawPath, query: url.searchParams });
      let reply: Reply;
      if (rawPath === `${base}/namespaces`) {
        if (world.listGate) await world.listGate;
        const prefix = url.searchParams.get("prefix");
        reply = world.listReply ?? {
          status: 200,
          body: world.namespaces.filter((r) => !prefix || namespaceContains(prefix, r.namespace)),
        };
      } else if (/^\/api\/ws\/[^/]+\/workflows\/namespaces\/[^/]+\/kv$/.test(rawPath)) {
        // The KV tab's list (W9c): an empty namespace.
        reply = { status: 200, body: [] };
      } else if (/^\/api\/ws\/[^/]+\/workflows\/namespaces\/[^/]+\/files\/directory$/.test(rawPath)) {
        // The Files tab's root listing (W9b): an empty namespace.
        reply = { status: 200, body: [] };
      } else if (/^\/api\/ws\/[^/]+\/workflows\/namespaces\/[^/]+\/dependencies$/.test(rawPath)) {
        // The Dependencies tab's graph (W7d §3.4): an empty namespace.
        reply = { status: 200, body: { nodes: [], edges: [], unparsable: 0 } };
      } else if (rawPath.startsWith(`${base}/namespaces/`)) {
        const ns = decodeURIComponent(rawPath.slice(`${base}/namespaces/`.length));
        const found = world.namespaces.find((r) => r.namespace === ns);
        reply =
          world.detailReply ??
          (found
            ? { status: 200, body: { ...found, can_manage: world.canManage ?? true } }
            : { status: 404, body: { error: `not found: namespace ${ns}` } });
      } else if (rawPath === `${base}/flows`) {
        reply = { status: 200, body: world.flows ?? [] };
      } else if (rawPath === `${base}/executions`) {
        const prefix = url.searchParams.get("namespace_prefix");
        const rows = (world.executions ?? []).filter((e) => !prefix || namespaceContains(prefix, e.namespace));
        const limit = Number(url.searchParams.get("limit") ?? 50);
        reply = { status: 200, body: { rows: rows.slice(0, limit), total: rows.length, limit, offset: 0 } };
      } else {
        reply = { status: 404, body: { error: `unexpected ${rawPath}` } };
      }
      return new Response(JSON.stringify(reply.body), { status: reply.status, headers: { "content-type": "application/json" } });
    }),
  );
  return requests;
}
