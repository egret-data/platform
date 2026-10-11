import { describe, expect, it } from "vitest";
import {
  DEFAULT_FILTERS,
  isFiltering,
  PAGE_SIZE,
  pageCount,
  readFilters,
  toQueryParams,
  withFilters,
  type ExecutionFilters,
} from "@/routes/workflows/executionsListModel";

const NOW = Date.parse("2026-10-11T12:00:00.000Z");

/** Writes `f` onto an empty query string and reads it back. */
function roundTrip(patch: Partial<ExecutionFilters>): { url: string; back: ExecutionFilters } {
  const params = withFilters(new URLSearchParams(), patch);
  return { url: params.toString(), back: readFilters(new URLSearchParams(params.toString())) };
}

describe("readFilters / withFilters", () => {
  it("reads an empty query string as the defaults: every state, all time, root executions only, page 1", () => {
    expect(readFilters(new URLSearchParams())).toEqual(DEFAULT_FILTERS);
    expect(DEFAULT_FILTERS).toEqual({
      states: [],
      namespace: "",
      subNamespaces: false,
      flow: "",
      time: "",
      since: "",
      until: "",
      showChildren: false,
      page: 1,
    });
  });

  it("round-trips several states as one comma list, in the order chosen", () => {
    const { url, back } = roundTrip({ states: ["RUNNING", "FAILED"] });
    expect(url).toBe("state=RUNNING%2CFAILED");
    expect(back.states).toEqual(["RUNNING", "FAILED"]);
  });

  it("drops empty and repeated states read from a hand-edited URL", () => {
    expect(readFilters(new URLSearchParams("state=FAILED,,FAILED,SUCCESS")).states).toEqual(["FAILED", "SUCCESS"]);
  });

  it("round-trips a namespace with and without its child namespaces", () => {
    expect(roundTrip({ namespace: "company.team" }).back).toMatchObject({ namespace: "company.team", subNamespaces: false });
    const withChildren = roundTrip({ namespace: "company", subNamespaces: true });
    expect(withChildren.url).toBe("namespace=company&subns=1");
    expect(withChildren.back).toMatchObject({ namespace: "company", subNamespaces: true });
  });

  it("does not keep 'include child namespaces' without a namespace", () => {
    expect(roundTrip({ subNamespaces: true }).url).toBe("");
    expect(readFilters(new URLSearchParams("subns=1")).subNamespaces).toBe(false);
  });

  it("round-trips the flow", () => {
    expect(roundTrip({ flow: "0a7c1d2e-0000-4000-8000-000000000001" }).back.flow).toBe("0a7c1d2e-0000-4000-8000-000000000001");
  });

  it("round-trips each time preset, and drops since/until that belong to no custom window", () => {
    for (const time of ["1h", "24h", "7d", "30d"] as const) {
      const { url, back } = roundTrip({ time, since: "2026-10-01T00:00:00.000Z" });
      expect(url).toBe(`time=${time}`);
      expect(back.time).toBe(time);
      expect(back.since).toBe("");
    }
  });

  it("round-trips a custom window", () => {
    const { back } = roundTrip({ time: "custom", since: "2026-10-01T08:00:00.000Z", until: "2026-10-02T08:00:00.000Z" });
    expect(back).toMatchObject({ time: "custom", since: "2026-10-01T08:00:00.000Z", until: "2026-10-02T08:00:00.000Z" });
  });

  it("reads an unknown time value as all time", () => {
    expect(readFilters(new URLSearchParams("time=2y")).time).toBe("");
  });

  it("round-trips the child switch", () => {
    expect(roundTrip({ showChildren: true })).toMatchObject({ url: "child=1", back: { showChildren: true } });
    expect(roundTrip({ showChildren: false }).url).toBe("");
  });

  it("round-trips the page, and reads a bad page as 1", () => {
    expect(roundTrip({ page: 3 })).toMatchObject({ url: "page=3", back: { page: 3 } });
    expect(roundTrip({ page: 1 }).url).toBe("");
    for (const bad of ["0", "-2", "x", "1.5"]) expect(readFilters(new URLSearchParams(`page=${bad}`)).page).toBe(1);
  });

  it("sends the list back to page 1 when a filter changes, unless the patch names a page", () => {
    const onPage3 = new URLSearchParams("page=3&state=FAILED");
    expect(readFilters(withFilters(onPage3, { states: ["SUCCESS"] })).page).toBe(1);
    expect(readFilters(withFilters(onPage3, { page: 4 }))).toMatchObject({ page: 4, states: ["FAILED"] });
  });

  it("leaves parameters it does not own alone", () => {
    expect(withFilters(new URLSearchParams("tab=x"), { flow: "f" }).get("tab")).toBe("x");
  });
});

