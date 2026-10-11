import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "@/api/client";

const request = vi.fn();
vi.mock("@/api/mutator", () => ({ customInstance: (...a: unknown[]) => request(...a) }));

import { useFlowSchema } from "@/components/workflows/editor/useFlowSchema";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

const SCHEMA = { title: "An egret flow" };
let client: QueryClient;
const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={client}>{children}</QueryClientProvider>
);

beforeEach(() => {
  request.mockReset();
  client = new QueryClient({ defaultOptions: { queries: { retryDelay: 0 } } });
});

describe("useFlowSchema", () => {
  it("loads the workspace's schema under the workspace's key, with its ETag", async () => {
    request.mockResolvedValue({ status: 200, data: SCHEMA, headers: new Headers({ etag: '"sha256-1"' }) });
    const { result } = renderHook(() => useFlowSchema("ws1"), { wrapper });
    expect(result.current.status).toBe("loading");
    await waitFor(() => expect(result.current.status).toBe("ready"));
    expect(result.current.schema).toEqual(SCHEMA);
    expect(result.current.etag).toBe('"sha256-1"');
    expect(request.mock.calls[0][0]).toBe("/api/ws/ws1/workflows/schema");
    expect(client.getQueryData(workflowsKeys.schema("ws1"))).toBeDefined();
    expect(client.getQueryData(workflowsKeys.schema("ws2"))).toBeUndefined();
  });

  it("reports an error, not an empty schema, and retry fetches again", async () => {
    request.mockRejectedValue(new ApiError(500, "internal error"));
    const { result } = renderHook(() => useFlowSchema("ws1"), { wrapper });
    await waitFor(() => expect(result.current.status).toBe("error"));
    expect(result.current.schema).toBeNull();
    expect(result.current.error).toBe("internal error");

    request.mockResolvedValue({ status: 200, data: SCHEMA, headers: new Headers({ etag: '"e2"' }) });
    act(() => result.current.retry());
    await waitFor(() => expect(result.current.status).toBe("ready"));
    expect(result.current.etag).toBe('"e2"');
  });
});
