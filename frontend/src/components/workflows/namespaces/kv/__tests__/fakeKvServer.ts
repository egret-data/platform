import { vi } from "vitest";
import type { KvEntryDto, KvInheritedDto } from "@/api/generated/model";

/**
 * W9c's KV routes for one namespace, served from memory through a stubbed
 * `fetch`, so the tab runs its real generated client, `apiFetch` and mutator
 * (FE-TEST-1). Every request is recorded with its method, its path as sent
 * (still encoded), its headers and its body.
 *
 * A key's value is held as the raw JSON text the server would send
 * (`values`), so a number with more digits than a JS number holds reaches
 * the page exactly as the server writes it. A PUT stores nothing: what the
 * server would infer from the body is W9c's business, pinned by its tests;
 * these pin what the page SENDS.
 */
export const KV_WS = "eng";

export type KvReply = { status: number; body?: unknown; text?: string };

export interface KvRequest {
  method: string;
  path: string;
  headers: Headers;
  body: string | undefined;
}

export interface KvWorld {
  namespace: string;
  keys: KvEntryDto[];
  /** `GET …/kv/{key}`'s raw JSON body, by key. */
  values?: Record<string, string>;
  inherited?: KvInheritedDto[];
  /** Answers every `GET …/kv` with this instead. */
  listReply?: KvReply | null;
  /** Answers every PUT with this instead of 200. */
  putReply?: KvReply | Error | null;
  /** Answers every DELETE with this instead. */
  deleteReply?: KvReply | null;
  /** While set, every `GET …/kv/{key}` waits for it (a slow read, so its fetching state paints). */
  valueGate?: Promise<void> | null;
}

export function kvEntry(key: string, over: Partial<KvEntryDto> = {}): KvEntryDto {
  return {
    key,
    type: "STRING",
    size: 7,
    description: null,
    expiresAt: null,
    revision: 1,
    updated: "2026-10-01T00:00:00Z",
    ...over,
  };
}

/** A `GET …/kv/{key}` body, its `value` given as raw JSON text. */
export function kvDetail(type: string, rawValue: string, over: { description?: string | null; expiresAt?: string | null; revision?: number } = {}): string {
  const meta = {
    revision: over.revision ?? 3,
    updated: "2026-10-05T06:07:08Z",
    description: over.description ?? null,
    expiresAt: over.expiresAt ?? null,
  };
  return `{"type":${JSON.stringify(type)},"value":${rawValue},${JSON.stringify(meta).slice(1)}`;
}

export function serveKv(world: KvWorld): KvRequest[] {
  const requests: KvRequest[] = [];
  const base = `/api/ws/${KV_WS}/workflows/namespaces/${encodeURIComponent(world.namespace)}/kv`;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init: RequestInit = {}) => {
      const path = String(input).split("?")[0];
      const method = (init.method ?? "GET").toUpperCase();
      const body = typeof init.body === "string" ? init.body : undefined;
      requests.push({ method, path, headers: new Headers(init.headers), body });
      const json = (r: KvReply) =>
        new Response(r.text ?? (r.body === undefined ? null : JSON.stringify(r.body)), {
          status: r.status,
          headers: r.body === undefined && r.text === undefined ? {} : { "content-type": "application/json" },
        });
      if (path === base && method === "GET") return json(world.listReply ?? { status: 200, body: world.keys });
      if (path === base && method === "DELETE") {
        if (world.deleteReply) return json(world.deleteReply);
        const asked = (JSON.parse(body ?? "{}") as { keys: string[] }).keys;
        const gone = asked.filter((k) => world.keys.some((e) => e.key === k)).sort();
        world.keys = world.keys.filter((e) => !gone.includes(e.key));
        return json({ status: 200, body: { keys: gone } });
      }
      if (path === `${base}/inheritance` && method === "GET") return json({ status: 200, body: world.inherited ?? [] });
      if (path.startsWith(`${base}/`)) {
        const key = decodeURIComponent(path.slice(base.length + 1));
        if (method === "GET") {
          if (world.valueGate) await world.valueGate;
          const text = world.values?.[key];
          return text === undefined
            ? json({ status: 404, body: { error: `No value found for key '${key}' in namespace '${world.namespace}'` } })
            : json({ status: 200, text });
        }
        if (method === "PUT") {
          if (world.putReply instanceof Error) throw world.putReply;
          return json(world.putReply ?? { status: 200 });
        }
        if (method === "DELETE") {
          if (world.deleteReply) return json(world.deleteReply);
          const was = world.keys.some((e) => e.key === key);
          world.keys = world.keys.filter((e) => e.key !== key);
          return json({ status: 200, body: was });
        }
      }
      return json({ status: 404, body: { error: `unexpected ${method} ${path}` } });
    }),
  );
  return requests;
}

/** The `description` header as the server reads it: its bytes as UTF-8. */
export function headerUtf8(value: string | null): string | null {
  if (value === null) return null;
  return new TextDecoder().decode(Uint8Array.from([...value].map((c) => c.charCodeAt(0))));
}
