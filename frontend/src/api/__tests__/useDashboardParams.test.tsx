import { describe, it, expect } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { MemoryRouter, useLocation } from "react-router-dom";
import type { ReactNode } from "react";
import { useDashboardParams } from "../useDashboardParams";
import type { DashboardParameter } from "@/api/dashboards";

const params: DashboardParameter[] = [
  { id: "p1", name: "Region", slug: "region", type: "text", operator: "is", default: "EU" },
  { id: "p2", name: "Tags", slug: "tags", type: "category", operator: "is" },
  { id: "p3", name: "City", slug: "city", type: "location", operator: "is" },
];
const wrap = (initial: string) => ({ children }: { children: ReactNode }) =>
  <MemoryRouter initialEntries={[initial]}>{children}</MemoryRouter>;

describe("useDashboardParams", () => {
  it("seeds from default when the URL has no value", () => {
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap("/d") });
    expect(result.current.values.region).toBe("EU");
  });
  it("reads a scalar value from the URL", () => {
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap("/d?region=US") });
    expect(result.current.values.region).toBe("US");
  });
  it("reads a JSON array value (category) from the URL", () => {
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap(`/d?tags=${encodeURIComponent('["a","b"]')}`) });
    expect(result.current.values.tags).toEqual(["a", "b"]);
  });
  it("setValue writes to the URL", () => {
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap("/d") });
    act(() => result.current.setValue("region", "APAC"));
    expect(result.current.values.region).toBe("APAC");
  });

  it("round-trips a multi-value location, including a value containing a comma", () => {
    // A genuine round trip: the write goes into the MemoryRouter's search params and the read
    // comes back out of them. The comma-bearing value is the point — it is the case a
    // comma-joined encoding could not survive even in principle, so this pins the encoding
    // against anyone who reintroduces one.
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap("/d") });
    const picked = ["Hà Nội", "Đà Nẵng, Việt Nam"];
    act(() => result.current.setValue("city", picked));
    expect(result.current.values.city).toEqual(picked);
  });

  it("does not JSON-decode a scalar list value", () => {
    // Reachable today: DashboardView's drill "Filter by" and crossfilter click behaviour both
    // call setValue with a SCALAR. Decoding it turned "42" into the NUMBER 42, and `in [42]`
    // against a text column matches nothing. Assert the RAW value, not a label.
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap("/d?tags=42") });
    expect(result.current.values.tags).toBe("42");
  });

  it("round-trips a multi-value id without coercing the single-value form", () => {
    const idParams: DashboardParameter[] = [
      { id: "p4", name: "User", slug: "user", type: "id", operator: "is", isMultiSelect: true },
    ];
    const { result } = renderHook(() => useDashboardParams(idParams), { wrapper: wrap("/d?user=42") });
    expect(result.current.values.user).toBe("42");
    act(() => result.current.setValue("user", ["42", "43"]));
    expect(result.current.values.user).toEqual(["42", "43"]);
  });

  it("setValues writes every key in one update", () => {
    // Two setValue calls in one tick would leave only the last: each reads the same location.
    const { result } = renderHook(() => useDashboardParams(params), { wrapper: wrap("/d") });
    act(() => result.current.setValues({ region: "APAC", city: ["Hà Nội"] }));
    expect(result.current.values.region).toBe("APAC");
    expect(result.current.values.city).toEqual(["Hà Nội"]);
  });
});

// ── KI-060: every filter type's value must survive the URL, and a reload of that URL ──────────
//
// Each row is a value exactly as the filter's own control emits it (`FilterControl`,
// `DateFilterPicker`). The round trip is write -> read in the same router, then the search string
// is opened in a FRESH router: that second read is what a reload and a shared link do.
const P = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "x", name: "X", slug: "x", type: "text", operator: "is", ...over });

