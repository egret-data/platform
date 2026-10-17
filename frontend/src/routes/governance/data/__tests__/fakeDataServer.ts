import { vi } from "vitest";
import type { BrowseResponse, GovernanceCatalog } from "@/api/generated/model";

/**
 * A fake of the endpoints the Data area reads: `GET …/governance/catalogs`,
 * `GET …/governance/browse/{catalog_id}?schema=&table=`, and an object's
 * `…/governance/tags/{catalog_id}` and `…/governance/lineage/{catalog_id}`
 * (both `?schema=&table=`), and the perm routes the Permissions tab and
 * `GrantDialog` call (`…/perm/grants`, `…/perm/grants/{id}`, `…/perm/preview`), and the rule
 * routes the Masks & filters tab and `RuleDialog` call (`…/perm/rules`, `…/perm/rules/{id}`), explain
 * (`…/perm/explain`) and the principal search its user field calls (`…/principals`), the tag
 * catalogue (`…/tags`) and the tag write (`PUT …/warehouses/{id}/resource-tags`). Answers go
 * through the real generated client and `apiFetch`, so a test sees exactly
 * what the page sees, including a 403 as an `ApiError`.
 */

export function catalog(id: string, name: string, connectorType: string, egret = false): GovernanceCatalog {
  return {
    id,
    name,
    source: egret ? "egret" : "engine",
    connectorType,
    engines: [],
    warehouseId: egret ? `wh-${id}` : null,
    warehouseName: egret ? `${name}_wh` : null,
    workspaces: [],
  };
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
  /**
   * Keyed `catalogId`, `catalogId|schema` or `catalogId|schema|table`. A key with no answer is a 404.
   * A list is answered in order, its last answer repeating.
   */
  browse: Record<string, Answer | Answer[]>;
  /** Keyed `catalogId|schema|table`. A key with no answer is a 404. */
  tags?: Record<string, Answer>;
  /** Keyed `catalogId|schema|table`. A key with no answer is a 404. */
  lineage?: Record<string, Answer>;
  /** `GET …/perm/grants`. A list is answered in order, its last answer repeating; none is a 404. */
  grants?: Answer | Answer[];
  /** `POST …/perm/preview`; none is a 404. */
  preview?: Answer;
  /** Every grant write (`POST …/perm/grants`, `PATCH`/`DELETE …/perm/grants/{id}`); default 200 `{}`. */
  grantWrite?: Answer;
  /** `GET …/perm/rules`. A list is answered in order, its last answer repeating; none is a 404. */
  rules?: Answer | Answer[];
  /** Every rule write (`POST …/perm/rules`, `PATCH`/`DELETE …/perm/rules/{id}`); default 200 `{}`. */
  ruleWrite?: Answer;
  /** `POST …/perm/explain`; none is a 404. A function answers each call when the promise it returns settles. */
  explain?: Answer | (() => Promise<Answer>);
  /** `POST …/perm/explain/principal`; none is a 404. */
  explainPrincipal?: Answer;
  /** `GET …/perm/people` (a group or role subject's search); default 200 with no items. */
  people?: Answer;
  /** `GET …/principals`; default 200 `[]`. */
  principals?: Answer;
  /** `GET …/tags`, the tag catalogue; none is a 404. */
  tagCatalogue?: Answer;
  /** `PUT …/warehouses/{id}/resource-tags`; default 200 `{}`. */
  tagWrite?: Answer;
}

/** What the fake was asked, as keys of {@link FakeData}. */
export interface DataCalls {
  browseCalls: string[];
  tagCalls: string[];
  lineageCalls: string[];
  /** `METHOD /perm/...`, with each body that was sent. */
  permCalls: { call: string; body?: unknown }[];
  /** Each tag write: the warehouse id from the path and the body. */
  tagWrites: { warehouseId: string; body: unknown }[];
}

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

