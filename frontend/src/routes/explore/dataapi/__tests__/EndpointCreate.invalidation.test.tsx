import { describe, it, expect, vi, beforeEach } from "vitest";
import { render } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

/**
 * `queryClient.ts` sets `staleTime: 30000` and the orval-generated mutation
 * hooks carry no `onSuccess` — there is not one `invalidate` in the generated
 * file. So a create wired straight to the generated hook navigates to the new
 * endpoint and, on Back, shows a list without the row it just created.
 *
 * This assertion used to live in `Endpoints.test.tsx`, next to Disable/Delete/
 * Bookmark, because the list page built the create mutation. It moved here with
 * the mutation itself when creating became its own page.
 *
 * The sibling `EndpointCreate.test.tsx` cannot carry it: that file mocks
 * `@/api/dataApi` wholesale, which is precisely the wrapper under test here.
 * So this file mocks the GENERATED module instead and drives the `onSuccess`
 * the wrapper attached, asserting on the KEY — a wrapper that invalidated the
 * wrong workspace would still "call invalidateQueries".
 */
vi.mock("@/api/generated/data-api/data-api");
vi.mock("@/api/databases");

import * as api from "@/api/generated/data-api/data-api";
import * as databasesApi from "@/api/databases";
import EndpointCreate from "@/routes/explore/dataapi/EndpointCreate";

function renderCreate() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const utils = render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/eng/data-api/endpoints/new"]}>
          <Routes>
            <Route path="/explore/:workspace/data-api/endpoints/new" element={<EndpointCreate />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
  return { ...utils, qc };
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(databasesApi.useDatabases).mockReturnValue(
    { data: [], isLoading: false, isError: false } as unknown as ReturnType<typeof databasesApi.useDatabases>,
  );
  vi.mocked(api.useDataApiCreateEndpoint).mockReturnValue(
    { mutateAsync: vi.fn(), isPending: false } as unknown as ReturnType<typeof api.useDataApiCreateEndpoint>,
  );
  // Automocking the generated module turns the query-key builders into
  // `vi.fn()`s returning `undefined` — give it an argument-sensitive fake so
  // "the right list was invalidated" is distinguishable from
  // "invalidateQueries was called with nothing".
  vi.mocked(api.getDataApiListEndpointsQueryKey).mockImplementation(
    (ws) => ["dataApiListEndpoints", ws] as unknown as ReturnType<typeof api.getDataApiListEndpointsQueryKey>,
  );
});

describe("EndpointCreate — cache invalidation", () => {
  it("Create invalidates the endpoint list, so navigating Back shows the new endpoint", async () => {
    const { qc } = renderCreate();
    const spy = vi.spyOn(qc, "invalidateQueries");

    const opts = vi.mocked(api.useDataApiCreateEndpoint).mock.calls.at(-1)?.[0] as
      | { mutation?: { onSuccess?: (...a: never[]) => unknown } }
      | undefined;
    await opts?.mutation?.onSuccess?.(
      { status: 201, data: {}, headers: new Headers() } as never,
      { workspace: "eng", data: {} } as never,
      undefined as never,
      undefined as never,
    );

    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListEndpoints", "eng"] }),
    );
  });
});
