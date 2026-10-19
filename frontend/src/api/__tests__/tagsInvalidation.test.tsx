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
import { getControlPlaneGovernanceTagUsageQueryKey } from "../generated/control-plane-governance/control-plane-governance";
import { useAssignResourceTag, useUnassignResourceTag } from "../catalogTags";
import {
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
    ["delete", () => useDeleteTag(), "t1"],
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
