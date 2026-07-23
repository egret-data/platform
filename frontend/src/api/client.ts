import { queryClient } from "./queryClient";

export class UnauthorizedError extends Error {}
export class ApiError extends Error {
  /** The error body's machine-readable `code` (e.g. `"feature_disabled"`), when
   * the server sent one. Optional so existing `new ApiError(status, message)`
   * call sites keep working unchanged. */
  constructor(public status: number, message: string, public code?: string) { super(message); }
  static async from(res: Response): Promise<ApiError> {
    let msg = res.statusText;
    let code: string | undefined;
    try {
      const j = await res.json();
      if (j?.error) msg = j.error;
      if (j?.code) code = j.code;
    } catch { /* ignore */ }
    return new ApiError(res.status, msg, code);
  }
}

export function readCookie(name: string): string | null {
  const m = document.cookie.match(new RegExp("(?:^|; )" + name + "=([^;]*)"));
  return m ? decodeURIComponent(m[1]) : null;
}

const MUTATING = new Set(["POST", "PUT", "PATCH", "DELETE"]);

export async function apiFetch(path: string, init: RequestInit = {}): Promise<Response> {
  const method = (init.method ?? "GET").toUpperCase();
  const headers = new Headers(init.headers);
  if (MUTATING.has(method)) {
    const csrf = readCookie("egret_csrf");
    if (csrf) headers.set("X-Egret-CSRF", csrf);
  }
  const res = await fetch(`/api${path}`, { ...init, headers, credentials: "include" });
  if (res.status === 401) throw new UnauthorizedError();
  if (!res.ok) {
    const err = await ApiError.from(res);
    // Race: an admin can switch a module off while a user still has it open.
    // The Sidebar/`RequireFeature` gate is an optimistic read of
    // `["my-workspaces"]` (see useWorkspaceFeatures); this 403/code pair is
    // the backend's real, un-bypassable enforcement firing. Invalidating that
    // one query here — the single source both read from — makes the route
    // guard notice the module is gone and swap to Forbidden, rather than
    // leaving the page to show whatever generic error its own failed query
    // produces.
    if (err.status === 403 && err.code === "feature_disabled") {
      void queryClient.invalidateQueries({ queryKey: ["my-workspaces"] });
    }
    throw err;
  }
  return res;
}

export async function apiJson<T>(path: string, init?: RequestInit): Promise<T> {
  return (await apiFetch(path, init)).json() as Promise<T>;
}
