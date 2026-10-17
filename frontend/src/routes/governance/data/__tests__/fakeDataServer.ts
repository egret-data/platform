import { vi } from "vitest";
import type { BrowseResponse, GovernanceCatalog } from "@/api/generated/model";

/**
 * A fake of the two endpoints the Data tree reads: `GET …/governance/catalogs`
 * and `GET …/governance/browse/{catalog_id}?schema=&table=`. Answers go
 * through the real generated client and `apiFetch`, so a test sees exactly
 * what the page sees, including a 403 as an `ApiError`.
 */

export function catalog(id: string, name: string, connectorType: string, egret = false): GovernanceCatalog {
  return { id, name, source: egret ? "egret" : "engine", connectorType, engines: [] };
}

export const browseItems = (items: BrowseResponse["items"], source: BrowseResponse["source"] = { kind: "egret" }): BrowseResponse => ({
  source,
  engines: [],
  items,
});

export const browseNotListed = (reason: NonNullable<BrowseResponse["notListed"]>["reason"], detail: string): BrowseResponse => ({
  source: null,
  engines: [],
  notListed: { reason, detail },
});

type Answer = { status: number; body: unknown } | "never";

export interface FakeData {
  catalogs: Answer;
  /** Keyed `catalogId` or `catalogId|schema`. A key with no answer is a 404. */
  browse: Record<string, Answer>;
}

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

/** Stubs `fetch`; returns the browse calls made, as `catalogId` or `catalogId|schema`. */
export function serveData(data: FakeData): { browseCalls: string[] } {
  const browseCalls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const url = new URL(String(input), "http://localhost");
      const reply = (a: Answer | undefined) =>
        a === "never"
          ? new Promise<Response>(() => {})
          : Promise.resolve(a ? json(a.status, a.body) : json(404, { error: "not found" }));
      if (url.pathname === "/api/control-plane/governance/catalogs") return reply(data.catalogs);
      const m = /^\/api\/control-plane\/governance\/browse\/([^/]+)$/.exec(url.pathname);
      if (m) {
        const id = decodeURIComponent(m[1]);
        const schema = url.searchParams.get("schema");
        const key = schema === null ? id : `${id}|${schema}`;
        browseCalls.push(key);
        return reply(data.browse[key]);
      }
      return Promise.reject(new Error(`no fake for ${url.pathname}`));
    }),
  );
  return { browseCalls };
}
