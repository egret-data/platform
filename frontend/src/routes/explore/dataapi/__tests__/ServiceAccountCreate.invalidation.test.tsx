import { describe, it, expect, vi, beforeEach } from "vitest";
import { render } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

/**
 * `queryClient.ts` sets `staleTime: 30000` and the orval-generated mutation
 * hooks carry no `onSuccess` — there is not one `invalidate` in the generated
 * file. The service-account list key is shared by FIVE surfaces (the Clients
 * table, Try-it's "Run as" picker, the Access checklist, the OpenAPI download
 * dialog and the Response preview), and this page navigates straight BACK to
 * the first of them — so a create wired to the generated hook would land the
 * author on a table without the row they just made.
 *
 * These assertions used to live in `Clients.test.tsx`, next to delete's,
 * because the list page built the create mutation. They moved here with the
 * mutation itself when creating became its own page.
 *
 * The sibling `ServiceAccountCreate.test.tsx` cannot carry them: that file
 * mocks `@/api/dataApi` wholesale, which is precisely the wrapper under test
 * here. So this file mocks the GENERATED module instead and drives the
 * `onSuccess` the wrapper attached, asserting on the KEY — a wrapper that
 * invalidated the wrong workspace would still "call invalidateQueries".
 */
vi.mock("@/api/generated/data-api/data-api");

import * as api from "@/api/generated/data-api/data-api";
import ServiceAccountCreate from "@/routes/explore/dataapi/ServiceAccountCreate";

function renderCreate() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const utils = render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/eng/data-api/clients/new"]}>
          <Routes>
            <Route path="/explore/:workspace/data-api/clients/new" element={<ServiceAccountCreate />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
  return { ...utils, qc };
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.useDataApiCreateServiceAccount).mockReturnValue(
    { mutateAsync: vi.fn(), isPending: false } as unknown as ReturnType<typeof api.useDataApiCreateServiceAccount>,
  );
  // Automocking the generated module turns the query-key builders into
  // `vi.fn()`s returning `undefined` — give it an argument-sensitive fake so
  // "the right list was invalidated" is distinguishable from
  // "invalidateQueries was called with nothing".
  vi.mocked(api.getDataApiListServiceAccountsQueryKey).mockImplementation(
    (ws) => ["dataApiListServiceAccounts", ws] as unknown as
      ReturnType<typeof api.getDataApiListServiceAccountsQueryKey>,
  );
});

async function fireSuccess(variables: Record<string, unknown>) {
  const opts = vi.mocked(api.useDataApiCreateServiceAccount).mock.calls.at(-1)?.[0] as
    | { mutation?: { onSuccess?: (...a: never[]) => unknown } }
    | undefined;
  await opts?.mutation?.onSuccess?.(
    { status: 201, data: {}, headers: new Headers() } as never,
    variables as never,
    undefined as never,
    undefined as never,
  );
}

describe("ServiceAccountCreate — cache invalidation", () => {
  it("Create invalidates the list, so the Clients table it returns to holds the new row", async () => {
    const { qc } = renderCreate();
    const spy = vi.spyOn(qc, "invalidateQueries");
    await fireSuccess({ workspace: "eng", data: {} });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListServiceAccounts", "eng"] }),
    );
  });

  // The workspace is read from the mutation VARIABLES, never from a separate
  // hook argument that could drift from where the write went.
  it("invalidates the list of the workspace the write actually went to", async () => {
    const { qc } = renderCreate();
    const spy = vi.spyOn(qc, "invalidateQueries");
    await fireSuccess({ workspace: "other-ws", data: {} });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListServiceAccounts", "other-ws"] }),
    );
  });
});