describe("isFiltering", () => {
  it("is false for the defaults and for the child switch alone: neither narrows what 'no executions yet' means", () => {
    expect(isFiltering(DEFAULT_FILTERS)).toBe(false);
    expect(isFiltering({ ...DEFAULT_FILTERS, showChildren: true })).toBe(false);
  });

  it("is true for each filter", () => {
    for (const patch of [{ states: ["FAILED"] }, { namespace: "a" }, { flow: "f" }, { time: "1h" as const }, { time: "custom" as const }]) {
      expect(isFiltering({ ...DEFAULT_FILTERS, ...patch })).toBe(true);
    }
  });
});

describe("readFilters on the flow's own tab", () => {
  it("reads a flow, namespace or child-namespace switch left in the URL as absent, and nothing else", () => {
    const params = new URLSearchParams("flow=other&namespace=x&subns=1&state=FAILED&child=1&page=2");
    expect(readFilters(params, { pinned: true })).toEqual({
      ...DEFAULT_FILTERS,
      states: ["FAILED"],
      showChildren: true,
      page: 2,
    });
    expect(isFiltering(readFilters(new URLSearchParams("flow=other&namespace=x"), { pinned: true }))).toBe(false);
  });
});

describe("toQueryParams", () => {
  it("sends root_only=true while the child switch is off, and no root_only once it is on", () => {
    expect(toQueryParams(DEFAULT_FILTERS, NOW)).toEqual({ root_only: true, limit: PAGE_SIZE, offset: 0 });
    expect(toQueryParams({ ...DEFAULT_FILTERS, showChildren: true }, NOW)).not.toHaveProperty("root_only");
  });

  it("sends the states as one comma list, the namespace, the flow and the page's offset", () => {
    expect(toQueryParams({ ...DEFAULT_FILTERS, states: ["RUNNING", "FAILED"], namespace: "company", flow: "f-1", page: 3 }, NOW)).toEqual({
      state: "RUNNING,FAILED",
      namespace: "company",
      flow_id: "f-1",
      root_only: true,
      limit: PAGE_SIZE,
      offset: 2 * PAGE_SIZE,
    });
  });

  it("sends the namespace as namespace_prefix, and not as namespace, when child namespaces are included", () => {
    const q = toQueryParams({ ...DEFAULT_FILTERS, namespace: "company", subNamespaces: true }, NOW);
    expect(q.namespace_prefix).toBe("company");
    expect(q).not.toHaveProperty("namespace");
  });

  it("computes since from each preset against the clock it is given, and sends no until", () => {
    const expected = { "1h": "2026-10-11T11:00:00.000Z", "24h": "2026-10-10T12:00:00.000Z", "7d": "2026-10-04T12:00:00.000Z", "30d": "2026-09-11T12:00:00.000Z" };
    for (const [time, since] of Object.entries(expected)) {
      const req = toQueryParams({ ...DEFAULT_FILTERS, time: time as ExecutionFilters["time"] }, NOW);
      expect(req.since).toBe(since);
      expect(req).not.toHaveProperty("until");
    }
    // Another clock, another window: the value is computed, not stored.
    expect(toQueryParams({ ...DEFAULT_FILTERS, time: "1h" }, NOW + 60_000).since).toBe("2026-10-11T11:01:00.000Z");
  });

  it("sends a custom window's since and until as given, and either alone", () => {
    const both = { ...DEFAULT_FILTERS, time: "custom" as const, since: "2026-10-01T00:00:00.000Z", until: "2026-10-02T00:00:00.000Z" };
    expect(toQueryParams(both, NOW)).toMatchObject({ since: both.since, until: both.until });
    const sinceOnly = toQueryParams({ ...both, until: "" }, NOW);
    expect(sinceOnly.since).toBe(both.since);
    expect(sinceOnly).not.toHaveProperty("until");
  });

  it("pins the flow on the flow's own tab, whatever the filters say, and sends no namespace", () => {
    const req = toQueryParams({ ...DEFAULT_FILTERS, flow: "other", namespace: "x", subNamespaces: true }, NOW, { pinnedFlowId: "mine" });
    expect(req.flow_id).toBe("mine");
    expect(req).not.toHaveProperty("namespace");
    expect(req).not.toHaveProperty("namespace_prefix");
  });
});

describe("pageCount", () => {
  it("is ceil(total / page size), at least 1, at any depth", () => {
    expect(pageCount(0)).toBe(1);
    expect(pageCount(50)).toBe(1);
    expect(pageCount(51)).toBe(2);
    expect(pageCount(3000)).toBe(60);
  });
});
