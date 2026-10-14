import { vi } from "vitest";
import type { NsFileRevisionDto, NsFileStatsDto } from "@/api/generated/model";

/**
 * W9b's namespace file routes for one namespace, served from memory through
 * a stubbed `fetch`, so `NamespaceFiles` runs its real generated client,
 * mutator and `apiFetch` (FE-TEST-1).
 *
 * The store keeps W9b's shape: a file is a list of revisions (the latest
 * last; a soft delete flags it), a directory is a path ending in `/`. Writes
 * follow W9b and W7d Task 2: `if_revision` must equal the latest LIVE
 * revision (0 for none) or the answer is 409 `file_changed`; a directory at
 * the path is a 409 without that code; a move onto a live path is a 409.
 *
 * Every request is recorded with its method, its path as sent (still
 * encoded), its query and, for an upload, the text of the uploaded part.
 */
export const FILES_WS = "eng";

export type FilesReply = { status: number; body?: unknown };

export interface FilesRequest {
  method: string;
  path: string;
  /** The route below `…/files`: "", "/directory", "/stats", "/search", "/revisions". */
  route: string;
  query: URLSearchParams;
  /** An upload's file part, as text. */
  upload?: string;
}

interface Rev {
  revision: number;
  bytes: Uint8Array;
  deleted: boolean;
  created: string;
}

export interface FilesWorld {
  namespace: string;
  /** Path → content (a string is UTF-8; give `Uint8Array` for bytes). Each becomes revision 1, or `revisions`. */
  files: Record<string, string | Uint8Array | (string | Uint8Array)[]>;
  /** Directory paths ending in `/`; parents of files are added. */
  dirs?: string[];
  /** Answers every request to this route instead (by route, e.g. "/directory"). */
  replies?: Record<string, FilesReply>;
  /** Answers an upload of this path instead. */
  uploadReplies?: Record<string, FilesReply>;
  /** While set, every download waits for it. */
  downloadGate?: Promise<void> | null;
  /** While set, every upload (`POST …/files`) waits for it. */
  uploadGate?: Promise<void> | null;
}

const enc = new TextEncoder();
const bytesOf = (v: string | Uint8Array) => (typeof v === "string" ? enc.encode(v) : v);
const parentOf = (p: string) => {
  const trimmed = p.endsWith("/") ? p.slice(0, -1) : p;
  return trimmed.slice(0, trimmed.lastIndexOf("/") + 1) || "/";
};

export class FilesStore {
  files = new Map<string, Rev[]>();
  dirs = new Set<string>();
  private clock = 0;

  constructor(world: FilesWorld) {
    for (const d of world.dirs ?? []) this.addDir(d);
    for (const [path, content] of Object.entries(world.files)) {
      const all = Array.isArray(content) ? content : [content];
      this.files.set(
        path,
        all.map((c, i) => ({ revision: i + 1, bytes: bytesOf(c), deleted: false, created: this.stamp() })),
      );
      this.addDir(parentOf(path));
    }
  }

  stamp(): string {
    this.clock += 1;
    return new Date(Date.UTC(2026, 9, 1, 0, 0, this.clock)).toISOString();
  }

  addDir(dir: string): void {
    for (let d = dir; d !== "/"; d = parentOf(d)) this.dirs.add(d);
  }

  /** The latest live revision of a file, or undefined. */
  live(path: string): Rev | undefined {
    const revs = this.files.get(path);
    const last = revs?.[revs.length - 1];
    return last && !last.deleted ? last : undefined;
  }

  text(path: string): string | undefined {
    const r = this.live(path);
    return r ? new TextDecoder().decode(r.bytes) : undefined;
  }

  write(path: string, bytes: Uint8Array): number {
    const revs = this.files.get(path) ?? [];
    const revision = (revs[revs.length - 1]?.revision ?? 0) + 1;
    revs.push({ revision, bytes, deleted: false, created: this.stamp() });
    this.files.set(path, revs);
    this.addDir(parentOf(path));
    return revision;
  }

  stats(path: string): NsFileStatsDto | undefined {
    const f = this.live(path);
    if (f) return { path, type: "File", size: f.bytes.length, revision: f.revision, created: f.created };
    if (this.dirs.has(path)) return { path, type: "Directory", size: 0, revision: 1, created: "2026-10-01T00:00:00Z" };
    return undefined;
  }

  children(dir: string): NsFileStatsDto[] {
    const out: NsFileStatsDto[] = [];
    for (const d of this.dirs) if (parentOf(d) === dir) out.push(this.stats(d)!);
    for (const p of this.files.keys()) if (this.live(p) && parentOf(p) === dir) out.push(this.stats(p)!);
    // The server's byte order, which mixes files and directories.
    return out.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
  }
}

const json = (r: FilesReply) =>
  new Response(r.body === undefined ? null : JSON.stringify(r.body), {
    status: r.status,
    headers: r.body === undefined ? {} : { "content-type": "application/json" },
  });