/** Stubs `fetch`; returns the calls made, keyed as {@link FakeData} keys them. */
export function serveData(data: FakeData): DataCalls {
  const browseCalls: string[] = [];
  const tagCalls: string[] = [];
  const lineageCalls: string[] = [];
  const permCalls: DataCalls["permCalls"] = [];
  const tagWrites: DataCalls["tagWrites"] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input), "http://localhost");
      const reply = (a: Answer | undefined) =>
        a === "never"
          ? new Promise<Response>(() => {})
          : Promise.resolve(a ? json(a.status, a.body) : json(404, { error: "not found" }));
      if (url.pathname === "/api/control-plane/governance/catalogs") return reply(data.catalogs);
      if (url.pathname === "/api/control-plane/principals") return reply(data.principals ?? { status: 200, body: [] });
      if (url.pathname === "/api/control-plane/tags") return reply(data.tagCatalogue);
      const w = /^\/api\/control-plane\/warehouses\/([^/]+)\/resource-tags$/.exec(url.pathname);
      if (w && (init?.method ?? "GET").toUpperCase() === "PUT") {
        tagWrites.push({ warehouseId: decodeURIComponent(w[1]), body: JSON.parse(String(init?.body)) });
        return reply(data.tagWrite ?? { status: 200, body: {} });
      }
      const m = /^\/api\/control-plane\/governance\/browse\/([^/]+)$/.exec(url.pathname);
      if (m) {
        const id = decodeURIComponent(m[1]);
        const schema = url.searchParams.get("schema");
        const table = url.searchParams.get("table");
        const key = [id, schema, table].filter((k) => k !== null).join("|");
        const asked = browseCalls.filter((k) => k === key).length;
        browseCalls.push(key);
        const a = data.browse[key];
        return reply(Array.isArray(a) ? a[Math.min(asked, a.length - 1)] : a);
      }
      const o = /^\/api\/control-plane\/governance\/(tags|lineage)\/([^/]+)$/.exec(url.pathname);
      if (o) {
        const key = `${decodeURIComponent(o[2])}|${url.searchParams.get("schema")}|${url.searchParams.get("table")}`;
        if (o[1] === "tags") {
          tagCalls.push(key);
          return reply(data.tags?.[key]);
        }
        lineageCalls.push(key);
        return reply(data.lineage?.[key]);
      }
      const perm = /^\/api\/control-plane(\/perm\/.+)$/.exec(url.pathname);
      if (perm) {
        const method = (init?.method ?? "GET").toUpperCase();
        const call = `${method} ${perm[1]}`;
        permCalls.push({ call, body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined });
        if (call === "GET /perm/people") return reply(data.people ?? { status: 200, body: { items: [], total: 0 } });
        if (call === "POST /perm/explain/principal") return reply(data.explainPrincipal);
        if (call === "GET /perm/grants") {
          const asked = permCalls.filter((c) => c.call === call).length - 1;
          const a = data.grants;
          return reply(Array.isArray(a) ? a[Math.min(asked, a.length - 1)] : a);
        }
        if (call === "GET /perm/rules") {
          const asked = permCalls.filter((c) => c.call === call).length - 1;
          const a = data.rules;
          return reply(Array.isArray(a) ? a[Math.min(asked, a.length - 1)] : a);
        }
        if (call === "POST /perm/preview") return reply(data.preview);
        if (call === "POST /perm/explain") {
          const e = data.explain;
          return typeof e === "function" ? e().then(reply) : reply(e);
        }
        if (/^(POST \/perm\/rules|(PATCH|DELETE) \/perm\/rules\/[^/]+)$/.test(call)) {
          return reply(data.ruleWrite ?? { status: 200, body: {} });
        }
        if (/^(POST \/perm\/grants|(PATCH|DELETE) \/perm\/grants\/[^/]+)$/.test(call)) {
          return reply(data.grantWrite ?? { status: 200, body: {} });
        }
      }
      return Promise.reject(new Error(`no fake for ${url.pathname}`));
    }),
  );
  return { browseCalls, tagCalls, lineageCalls, permCalls, tagWrites };
}
