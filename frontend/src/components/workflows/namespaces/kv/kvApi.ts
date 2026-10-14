import { ApiError, apiFetch } from "@/api/client";
import { getWorkflowGetKvValueUrl, workflowPutKvValue } from "@/api/generated/workflow/workflow";
import { isKvType, type KvType } from "./kvValue";
import { JsonNumber, parseLossless, type LosslessJson } from "./losslessJson";
import { headerText } from "./kvValue";

/** One key's latest value, its numbers as the server wrote them. */
export interface KvValueDetail {
  type: KvType;
  value: LosslessJson;
  revision: number;
  updated: string;
  description: string | null;
  expiresAt: string | null;
}

/**
 * `GET …/kv/{key}` read losslessly.
 *
 * FE-DATA-2 exception: the generated fetcher parses the body with
 * `JSON.parse`, which rounds a number past 2^53, and W9c sends a NUMBER with
 * every digit; the generated URL is fetched through `apiFetch` and read with
 * `parseLossless` instead.
 *
 * A key named `inheritance` is read at `…/kv/inheritanc%65`: the path as
 * written is the inheritance listing (W9c spec §9), and the server decodes the
 * key after matching the route.
 */
export async function readKvValue(ws: string, namespace: string, key: string): Promise<KvValueDetail> {
  let url = getWorkflowGetKvValueUrl(ws, namespace, key);
  if (key === "inheritance") url = url.replace(/\/kv\/inheritance$/, "/kv/inheritanc%65");
  const res = await apiFetch(url.replace(/^\/api(?=\/)/, ""));
  const body = parseLossless(await res.text());
  if (body === null || typeof body !== "object" || Array.isArray(body) || body instanceof JsonNumber) {
    throw new Error("the server's answer was not a KV value");
  }
  const type = String(body.type);
  if (!isKvType(type)) throw new Error(`the server answered an unknown type, ${type}`);
  const text = (v: LosslessJson | undefined) => (typeof v === "string" ? v : null);
  return {
    type,
    value: body.value ?? null,
    revision: Number(String(body.revision)),
    updated: text(body.updated) ?? "",
    description: text(body.description),
    expiresAt: text(body.expiresAt),
  };
}

/**
 * `PUT …/kv/{key}`: `body` as `text/plain`, and the `description` and `ttl`
 * headers, each left out when there is none. The generated fetcher, called
 * directly (FE-DATA-2 exception): the headers differ per request, and the
 * generated mutation fixes its request options when the hook is created.
 */
export async function putKvValue(
  ws: string,
  namespace: string,
  key: string,
  body: string,
  meta: { description: string; ttl: string | null },
): Promise<void> {
  const headers: Record<string, string> = {};
  if (meta.description) headers.description = headerText(meta.description);
  if (meta.ttl) headers.ttl = meta.ttl;
  await workflowPutKvValue(ws, namespace, key, body, { headers });
}

/**
 * A KV request's failure as one sentence that says what happened and what to
 * do (FE-COPY-2), with the server's own words when it sent any.
 * `what` names the attempt: "The key could not be saved".
 */
export function kvErrorMessage(e: unknown, what: string): string {
  const server = e instanceof Error && e.message ? e.message.replace(/\.$/, "") : "";
  if (e instanceof ApiError) {
    switch (e.status) {
      case 403:
        return `${what}: ${server || "permission denied"}. Changing KV keys needs the workflow:manage permission; ask a workspace admin for it.`;
      case 400:
        return `${what}: ${server}. Correct it and try again.`;
      case 404:
        return `${what}: ${server}. It may have been deleted or have expired; refresh the list.`;
      case 409:
        return `${what}: ${server}. Another change to this key is in progress; try again in a moment.`;
      case 413:
        return `${what}: ${server}. Make it smaller and try again.`;
      case 503:
        return `${what}: ${server}. The store is unavailable; try again in a moment.`;
      default:
        return `${what}: ${server || `the server answered ${e.status}`}. Try again.`;
    }
  }
  if (e instanceof TypeError) return `${what}: the server could not be reached. Check your connection and try again.`;
  return `${what}: ${server || "something went wrong"}. Try again.`;
}
