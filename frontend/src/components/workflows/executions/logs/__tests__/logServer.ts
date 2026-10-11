import { vi } from "vitest";
import type { LogLevel, LogLineDto, LogPageDto } from "@/api/generated/model";

/**
 * `GET …/executions/{id}/logs` from memory, through a stubbed `fetch`, so the
 * hook and the view run the real orval client, mutator and `apiFetch`
 * (FE-TEST-1). Every request's query is recorded.
 *
 * Two modes. `replies` answers reads in turn (a scripted sequence, for the
 * cursor tests). Otherwise it serves `lines` the way the route does: the lines
 * after `after`, filtered, at most `limit`, `next_after` the last id or null.
 */
export const WS = "eng";
export const EXEC = "5e5e5e5e-0000-4000-8000-0000000000aa";
export const LOGS_PATH = `/api/ws/${WS}/workflows/executions/${EXEC}/logs`;

type Reply = { status: number; body: unknown };

const RANK: Record<LogLevel, number> = { TRACE: 0, DEBUG: 1, INFO: 2, WARN: 3, ERROR: 4 };

let seq = 0;
export function line(id: number, extra: Partial<LogLineDto> = {}): LogLineDto {
  seq++;
  return {
    id,
    task_id: null,
    task_run_id: null,
    attempt: null,
    level: "INFO",
    ts: `2026-10-10T10:00:${String(id % 60).padStart(2, "0")}.${String(seq % 1000).padStart(3, "0")}Z`,
    message: `line ${id}`,
    ...extra,
  };
}

export function page(items: LogLineDto[], extra: Partial<LogPageDto> = {}): Reply {
  return {
    status: 200,
    body: {
      items,
      next_after: items.length ? items[items.length - 1].id : null,
      execution_state: "RUNNING",
      complete: false,
      ...extra,
    } satisfies LogPageDto,
  };
}

export class LogServer {
  queries: URLSearchParams[] = [];
  /** Scripted answers, in turn; the last is repeated. */
  replies: Reply[] = [];
  /** Served when no reply is scripted. */
  lines: LogLineDto[] = [];
  state = "RUNNING";
  complete = false;
  /** How many of the next reads fail with 503. */
  failures = 0;
  /** Other routes, `GET` by path (the execution page's own read). */
  others = new Map<string, Reply>();
  private served = 0;

  install(): this {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string, init?: RequestInit) => {
        // As a real fetch does: an aborted request rejects.
        init?.signal?.throwIfAborted();
        const [path, qs] = String(input).split("?");
        if (path !== LOGS_PATH) {
          const other = this.others.get(path) ?? { status: 404, body: { error: `no route ${path}` } };
          return new Response(JSON.stringify(other.body), { status: other.status, headers: { "content-type": "application/json" } });
        }
        const q = new URLSearchParams(qs ?? "");
        this.queries.push(q);
        const reply = this.reply(q);
        return new Response(JSON.stringify(reply.body), { status: reply.status, headers: { "content-type": "application/json" } });
      }),
    );
    return this;
  }

  private reply(q: URLSearchParams): Reply {
    if (this.failures > 0) {
      this.failures--;
      return { status: 503, body: { error: "the database is unavailable" } };
    }
    if (this.replies.length > 0) return this.replies[Math.min(this.served++, this.replies.length - 1)];
    const after = q.has("after") ? Number(q.get("after")) : -Infinity;
    const limit = Number(q.get("limit") ?? 500);
    const min = q.get("min_level") as LogLevel | null;
    const items = this.lines
      .filter((l) => l.id > after)
      .filter((l) => !min || RANK[l.level] >= RANK[min])
      .filter((l) => !q.get("task_id") || l.task_id === q.get("task_id"))
      .filter((l) => !q.get("task_run_id") || l.task_run_id === q.get("task_run_id"))
      .slice(0, limit);
    return page(items, { execution_state: this.state, complete: this.complete });
  }

  /** The `after` each read sent (`null` when it sent none). */
  afters(): (number | null)[] {
    return this.queries.map((q) => (q.has("after") ? Number(q.get("after")) : null));
  }
}
