import { queryClient } from "./queryClient";

export class UnauthorizedError extends Error {}
export class ApiError extends Error {
  /**
   * @param status HTTP status.
   * @param message Human-readable message (from the body's `error` field when present).
   * @param code The error body's machine-readable `code` (e.g. `"feature_disabled"`,
   *   `"oidc_step_up"`), when the server sent one. Optional so existing
   *   `new ApiError(status, message)` call sites keep working unchanged.
   * @param body The full parsed JSON error body, when the response was JSON. Most
   *   call sites only need `code`/`message`, but some error bodies carry extra
   *   fields beyond those two (e.g. `OidcStepUp`'s `authorize_url`) that a caller
   *   may need to read — `code` alone can't surface them.
   */
  constructor(public status: number, message: string, public code?: string, public body?: unknown) { super(message); }
  static async from(res: Response): Promise<ApiError> {
    let msg = res.statusText;
    let code: string | undefined;
    let body: unknown;
    try {
      const j = await res.json();
      body = j;
      if (j?.error) msg = j.error;
      if (j?.code) code = j.code;
    } catch { /* ignore */ }
    return new ApiError(res.status, msg, code, body);
  }
}

export function readCookie(name: string): string | null {
  const m = document.cookie.match(new RegExp("(?:^|; )" + name + "=([^;]*)"));
  return m ? decodeURIComponent(m[1]) : null;
}

/**
 * The CSRF token to echo in `X-Egret-CSRF`. A deployment with Secure cookies issues
 * `__Host-egret_csrf` (the prefix stops a sibling host from tossing a cookie of that
 * name); plain-http dev issues `egret_csrf`. The `__Host-` one wins when both exist, so
 * a tossed plain-name cookie is never preferred — the server ignores it in a secure
 * deployment anyway.
 */
export function csrfToken(): string | null {
  return readCookie("__Host-egret_csrf") ?? readCookie("egret_csrf");
}

const MUTATING = new Set(["POST", "PUT", "PATCH", "DELETE"]);

export async function apiFetch(path: string, init: RequestInit = {}): Promise<Response> {
  const method = (init.method ?? "GET").toUpperCase();
  const headers = new Headers(init.headers);
  if (MUTATING.has(method)) {
    const csrf = csrfToken();
    if (csrf) headers.set("X-Egret-CSRF", csrf);
  }
  const res = await fetch(`/api${path}`, { ...init, headers, credentials: "include" });
  if (res.status === 401) throw new UnauthorizedError();
  if (!res.ok) {
    const err = await ApiError.from(res);
    // Race: an admin can switch a module off while a user still has it open.
    // The Sidebar/`RequireFeature` gate is an optimistic read of
    // `["my-workspaces"]` (see useWorkspaceFeatures); this 403/code pair is
    // the backend's real, un-bypassable enforcement firing. Refetching that
    // one query here — the single source both read from — makes the route
    // guard notice the module is gone and swap to Forbidden, rather than
    // leaving the page to show whatever generic error its own failed query
    // produces.
    //
    // `client.ts` (transport) reaching into the query cache like this is a
    // layering inversion in principle. It's deliberate here, not incidental:
    // this is the ONE choke point every request (hand-written hooks and the
    // generated client, via mutator.ts) already passes through, so it is the
    // only place that can react to this specific error for every call site
    // without adding a query-layer error handler that each of those ~40
    // call sites would need to opt into individually. `queryClient` is
    // exported as a singleton from `./queryClient` for exactly this reason.
    //
    // "~40" is now measured (SP4d debt Task 11 survey, 2026-10-10): the
    // hand-written layer is 61 mutation hooks across 17 `src/api/*.ts` modules,
    // carrying 99 `useMutation` calls, reached from 152 `.mutate()`/
    // `.mutateAsync()` call sites outside `src/api/**`. 21 of those 152 discard
    // the mutation result entirely — no `onError`, no `.isError` read, no
    // `.catch`, no `guard()`/`run()` wrapper — so their failure is invisible to
    // the user, which is the same defect Task 3 fixed in `CatalogTagEditor`'s
    // assign half and Task 11 fixed in its unassign half. 9 of the 21 are
    // reachable through a documented permission refusal the UI does not gate on
    // (the `TagEditor` pair, `Classifications` delete-tag, three SQL-Lab saved-
    // query/snippet sites, the notification remove); the rest need a 5xx or a
    // dropped connection. Fixing them is a separate package — the 10
    // `NotebookPage` sites are one cluster with one remedy, since that page
    // already reads `addCell.isPending`. Note the count is about
    // *visibility at the call site*, which is a wider question than this
    // comment's: a site that opts into an error handler still does not get the
    // `feature_disabled` refetch below without this choke point.
    //
    // We `await` (rather than fire-and-forget) and force `refetchType: "all"`
    // so the refetch isn't deferred as inactive — this doesn't close the gap
    // (the caller's own catch/toast can still run before RequireFeature's
    // next render), but it shrinks it to this one round-trip instead of an
    // arbitrary background refetch. Closing it fully would mean either
    // touching every page's own `onError`/`.catch` (out of proportion for
    // this race) or reaching into react-query's Mutation internals to
    // silence a per-call onError after the fact (fragile, and only covers
    // mutations wired through the `onError` option — not the many call
    // sites that `.catch()` a `mutateAsync()` directly) — so a brief flash
    // of the page's own generic error remains possible.
    if (err.status === 403 && err.code === "feature_disabled") {
      await queryClient.invalidateQueries({ queryKey: ["my-workspaces"], refetchType: "all" });
    }
    throw err;
  }
  return res;
}

export async function apiJson<T>(path: string, init?: RequestInit): Promise<T> {
  return (await apiFetch(path, init)).json() as Promise<T>;
}
