import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

/** KI-158: the dataset header said "1 metrics". */

const col = (column_name: string) => ({
  column_name, data_type: "varchar", type_generic: "string", is_temporal: false, ordinal: 0, expression: null,
  verbose_name: "", description: "", is_calculated: false, groupby: true, filterable: true, format: "", semantic_type: "",
});
const metric = {
  id: "m1", metric_name: "n", expression: "count(*)", data_type: "bigint", type_generic: "numeric", verbose_name: "",
  description: "", ordinal: 0, format: "", certified: false, certified_by: "", certification_details: "",
};
const detail = {
  id: "ds1", name: "Order priority summary", description: "", database_id: "db1", kind: "virtual", catalog: null,
  schema: null, table: null, sql: "SELECT 1", updated_at: "2026-10-21T00:00:00Z", columns: [col("a")],
  metrics: [metric], certified: false, certified_by: "", certification_details: "", deleted_at: null,
};
const state = { detail };

vi.mock("@/auth/useAuth", () => ({ useAuth: () => ({ me: { platform_roles: [], permissions: [] } }) }));
vi.mock("@/api/datasets", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api/datasets")>()),
  useDataset: () => ({ data: state.detail, isLoading: false, isError: false }),
  useRefreshDataset: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useUpdateDataset: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useSetDatasetCertification: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useDatasetMutations: () => Object.fromEntries(
    ["addColumn", "updateColumn", "deleteColumn", "addMetric", "updateMetric", "deleteMetric"]
      .map((k) => [k, { mutateAsync: vi.fn(), isPending: false }]),
  ),
}));

async function mount() {
  const { default: DatasetDetail } = await import("../DatasetDetail");
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={["/explore/eng/datasets/ds1"]}>
        <Routes>
          <Route path="/explore/:workspace/datasets/:id" element={<DatasetDetail />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("DatasetDetail header counts", () => {
  it("is singular for one column and one metric", async () => {
    state.detail = detail;
    await mount();
    // The header chip (the Columns tab below repeats the column count in the same words).
    expect((await screen.findAllByText("1 column")).length).toBeGreaterThan(0);
    expect(screen.getByText("1 metric")).toBeInTheDocument();
    expect(screen.queryByText("1 metrics")).toBeNull();
    expect(screen.queryByText("1 columns")).toBeNull();
  });

  it("is plural otherwise", async () => {
    state.detail = { ...detail, columns: [col("a"), col("b")], metrics: [] };
    await mount();
    expect((await screen.findAllByText("2 columns")).length).toBeGreaterThan(0);
    expect(screen.getByText("0 metrics")).toBeInTheDocument();
  });
});
