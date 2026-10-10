import { vi } from "vitest";
import type { FlowDto, FlowVersionDto, ValidationReportDto } from "@/api/generated/model";

/**
 * The workflow routes the flow pages call, served from memory through a
 * stubbed `fetch`, so the pages run their real orval hooks, mutator and
 * `apiFetch` (FE-TEST-1). Every request is recorded in `calls`.
 *
 * `updated_at` values carry microseconds, as the server's do, so a page that
 * parsed and re-formatted one (a JS `Date` keeps milliseconds) would send a
 * different string and be refused.
 */
export type Call = { method: string; path: string; body: unknown };
type Reply = { status: number; body: unknown };

export const WS = "eng";
export const FLOW_ID = "0a7c1d2e-0000-4000-8000-000000000001";

export function flowSource(message: string): string {
  return `id: hello\nnamespace: company.team\ntasks:\n  - id: hello\n    type: io.kestra.plugin.core.log.Log\n    message: ${message}\n`;
}

let tick = 0;
/** A fresh `updated_at`, with microseconds that a `Date` round-trip would lose. */
export function stamp(): string {
  tick++;
  return `2026-10-10T09:47:${String(10 + (tick % 49)).padStart(2, "0")}.6187${String(tick % 100).padStart(2, "0")}Z`;
}

export class FakeWorkflowServer {
  flow: FlowDto | null;
  versions: FlowVersionDto[] = [];
  calls: Call[] = [];
  validation: Reply = { status: 200, body: { errors: [], warnings: [] } satisfies ValidationReportDto };
  /** Answers the next `POST …/flows` with this instead of creating. */
  createReply: Reply | null = null;
  getFlowReply: Reply | null = null;

  constructor(flow: Partial<FlowDto> | null = {}) {
    this.flow = flow === null ? null : {
      id: FLOW_ID,
      folder_id: null,
      namespace: "company.team",
      flow_id: "hello",
      source: flowSource("one"),
      plan: {},
      state: "draft",
      live_version_id: null,
      created_by: null,
      created_by_name: null,
      created_at: "2026-10-01T00:00:00.000001Z",
      updated_at: "2026-10-10T09:47:08.618779Z",
      validation: { errors: [], warnings: [] },
      can_manage: true,
      ...flow,
    };
  }

  /** Publish the current draft as the next version, as `POST …/publish` does. */
  publishNow(note = ""): FlowVersionDto {
    const flow = this.flow!;
    const version: FlowVersionDto = {
      id: `v-${this.versions.length + 1}`,
      flow_id: flow.id,
      version: this.versions.length + 1,
      snapshot: { source: flow.source, plan: {}, warnings: [] },
      note,
      created_at: stamp(),
    };
    this.versions.push(version);
    // `set_live` moves `updated_at` too.
    this.flow = { ...flow, state: "live", live_version_id: version.id, updated_at: stamp() };
    return version;
  }

  /** Someone else saves the draft. */
  saveElsewhere(source: string): void {
    this.flow = { ...this.flow!, source, updated_at: stamp() };
  }

  callsTo(method: string, path: string): Call[] {
    return this.calls.filter((c) => c.method === method && c.path === path);
  }

  install(): void {
    vi.stubGlobal("fetch", vi.fn(async (input: string, init?: RequestInit) => {
      const method = (init?.method ?? "GET").toUpperCase();
      const path = String(input).split("?")[0];
      const body = typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
      this.calls.push({ method, path, body });
      const { status, body: out, headers } = this.route(method, path, body);
      return new Response(out === undefined ? null : JSON.stringify(out), {
        status,
        headers: { "content-type": "application/json", ...headers },
      });
    }));
  }

  private route(method: string, path: string, body: unknown): Reply & { headers?: Record<string, string> } {
    const base = `/api/ws/${WS}/workflows`;
    const flowPath = `${base}/flows/${FLOW_ID}`;
    if (method === "GET" && path === `${base}/schema`) return { status: 200, body: {}, headers: { etag: '"s1"' } };
    if (method === "GET" && path === `${base}/flows`) return { status: 200, body: this.flow ? [this.flow] : [] };
    if (method === "POST" && (path === `${base}/flows/validate` || path === `${flowPath}/validate`)) return this.validation;
    if (method === "POST" && path === `${base}/flows`) {
      if (this.createReply) return this.createReply;
      const source = (body as { source: string }).source;
      this.flow = new FakeWorkflowServer({ source, updated_at: stamp() }).flow;
      return { status: 201, body: this.flow };
    }
    if (!this.flow || !path.startsWith(flowPath)) return { status: 404, body: { error: "not found: flow" } };
    if (method === "GET" && path === flowPath) return this.getFlowReply ?? { status: 200, body: this.flow };
    if (method === "GET" && path === `${flowPath}/versions`) return { status: 200, body: this.versions };
    if (method === "PUT" && path === flowPath) {
      const req = body as { source: string; if_updated_at?: string | null };
      if (req.if_updated_at != null && req.if_updated_at !== this.flow.updated_at) {
        return { status: 409, body: { error: "conflict: the draft was changed since you opened it" } };
      }
      this.flow = { ...this.flow, source: req.source, updated_at: stamp() };
      return { status: 200, body: this.flow };
    }
    if (method === "POST" && path === `${flowPath}/publish`) {
      return { status: 200, body: this.publishNow((body as { note?: string }).note ?? "") };
    }
    if (method === "DELETE" && path === flowPath) {
      this.flow = null;
      return { status: 200, body: { ok: true } };
    }
    return { status: 404, body: { error: `no route ${method} ${path}` } };
  }
}
