/**
 * CL16: every tag write refreshes the one list key (`["tags"]`) AND the
 * classification's usage (route C), so neither the index counts nor Where
 * used nor the delete dialog stays stale until a reload (#4a defect D4).
 * Real hooks over a stubbed `fetch`; the assertion is on the keys handed to
 * `invalidateQueries`.
 */
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  getControlPlaneGovernanceObjectHandwrittenQueryKey,
  getControlPlaneGovernanceObjectTagsQueryKey,
  getControlPlaneGovernanceTagHandwrittenQueryKey,
  getControlPlaneGovernanceTagUsageQueryKey,
} from "../generated/control-plane-governance/control-plane-governance";
import { useAssignResourceTag, useUnassignResourceTag } from "../catalogTags";
import { getControlPlaneTagsListTagsQueryKey } from "../generated/control-plane-tags/control-plane-tags";
import {
  objectTagsQueryKey,
  tagsQueryKey,
  useAssignTag,
  useClearTagAccessPolicy,
  useDeleteTag,
  useSetTagAccessPolicy,
  useUnassignTag,
  useUpdateTag,
} from "../tags";

afterEach(() => vi.restoreAllMocks());

function setup() {
  vi.spyOn(globalThis, "fetch").mockImplementation(async () =>
    new Response(JSON.stringify({ ok: true, skipped: [] }), { status: 200, headers: { "content-type": "application/json" } }),
  );
  const qc = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  const spy = vi.spyOn(qc, "invalidateQueries");
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
  const keys = () => spy.mock.calls.map((c) => JSON.stringify(c[0]?.queryKey));
  return { wrapper, keys };
}

const USAGE_T1 = JSON.stringify(getControlPlaneGovernanceTagUsageQueryKey("t1"));
const TAGS = JSON.stringify(tagsQueryKey);

describe("every tag write invalidates the list and the tag's usage", () => {
  it.each([
    ["update", () => useUpdateTag(), { id: "t1", name: "x" }],
    ["set clearance", () => useSetTagAccessPolicy(), { id: "t1", required_roles: ["r"], required_groups: [] }],
    ["clear clearance", () => useClearTagAccessPolicy(), "t1"],
    ["assign to a BI object", () => useAssignTag("w1", "chart", "c1"), "t1"],
    ["unassign from a BI object", () => useUnassignTag("w1", "chart", "c1"), "t1"],
    ["assign to a table", () => useAssignResourceTag("wh1"), { schema: "s", table: "t", tag_id: "t1" }],
    ["unassign from a table", () => useUnassignResourceTag("wh1"), { schema: "s", table: "t", tag_id: "t1" }],
  ] as const)("%s", async (_what, hook, vars) => {
    const { wrapper, keys } = setup();
    const { result } = renderHook(hook as () => { mutate: (v: unknown) => void; isSuccess: boolean }, { wrapper });
    result.current.mutate(vars);
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    await waitFor(() => expect(keys()).toEqual(expect.arrayContaining([TAGS, USAGE_T1])));
  });
});

/**
 * Data's Hand-written section (route A) matches tag policies through the
 * object's tags, by tag NAME: a Data assignment, a rename or a delete changes
 * it on every object page, whatever its catalog. A tag's Hand-written tab
 * (route D) matches by the tag's name: a rename or a delete changes it.
 */
describe("tag writes that change hand-written reach", () => {
  const ROUTE_A = getControlPlaneGovernanceObjectHandwrittenQueryKey("c-any", { schema: "s", table: "t" });
  const ROUTE_D_T1 = JSON.stringify(getControlPlaneGovernanceTagHandwrittenQueryKey("t1"));

  it.each([
    ["assign to a table", () => useAssignResourceTag("wh1"), { schema: "s", table: "t", tag_id: "t1" }, false],
    ["unassign from a table", () => useUnassignResourceTag("wh1"), { schema: "s", table: "t", tag_id: "t1" }, false],
    ["update", () => useUpdateTag(), { id: "t1", name: "x" }, true],
  ] as const)("%s", async (_what, hook, vars, byName) => {
    vi.spyOn(globalThis, "fetch").mockImplementation(async () =>
      new Response(JSON.stringify({ ok: true }), { status: 200, headers: { "content-type": "application/json" } }),
    );
    const qc = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
    const spy = vi.spyOn(qc, "invalidateQueries");
    const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
    const { result } = renderHook(hook as () => { mutate: (v: unknown) => void; isSuccess: boolean }, { wrapper });
    result.current.mutate(vars);
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    const filters = () => spy.mock.calls.map((c) => c[0]);
    await waitFor(() =>
      expect(filters().some((f) => f?.predicate?.({ queryKey: ROUTE_A } as never) === true)).toBe(true),
    );
    // The predicate is route A's family only.
    const pred = filters().find((f) => f?.predicate)?.predicate;
    expect(pred?.({ queryKey: getControlPlaneGovernanceTagUsageQueryKey("t1") } as never)).toBe(false);
    expect(pred?.({ queryKey: tagsQueryKey } as never)).toBe(false);
    const keys = filters().map((f) => JSON.stringify(f?.queryKey));
    expect(keys.includes(ROUTE_D_T1)).toBe(byName);
  });
});

