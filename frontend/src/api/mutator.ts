import { apiFetch } from "./client";

/**
 * Orval custom mutator (fetch-style).
 *
 * Orval's `react-query` client invokes this as `customInstance<T>(url, init)`
 * and expects back a `{ status, data, headers }` envelope (T). We keep the
 * generated client behaviourally identical to the hand-written modules by
 * delegating the actual request to {@link apiFetch} in `client.ts`, which owns:
 *   - `credentials: "include"` (auth cookies),
 *   - the `X-Egret-CSRF` header on mutating methods,
 *   - `UnauthorizedError` on 401 and `ApiError` on other non-2xx responses.
 *
 * Because `apiFetch` throws on non-2xx, this mutator only ever returns the
 * success envelope; react-query surfaces the thrown `ApiError`/`UnauthorizedError`
 * as the query/mutation `error` — same as today. (The generated `TError` is
 * typed `ErrorBody`, but the runtime value is an `ApiError` carrying `.status`,
 * which call sites already rely on.)
 *
 * Path reconcile: the spec's paths carry the `/api` prefix (e.g.
 * `/api/ws/{workspace}/databases`), whereas `apiFetch` prepends `/api` itself.
 * We strip the leading `/api` segment here so the final request hits the same
 * absolute path the hand-written modules use — no double `/api/api`. Query
 * strings are already baked into `url` by the generated URL builders.
 *
 * CONSTRAINT — only `/api`-scoped operations are adoptable through this mutator.
 * `apiFetch` structurally hardcodes an `/api` prefix, so the 9 non-`/api` spec
 * paths (oidc `/.well-known/*`, `/oauth/token`, ranger `/service/*`) would be
 * mis-routed to `/api/oauth/token` etc. Those are machine/protocol endpoints
 * with no browser FE module, so do NOT wire oidc/idp/ranger through this mutator.
 */
export interface FetchResponse<T> {
  status: number;
  data: T;
  headers: Headers;
}

/**
 * The mutator's own request options, beside `RequestInit`. Every generated
 * fetcher takes `Parameters<typeof customInstance>[1]`, so a caller passes
 * them to the generated function itself.
 */
export interface CustomRequestOptions extends RequestInit {
  /**
   * Answer the success body as its raw text in `data`, never parsed, whatever
   * its type. For a caller that must read the JSON itself: `JSON.parse` rounds
   * a number past 2^53, and the KV routes send numbers with every digit
   * (`components/workflows/namespaces/kv/kvApi.ts`, which reads them with
   * `parseLossless`). The generated response type still names the parsed
   * body; read `data` through `rawText()`. Errors are thrown as always.
   */
  rawText?: boolean;
  /**
   * Answer the success body as a `Blob` in `data`, every byte as sent, never
   * decoded. For a caller that reads bytes the text decoding would change: a
   * namespace file may be an image or any other binary, and the editor must
   * write back exactly the bytes it read
   * (`components/workflows/namespaces/files/nsFilesApi.ts`). Read `data`
   * through `rawBlob()`. Errors are thrown as always.
   */
  rawBlob?: boolean;
}

/**
 * The raw body of a response requested with `rawText: true`. The generated
 * type says the parsed body; at runtime `data` is the text.
 */
export function rawText(response: { data: unknown }): string {
  if (typeof response.data !== "string") throw new Error("the response was not read as text (pass rawText: true)");
  return response.data;
}

/**
 * The body of a response requested with `rawBlob: true`. Checked by shape,
 * not `instanceof`: in tests the `Blob` comes from another realm (Node's
 * `fetch` under jsdom).
 */
export function rawBlob(response: { data: unknown }): Blob {
  const data = response.data as Blob | undefined;
  if (!data || typeof data !== "object" || typeof data.arrayBuffer !== "function") {
    throw new Error("the response was not read as bytes (pass rawBlob: true)");
  }
  return data;
}

export const customInstance = async <T>(
  url: string,
  requestOptions?: CustomRequestOptions,
): Promise<T> => {
  const { rawText: asText = false, rawBlob: asBlob = false, ...options } = requestOptions ?? {};
  const method = (options.method ?? "GET").toUpperCase();
  // Reconcile the spec's `/api`-prefixed paths with apiFetch's own `/api` prefix.
  // Segment-anchored so a hypothetical `/apiXxx` path is never mangled.
  const path = url.replace(/^\/api(?=\/|$)/, "");

  // orval writes no `Content-Type` for a route that takes two body types (JSON or
  // multipart), and a browser's `fetch` labels a string body `text/plain`, which
  // such a route refuses with 415 (W7a Task 8: every Run of a flow failed so).
  // Every generated body that is a string is `JSON.stringify`'d, so a string
  // with no type is JSON; FormData, URLSearchParams and Blob carry their own.
  const headers = new Headers(options.headers);
  if (typeof options.body === "string" && !headers.has("Content-Type")) {
    headers.set("Content-Type", "application/json");
  }

  const res = await apiFetch(path, { ...options, method, headers });

  let data: unknown = undefined;
  if (asText) {
    data = await res.text();
  } else if (asBlob) {
    data = await res.blob();
  } else if (res.status !== 204 && res.headers.get("content-length") !== "0") {
    const contentType = res.headers.get("content-type") ?? "";
    data = contentType.includes("application/json")
      ? await res.json()
      : await res.text();
  }

  return { status: res.status, data, headers: res.headers } as T;
};

export default customInstance;
