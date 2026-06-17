export class UnauthorizedError extends Error {}
export class ApiError extends Error {
  constructor(public status: number, message: string) { super(message); }
  static async from(res: Response): Promise<ApiError> {
    let msg = res.statusText;
    try { const j = await res.json(); if (j?.error) msg = j.error; } catch { /* ignore */ }
    return new ApiError(res.status, msg);
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
  if (!res.ok) throw await ApiError.from(res);
  return res;
}

export async function apiJson<T>(path: string, init?: RequestInit): Promise<T> {
  return (await apiFetch(path, init)).json() as Promise<T>;
}
