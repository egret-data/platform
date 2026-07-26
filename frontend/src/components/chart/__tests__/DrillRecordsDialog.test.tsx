import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { DrillRecordsDialog } from "../DrillRecordsDialog";
import type { ResultColumn } from "../ChartTable";

// `DrillRecordsDialog` now runs through `useSpecData` -> `chartsChRunSpec`
// (`POST /charts/run`), a single request, rather than the old build -> submit
// -> poll pipeline through the free-form SQL endpoint.
const chartsChRunSpec = vi.fn();
vi.mock("@/api/generated/charts/charts", () => ({
  chartsChRunSpec: (...a: unknown[]) => chartsChRunSpec(...a),
}));

beforeEach(() => {
  chartsChRunSpec.mockReset();
  chartsChRunSpec.mockResolvedValue({ data: { columns: [], rows: [] } });
});

const baseProps = {
  onOpenChange: () => {},
  ws: "w1",
  datasetId: "ds1",
  filters: [],
};

function wrapper({ children }: { children: React.ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

describe("DrillRecordsDialog", () => {
  it("does not build a query while columns are empty, then builds once columns arrive", async () => {
    const { rerender } = render(
      <DrillRecordsDialog {...baseProps} open columns={[]} />,
      { wrapper },
    );

    // Empty-columns guard: no request fired yet (columns not loaded).
    expect(chartsChRunSpec).not.toHaveBeenCalled();

    const columns: ResultColumn[] = [
      { name: "region", type: "varchar" },
      { name: "revenue", type: "double" },
    ];

    // Dataset detail arrives late → columns become non-empty.
    rerender(<DrillRecordsDialog {...baseProps} open columns={columns} />);

    await vi.waitFor(() => {
      expect(chartsChRunSpec).toHaveBeenCalledTimes(1);
    });
    const [ws, body] = chartsChRunSpec.mock.calls[0];
    expect(ws).toBe("w1");
    expect(body).toEqual({
      dataset_id: "ds1",
      query_spec: expect.objectContaining({ columns: ["region", "revenue"] }),
    });
  });

  it("does not show 'No matching records.' before columns have arrived (the query is disabled, not empty)", () => {
    render(<DrillRecordsDialog {...baseProps} open columns={[]} />, { wrapper });

    // Disabled ≠ completed-and-empty: the query never fired for this render,
    // so the empty-state message must not appear — only the loading state may.
    expect(screen.queryByText("No matching records.")).not.toBeInTheDocument();
    expect(screen.getByText("Loading…")).toBeInTheDocument();
  });

  it("shows the real empty state once a completed query actually returns no rows", async () => {
    const columns: ResultColumn[] = [{ name: "region", type: "varchar" }];
    render(<DrillRecordsDialog {...baseProps} open columns={columns} />, { wrapper });

    await vi.waitFor(() => {
      expect(screen.getByText("No matching records.")).toBeInTheDocument();
    });
  });

  it("does not re-fire on parent re-render with the same column set", async () => {
    const columns: ResultColumn[] = [{ name: "region", type: "varchar" }];
    const { rerender } = render(
      <DrillRecordsDialog {...baseProps} open columns={columns} />,
      { wrapper },
    );
    await vi.waitFor(() => expect(chartsChRunSpec).toHaveBeenCalledTimes(1));

    // Fresh array reference, same column names → must not re-fire (the spec
    // serializes to the same content, and `useSpecData` dedupes on that).
    rerender(
      <DrillRecordsDialog {...baseProps} open columns={[{ name: "region", type: "varchar" }]} />,
    );
    await Promise.resolve();
    expect(chartsChRunSpec).toHaveBeenCalledTimes(1);
  });
});