const ROUND_TRIP: Array<[string, Partial<DashboardParameter>, unknown]> = [
  ["text is", { type: "text", operator: "is" }, "Asia & Europe = 50%"],
  ["text that looks like JSON", { type: "text", operator: "contains" }, "[draft] {x}"],
  ["number =", { type: "number", operator: "=" }, 42],
  ["number = decimal", { type: "number", operator: ">=" }, 12.5],
  ["number between", { type: "number", operator: "between" }, [10, 20]],
  ["number between, max only", { type: "number", operator: "between" }, [null, 20]],
  ["date single", { type: "date", operator: "single" }, "1997-06-01"],
  ["date range", { type: "date", operator: "range" }, ["1995-01-01", "1996-12-31"]],
  ["date range, start only", { type: "date", operator: "range" }, ["1995-01-01", ""]],
  ["date relative", { type: "date", operator: "relative" }, "last30"],
  ["date month-year", { type: "date", operator: "month-year" }, "1997-06"],
  ["date quarter-year", { type: "date", operator: "quarter-year" }, "1997-2"],
  ["date all-options on", { type: "date", operator: "all-options" }, { mode: "specific", op: "on", from: "1997-06-01" }],
  ["date all-options after", { type: "date", operator: "all-options" }, { mode: "specific", op: "after", from: "1997-06-01" }],
  ["date all-options before", { type: "date", operator: "all-options" }, { mode: "specific", op: "before", from: "1997-06-01" }],
  ["date all-options between", { type: "date", operator: "all-options" }, { mode: "specific", op: "between", from: "1995-01-01", to: "1996-12-31" }],
  ["date all-options past", { type: "date", operator: "all-options" }, { mode: "relative", dir: "past", n: 3, unit: "month" }],
  ["date all-options next", { type: "date", operator: "all-options" }, { mode: "relative", dir: "next", n: 1, unit: "year" }],
  ["date all-options current", { type: "date", operator: "all-options" }, { mode: "relative", dir: "current", n: 1, unit: "week" }],
  ["category single", { type: "category", operator: "is", isMultiSelect: false }, "BUILDING"],
  ["category multi", { type: "category", operator: "is" }, ["BUILDING", "AUTO, MOBILE"]],
  ["location multi", { type: "location", operator: "is-not" }, ["Hà Nội", "Đà Nẵng"]],
  ["id single", { type: "id", operator: "is" }, "42"],
  ["id multi", { type: "id", operator: "is", isMultiSelect: true }, ["42", "43"]],
  ["boolean true", { type: "boolean", operator: "is" }, "true"],
  ["boolean false", { type: "boolean", operator: "is" }, "false"],
  ["temporal unit", { type: "temporal_unit", operator: "is" }, "quarter"],
];

const withLocation = (ps: DashboardParameter[]) => () => ({ s: useDashboardParams(ps), search: useLocation().search });

describe("useDashboardParams round trip (KI-060)", () => {
  it.each(ROUND_TRIP)("%s survives write, read and reload", (_name, over, value) => {
    const ps = [P(over)];
    const { result } = renderHook(withLocation(ps), { wrapper: wrap("/d") });
    act(() => result.current.s.setValue("x", value));
    expect(result.current.s.values.x).toEqual(value);
    const reloaded = renderHook(withLocation(ps), { wrapper: wrap(`/d${result.current.search}`) });
    expect(reloaded.result.current.s.values.x).toEqual(value);
  });
});

// ── KI-061: a viewer can clear a filter that has a default, and "Reset all" counts defaults ───
describe("useDashboardParams clearing a default (KI-061)", () => {
  const ps: DashboardParameter[] = [
    { id: "d", name: "Order date", slug: "order_date", type: "date", operator: "range", default: ["1995-01-01", "1996-12-31"] },
    { id: "r", name: "Region", slug: "region", type: "text", operator: "is" },
  ];

  it("clearing a filter with a default keeps it cleared, across a reload", () => {
    const { result } = renderHook(withLocation(ps), { wrapper: wrap("/d") });
    expect(result.current.s.values.order_date).toEqual(["1995-01-01", "1996-12-31"]);
    act(() => result.current.s.setValue("order_date", []));
    expect(result.current.s.values.order_date).toBeUndefined();
    const reloaded = renderHook(withLocation(ps), { wrapper: wrap(`/d${result.current.search}`) });
    expect(reloaded.result.current.s.values.order_date).toBeUndefined();
  });

  it("clearing a filter without a default leaves no trace in the URL", () => {
    const { result } = renderHook(withLocation(ps), { wrapper: wrap("/d?region=EU") });
    act(() => result.current.s.setValue("region", ""));
    expect(result.current.search).toBe("");
  });

  it("a required filter cannot be emptied: clearing returns it to its default", () => {
    const req = [{ ...ps[0], required: true }];
    const { result } = renderHook(withLocation(req), { wrapper: wrap("/d?order_date=%5B%221990-01-01%22%2C%221990-12-31%22%5D") });
    act(() => result.current.s.setValue("order_date", []));
    expect(result.current.s.values.order_date).toEqual(["1995-01-01", "1996-12-31"]);
  });

  it("Reset all is offered only when a filter differs from its default, and restores the defaults", () => {
    const { result } = renderHook(withLocation(ps), { wrapper: wrap("/d") });
    expect(result.current.s.canReset).toBe(false);
    // A URL that names the default value is not a change.
    act(() => result.current.s.setValue("order_date", ["1995-01-01", "1996-12-31"]));
    expect(result.current.s.canReset).toBe(false);
    act(() => result.current.s.setValue("order_date", []));
    expect(result.current.s.canReset).toBe(true);
    act(() => result.current.s.reset());
    expect(result.current.s.values.order_date).toEqual(["1995-01-01", "1996-12-31"]);
    expect(result.current.s.canReset).toBe(false);
    act(() => result.current.s.setValue("region", "EU"));
    expect(result.current.s.canReset).toBe(true);
    act(() => result.current.s.reset());
    expect(result.current.s.values.region).toBeUndefined();
    expect(result.current.search).toBe("");
  });

  it("reset leaves the view controls in the URL alone", () => {
    const { result } = renderHook(withLocation(ps), { wrapper: wrap("/d?refresh=60&fullscreen=true&region=EU&order_date=") });
    act(() => result.current.s.reset());
    expect(result.current.search).toBe("?refresh=60&fullscreen=true");
  });
});
