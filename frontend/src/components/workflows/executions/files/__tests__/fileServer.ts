import { vi } from "vitest";
import type { FileMetasDto, FilePreviewDto } from "@/api/generated/model";

/**
 * `GET …/executions/{id}/file/metas` and `…/file/preview` from memory, through
 * a stubbed `fetch`, so the rows and the drawer run the real orval client,
 * mutator and `apiFetch` (FE-TEST-1). Each file is keyed by its `path` (the
 * `kestra:///` URI); a path with no entry answers 404, as the route does for
 * a file pruned or outside the execution's loop family. Every request is
 * recorded with its query.
 */
export const WS = "eng";
export const EXEC = "5e5e5e5e-0000-4000-8000-0000000000aa";
export const EXEC_PATH = `/api/ws/${WS}/workflows/executions/${EXEC}`;

type Reply = { status: number; body: unknown };
export type FileCall = { path: string; query: URLSearchParams };

export const uri = (name: string) => `kestra:///company/team/hello/executions/${EXEC}/tasks/t1/r1/${name}`;

export class FileServer {
  calls: FileCall[] = [];
  metas = new Map<string, Reply>();
  /** By file path; the reply may depend on `maxRows`. */
  previews = new Map<string, (maxRows: number | null) => Reply>();
  /** Other routes, `GET` by path (the execution page's own read). */
  others = new Map<string, Reply>();

  file(path: string, size: number, preview?: FilePreviewDto | ((maxRows: number | null) => FilePreviewDto)): this {
    this.metas.set(path, { status: 200, body: { size } satisfies FileMetasDto });
    if (preview) {
      this.previews.set(path, (n) => ({ status: 200, body: typeof preview === "function" ? preview(n) : preview }));
    }
    return this;
  }

  install(): this {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string) => {
        const [path, qs] = String(input).split("?");
        const query = new URLSearchParams(qs ?? "");
        this.calls.push({ path, query });
        const reply = this.route(path, query);
        return new Response(JSON.stringify(reply.body), { status: reply.status, headers: { "content-type": "application/json" } });
      }),
    );
    return this;
  }

  private route(path: string, query: URLSearchParams): Reply {
    const missing = { status: 404, body: { error: "file not found" } };
    const file = query.get("path") ?? "";
    if (path === `${EXEC_PATH}/file/metas`) return this.metas.get(file) ?? missing;
    if (path === `${EXEC_PATH}/file/preview`) {
      const maxRows = query.get("maxRows");
      return this.previews.get(file)?.(maxRows === null ? null : Number(maxRows)) ?? missing;
    }
    return this.others.get(path) ?? { status: 404, body: { error: `no route ${path}` } };
  }

  /** The requests made to one route (`metas` or `preview`). */
  callsTo(route: "metas" | "preview"): FileCall[] {
    return this.calls.filter((c) => c.path === `${EXEC_PATH}/file/${route}`);
  }
}