/**
 * A delete refreshes the list and route A, and DROPS the deleted tag's own
 * reads (its usage, its route D): refetched, they would only 404 (live result
 * §7, Task 13).
 */
describe("a tag delete", () => {
  it("invalidates the list and route A, and removes the tag's own usage and route D without refetching them", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation(async () => new Response(null, { status: 204 }));
    const qc = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
    const usage = getControlPlaneGovernanceTagUsageQueryKey("t1");
    const routeD = getControlPlaneGovernanceTagHandwrittenQueryKey("t1");
    qc.setQueryData(usage, { status: 200, data: {} });
    qc.setQueryData(routeD, { status: 200, data: {} });
    const spy = vi.spyOn(qc, "invalidateQueries");
    const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
    const { result } = renderHook(() => useDeleteTag(), { wrapper });
    result.current.mutate("t1");
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    const filters = spy.mock.calls.map((c) => c[0]);
    expect(filters.map((f) => JSON.stringify(f?.queryKey))).toContain(JSON.stringify(tagsQueryKey));
    const routeA = getControlPlaneGovernanceObjectHandwrittenQueryKey("c-any", { schema: "s", table: "t" });
    expect(filters.some((f) => f?.predicate?.({ queryKey: routeA } as never) === true)).toBe(true);
    expect(filters.map((f) => JSON.stringify(f?.queryKey))).not.toContain(JSON.stringify(usage));
    expect(qc.getQueryData(usage)).toBeUndefined();
    expect(qc.getQueryData(routeD)).toBeUndefined();
  });
});

describe("useDeleteTag", () => {
  // `DELETE /tags/{id}` answers 204 with no body (spec premise 11); any 2xx is success (#4a D2).
  it("treats the 204 with no body as success", async () => {
    vi.spyOn(globalThis, "fetch").mockImplementation(async () => new Response(null, { status: 204 }));
    const qc = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
    const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
    const { result } = renderHook(() => useDeleteTag(), { wrapper });
    result.current.mutate("t1");
    await waitFor(() => expect(result.current.isSuccess || result.current.isError).toBe(true));
    expect(result.current.error).toBeNull();
  });
});

/**
 * A rename or a delete changes what every object's chips say (Data's
 * `GET /governance/tags/{catalog}` and a BI object's tags carry the name) and
 * the catalogue under its generated key (`AddTagDialog` reads it): both are
 * refetched, not left stale until a reload.
 */
describe("a tag rename or delete refreshes the chips and the generated list", () => {
  const DATA_CHIPS = getControlPlaneGovernanceObjectTagsQueryKey("c-any", { schema: "s", table: "t" });
  const BI_CHIPS = objectTagsQueryKey("w1", "chart", "c1");
  const LIST = JSON.stringify(getControlPlaneTagsListTagsQueryKey());

  it.each([
    ["rename", () => useUpdateTag(), { id: "t1", name: "x" }, 200],
    ["delete", () => useDeleteTag(), "t1", 204],
  ] as const)("%s", async (_what, hook, vars, status) => {
    vi.spyOn(globalThis, "fetch").mockImplementation(async () =>
      status === 204
        ? new Response(null, { status })
        : new Response(JSON.stringify({ id: "t1" }), { status, headers: { "content-type": "application/json" } }),
    );
    const qc = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
    const spy = vi.spyOn(qc, "invalidateQueries");
    const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
    const { result } = renderHook(hook as () => { mutate: (v: unknown) => void; isSuccess: boolean }, { wrapper });
    result.current.mutate(vars);
    await waitFor(() => expect(result.current.isSuccess).toBe(true));
    const filters = spy.mock.calls.map((c) => c[0]);
    const matches = (queryKey: readonly unknown[]) =>
      filters.some((f) => f?.predicate?.({ queryKey } as never) === true || JSON.stringify(f?.queryKey) === JSON.stringify(queryKey));
    expect(matches(DATA_CHIPS)).toBe(true);
    expect(matches(BI_CHIPS)).toBe(true);
    expect(filters.map((f) => JSON.stringify(f?.queryKey))).toContain(LIST);
  });
});
