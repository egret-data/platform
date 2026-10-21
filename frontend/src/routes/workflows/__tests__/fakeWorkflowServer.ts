import { vi } from "vitest";
import type {
  BulkResultDto,
  ExecutionListItemDto,
  FlowDto,
  FlowTriggersDto,
  FlowVersionDto,
  CreateBackfillRequest,
  SetBackfillPausedRequest,
  SetTriggerDisabledRequest,
  SetTriggersDisabledRequest,
  TriggerEntryDto,
  ValidationReportDto,
} from "@/api/generated/model";

/**
 * The workflow routes the flow pages call, served from memory through a
 * stubbed `fetch`, so the pages run their real orval hooks, mutator and
 * `apiFetch` (FE-TEST-1). Every request is recorded in `calls`.
 *
 * `updated_at` values carry microseconds, as the server's do, so a page that
 * parsed and re-formatted one (a JS `Date` keeps milliseconds) would send a
 * different string and be refused.
 */
export type Call = { method: string; path: string; body: unknown; query?: URLSearchParams };
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
  /** Answers the next `PUT …/flows/{id}` with this instead of saving. */
  putReply: Reply | null = null;
  /** How many of the next `GET …/flows/{id}` fail with 503. */
  getFlowFailures = 0;
  /** Answers `GET …/flows/{id}/versions` with this instead of the versions. */
  versionsReply: Reply | null = null;
  /** What `GET …/executions` lists, unfiltered: the flow's Executions tab reads it. */
  executions: ExecutionListItemDto[] = [];
  /** While set, `GET …/flows/{id}/versions` waits for it before answering. */
  versionsGate: Promise<void> | null = null;
  /** While set, `PUT …/flows/{id}` waits for it before answering. */
  putGate: Promise<void> | null = null;
  /** While set, `POST …/flows` waits for it before answering. */
  createGate: Promise<void> | null = null;
  /**
   * What `GET …/flows/{id}/triggers` answers (W7e), and what the switch routes change: a
   * `PUT …/triggers/{trigger_id}/disabled` or `PUT …/workflows/triggers/disabled` flips the
   * entry's `runtime_disabled` as the server does. Each request's body is in `calls`.
   */
  triggers: FlowTriggersDto = {
    flow_disabled: false,
    flow_deleted: false,
    scheduler_enabled: true,
    worker_enabled: true,
    webhooks_enabled: true,
    triggers: [],
  };
  /** Answers `GET …/flows/{id}/triggers` with this instead of `triggers`. */
  triggersReply: Reply | null = null;
  /** Answers the next `PUT …/triggers/{trigger_id}/disabled` with this instead of switching. */
  triggerPutReply: Reply | null = null;
  /** Answers the next `POST …/triggers/{trigger_id}/backfill` with this instead of creating. */
  backfillCreateReply: Reply | null = null;
  /** Answers the next `PUT …/workflows/triggers/disabled` with this instead of switching. */
  bulkReply: Reply | null = null;
  /** Bulk entries refused by trigger id, with the server's code and sentence. */
  bulkRefusals: Record<string, { code: string; message: string }> = {};

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
      latest_version: null,
      created_at: "2026-10-01T00:00:00.000001Z",
      updated_at: "2026-10-10T09:47:08.618779Z",
      validation: { errors: [], warnings: [] },
      can_manage: true,
      worker_enabled: true,
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
    this.flow = { ...flow, state: "live", live_version_id: version.id, latest_version: version.version, updated_at: stamp() };
    return version;
  }

  /** Someone else saves the draft. */
  saveElsewhere(source: string): void {
    this.flow = { ...this.flow!, source, updated_at: stamp() };
  }

  callsTo(method: string, path: string): Call[] {
    return this.calls.filter((c) => c.method === method && c.path === path);
  }

  /** Switch one trigger as `set_trigger_disabled` does; undefined when the flow declares no such id. */
  private switchOne(triggerId: string, req: SetTriggerDisabledRequest): TriggerEntryDto | undefined {
    const at = this.triggers.triggers.findIndex((t) => t.trigger_id === triggerId);
    if (at < 0) return undefined;
    const old = this.triggers.triggers[at];
    const next: TriggerEntryDto = {
      ...old,
      runtime_disabled: req.disabled,
      disabled_by_name: req.disabled ? "Ana Lima" : null,
      disabled_at: req.disabled ? new Date().toISOString() : null,
      effective_disabled: req.disabled || old.yaml_disabled || this.triggers.flow_disabled || this.triggers.flow_deleted,
    };
    this.triggers = { ...this.triggers, triggers: this.triggers.triggers.map((t, i) => (i === at ? next : t)) };
    return next;
  }

  /**
   * The per-trigger backfill routes (W7f §3.5): pause or resume, and delete. Only the backfill
   * entry changes; the answer is the trigger's entry, as the server returns it.
   */
  private backfillRoute(triggerId: string, method: string, body: unknown): Reply {
    const at = this.triggers.triggers.findIndex((t) => t.trigger_id === triggerId);
    if (at < 0) return { status: 404, body: { error: "not found: trigger", code: "not_found" } };
    const old = this.triggers.triggers[at];
    if (method === "POST") {
      if (this.backfillCreateReply) {
        const reply = this.backfillCreateReply;
        this.backfillCreateReply = null;
        return reply;
      }
      if (old.backfill) return { status: 409, body: { error: "This trigger already has a backfill; pause or delete it first", code: "backfill_exists" } };
      const req = body as CreateBackfillRequest;
      const end = req.end ?? req.start;
      const created: TriggerEntryDto = {
        ...old,
        backfill: {
          id: "00000000-0000-4000-8000-0000000000c1",
          start: req.start,
          end,
          current_tick: req.start,
          paused: false,
          paused_by_name: null,
          paused_at: null,
          inputs: req.inputs ?? {},
          labels: req.labels ?? [],
          created_by_name: "Ana Lima",
          created_at: req.start,
          previous_next_evaluation_date: req.start,
          total_ticks: 1,
          ticks_done: 0,
          progress: 0,
        },
      };
      this.triggers = { ...this.triggers, triggers: this.triggers.triggers.map((t, i) => (i === at ? created : t)) };
      return { status: 201, body: created };
    }
    if (!old.backfill) return { status: 404, body: { error: "This trigger has no backfill", code: "no_backfill" } };
    const next: TriggerEntryDto =
      method === "DELETE"
        ? { ...old, backfill: null }
        : { ...old, backfill: { ...old.backfill, paused: (body as SetBackfillPausedRequest).paused } };
    this.triggers = { ...this.triggers, triggers: this.triggers.triggers.map((t, i) => (i === at ? next : t)) };
    return { status: 200, body: next };
  }

  private switchBulk(req: SetTriggersDisabledRequest): Reply {
    if (this.bulkReply) {
      const reply = this.bulkReply;
      this.bulkReply = null;
      return reply;
    }
    const results: BulkResultDto[] = req.triggers.map(({ flow_id, trigger_id }) => {
      const refusal = this.bulkRefusals[trigger_id];
      if (refusal) return { flow_id, trigger_id, ok: false, code: refusal.code, message: refusal.message, trigger: null };
      const trigger = flow_id === FLOW_ID ? this.switchOne(trigger_id, req) : undefined;
      return trigger
        ? { flow_id, trigger_id, ok: true, code: null, message: null, trigger }
        : { flow_id, trigger_id, ok: false, code: "not_found", message: "not found: trigger", trigger: null };
    });
    return { status: 200, body: { results } };
  }

  install(): void {
    vi.stubGlobal("fetch", vi.fn(async (input: string, init?: RequestInit) => {
      const method = (init?.method ?? "GET").toUpperCase();
      const [path, qs] = String(input).split("?");
      const body = typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
      this.calls.push({ method, path, body, query: new URLSearchParams(qs ?? "") });
      if (method === "POST" && path === `/api/ws/${WS}/workflows/flows` && this.createGate) await this.createGate;
      if (method === "GET" && path.endsWith("/versions") && this.versionsGate) await this.versionsGate;
      if (method === "PUT" && this.putGate) await this.putGate;
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
    if (method === "GET" && path === `${base}/executions`) {
      return { status: 200, body: { rows: this.executions, total: this.executions.length, limit: 50, offset: 0, worker_enabled: this.flow?.worker_enabled ?? true } };
    }
    if (method === "POST" && (path === `${base}/flows/validate` || path === `${flowPath}/validate`)) return this.validation;
    if (method === "POST" && path === `${base}/flows`) {
      if (this.createReply) return this.createReply;
      const source = (body as { source: string }).source;
      this.flow = new FakeWorkflowServer({ source, updated_at: stamp() }).flow;
      return { status: 201, body: this.flow };
    }
    if (method === "GET" && path === `${base}/triggers`) {
      // The workspace's list (Task 7's page): the one flow's triggers as rows, unfiltered.
      const flow = this.flow;
      const rows = flow
        ? this.triggers.triggers.map((t) => ({
            ...t,
            flow_uuid: flow.id,
            namespace: flow.namespace,
            flow_id: flow.flow_id,
            flow_disabled: this.triggers.flow_disabled,
          }))
        : [];
      const { scheduler_enabled, worker_enabled, webhooks_enabled } = this.triggers;
      return { status: 200, body: { rows, total: rows.length, limit: 50, offset: 0, unparsable: 0, scheduler_enabled, worker_enabled, webhooks_enabled, can_manage: true } };
    }
    if (method === "PUT" && path === `${base}/triggers/disabled`) return this.switchBulk(body as SetTriggersDisabledRequest);
    if (!this.flow || !path.startsWith(flowPath)) return { status: 404, body: { error: "not found: flow" } };
    if (method === "GET" && path === `${flowPath}/triggers`) return this.triggersReply ?? { status: 200, body: this.triggers };
    const switchPath = new RegExp(`^${flowPath}/triggers/([^/]+)/disabled$`).exec(path);
    if (method === "PUT" && switchPath) {
      if (this.triggerPutReply) {
        const reply = this.triggerPutReply;
        this.triggerPutReply = null;
        return reply;
      }
      const switched = this.switchOne(decodeURIComponent(switchPath[1]), body as SetTriggerDisabledRequest);
      return switched ? { status: 200, body: switched } : { status: 404, body: { error: "not found: trigger" } };
    }
    const backfillPath = new RegExp(`^${flowPath}/triggers/([^/]+)/backfill(/paused)?$`).exec(path);
    if (backfillPath) return this.backfillRoute(decodeURIComponent(backfillPath[1]), method, body);
    if (method === "GET" && path === flowPath) {
      if (this.getFlowFailures > 0) {
        this.getFlowFailures--;
        return { status: 503, body: { error: "the database is unavailable" } };
      }
      return this.getFlowReply ?? { status: 200, body: this.flow };
    }
    if (method === "GET" && path === `${flowPath}/versions`) return this.versionsReply ?? { status: 200, body: this.versions };
    if (method === "PUT" && path === flowPath) {
      if (this.putReply) return this.putReply;
      const req = body as { source: string; if_updated_at?: string | null };
      if (req.if_updated_at != null && req.if_updated_at !== this.flow.updated_at) {
        return { status: 409, body: { error: "conflict: the draft was changed since you opened it", code: "draft_changed" } };
      }
      // As the server does, the saved draft's namespace is the YAML's.
      const namespace = /^namespace:\s*(\S+)\s*$/m.exec(req.source)?.[1] ?? this.flow.namespace;
      this.flow = { ...this.flow, source: req.source, namespace, updated_at: stamp() };
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