export function serveFiles(world: FilesWorld): { requests: FilesRequest[]; store: FilesStore } {
  const requests: FilesRequest[] = [];
  const store = new FilesStore(world);
  const base = `/api/ws/${FILES_WS}/workflows/namespaces/${encodeURIComponent(world.namespace)}/files`;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string, init: RequestInit = {}) => {
      const raw = String(input);
      const path = raw.split("?")[0];
      const query = new URL(raw, "http://x").searchParams;
      const method = (init.method ?? "GET").toUpperCase();
      if (!path.startsWith(base)) return json({ status: 404, body: { error: `unexpected ${path}` } });
      const route = path.slice(base.length);
      const request: FilesRequest = { method, path, route, query };
      requests.push(request);
      if (init.body instanceof FormData) {
        const part = init.body.get("fileContent") as Blob;
        request.upload = await part.text();
      }
      const override = world.replies?.[route];
      if (override) return json(override);
      const p = query.get("path") ?? "";

      if (method === "GET" && route === "/directory") {
        const dir = p || "/";
        if (dir !== "/" && !store.dirs.has(dir)) {
          if (store.live(dir)) return json({ status: 400, body: { error: `'${dir}' is a file, not a directory` } });
          return json({ status: 404, body: { error: `directory '${dir}' does not exist` } });
        }
        return json({ status: 200, body: store.children(dir) });
      }
      if (method === "GET" && route === "/stats") {
        const s = store.stats(p);
        return s ? json({ status: 200, body: s }) : json({ status: 404, body: { error: `file '${p}' does not exist` } });
      }
      if (method === "GET" && route === "/search") {
        const q = (query.get("q") ?? "").toLowerCase();
        const found = [...store.files.keys()].filter((f) => store.live(f) && q && f.toLowerCase().includes(q)).sort();
        return json({ status: 200, body: found });
      }
      if (method === "GET" && route === "/revisions") {
        const revs = store.files.get(p);
        if (!revs) return json({ status: 404, body: { error: `file '${p}' does not exist` } });
        const body: NsFileRevisionDto[] = revs.map((r) => ({ revision: r.revision, deleted: r.deleted, size: r.bytes.length, created: r.created }));
        return json({ status: 200, body });
      }
      if (method === "GET" && route === "") {
        if (world.downloadGate) await world.downloadGate;
        const wanted = query.get("revision");
        const rev = wanted ? store.files.get(p)?.find((r) => r.revision === Number(wanted)) : store.live(p);
        if (!rev) return json({ status: 404, body: { error: `file '${p}' does not exist` } });
        // An ArrayBuffer: Node's `Response` cannot read jsdom's `Blob`.
        return new Response(rev.bytes.slice().buffer, { status: 200, headers: { "content-type": "application/octet-stream" } });
      }
      if (method === "POST" && route === "") {
        if (world.uploadGate) await world.uploadGate;
        const reply = world.uploadReplies?.[p];
        if (reply) return json(reply);
        if (store.dirs.has(`${p}/`)) {
          return json({ status: 409, body: { error: `conflict: a directory exists at '${p}/'` } });
        }
        const expected = query.get("if_revision");
        const current = store.live(p)?.revision ?? 0;
        if (expected !== null && Number(expected) !== current) {
          const deleted = !store.live(p) && store.files.has(p);
          // W7d Task 2's sentences (`storage/nsfiles/routes.rs`, `file_changed`), behind `Error::Conflict`'s prefix.
          const where = `namespace file '${p}' in namespace '${world.namespace}'`;
          const error = deleted
            ? `conflict: ${where} was deleted (at revision ${store.files.get(p)!.at(-1)!.revision}); nothing was written`
            : current === 0
              ? `conflict: ${where} does not exist; nothing was written`
              : Number(expected) === 0
                ? `conflict: namespace file '${p}' already exists in namespace '${world.namespace}' (revision ${current}); nothing was written`
                : `conflict: ${where} was changed: it is at revision ${current}, not ${expected}; nothing was written`;
          return json({ status: 409, body: { error, code: "file_changed" } });
        }
        store.write(p, enc.encode(request.upload ?? ""));
        return json({ status: 200 });
      }
      if (method === "POST" && route === "/directory") {
        store.addDir(p);
        return json({ status: 200 });
      }
      if (method === "PUT" && route === "") {
        const from = query.get("from") ?? "";
        const to = query.get("to") ?? "";
        if (store.stats(to)) return json({ status: 409, body: { error: `conflict: '${to}' already exists` } });
        if (from.endsWith("/")) {
          if (!store.dirs.has(from)) return json({ status: 404, body: { error: `'${from}' does not exist` } });
          for (const d of [...store.dirs]) if (d.startsWith(from)) {
            store.dirs.delete(d);
            store.dirs.add(to + d.slice(from.length));
          }
          for (const [f, revs] of [...store.files]) if (f.startsWith(from)) {
            store.files.delete(f);
            store.files.set(to + f.slice(from.length), revs);
          }
          store.addDir(to);
        } else {
          const revs = store.files.get(from);
          if (!revs || !store.live(from)) return json({ status: 404, body: { error: `'${from}' does not exist` } });
          store.files.delete(from);
          store.files.set(to, revs);
          store.addDir(parentOf(to));
        }
        return json({ status: 200 });
      }
      if (method === "DELETE" && route === "") {
        if (p.endsWith("/")) {
          for (const d of [...store.dirs]) if (d.startsWith(p)) store.dirs.delete(d);
          for (const [f, revs] of store.files) if (f.startsWith(p) && store.live(f)) revs[revs.length - 1].deleted = true;
        } else {
          const revs = store.files.get(p);
          if (revs && store.live(p)) revs[revs.length - 1].deleted = true;
        }
        return json({ status: 200 });
      }
      return json({ status: 404, body: { error: `unexpected ${method} ${path}` } });
    }),
  );
  return { requests, store };
}
