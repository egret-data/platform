import { describe, it, expect, vi, beforeEach } from "vitest";
import { render } from "@testing-library/react";
import { DrillRecordsDialog } from "../DrillRecordsDialog";
import type { ResultColumn } from "../ChartTable";

const buildMutateAsync = vi.fn(() => Promise.resolve({ sql: "select 1", database_id: "db1" }));
const submitMutateAsync = vi.fn(() => Promise.resolve({ id: "q1", status: "queued" }));

vi.mock("@/api/charts", () => ({
  useBuildChartData: () => ({ mutateAsync: buildMutateAsync }),
}));
vi.mock("@/api/sqllab", () => ({
  useSubmitQuery: () => ({ mutateAsync: submitMutateAsync }),
  useQueryStatus: () => ({ data: undefined }),
  useResults: () => ({ data: undefined }),
}));

beforeEach(() => {
  buildMutateAsync.mockClear();
  submitMutateAsync.mockClear();
});

const baseProps = {
  onOpenChange: () => {},
  ws: "w1",
  datasetId: "ds1",
  filters: [],
};

describe("DrillRecordsDialog", () => {
  it("does not build a query while columns are empty, then builds once columns arrive", async () => {
    const { rerender } = render(
      <DrillRecordsDialog {...baseProps} open columns={[]} />,
    );

    // Empty-columns guard: no build fired yet (columns not loaded).
    expect(buildMutateAsync).not.toHaveBeenCalled();

    const columns: ResultColumn[] = [
      { name: "region", type: "varchar" },
      { name: "revenue", type: "double" },
    ];

    // Dataset detail arrives late → columns become non-empty.
    rerender(<DrillRecordsDialog {...baseProps} open columns={columns} />);

    // Wait a tick for the async effect to run mutateAsync.
    await vi.waitFor(() => {
      expect(buildMutateAsync).toHaveBeenCalledTimes(1);
    });
    expect(buildMutateAsync).toHaveBeenCalledWith({
      dataset_id: "ds1",
      query_spec: expect.objectContaining({ columns: ["region", "revenue"] }),
    });
  });

  it("does not re-fire on parent re-render with the same column set", async () => {
    const columns: ResultColumn[] = [{ name: "region", type: "varchar" }];
    const { rerender } = render(
      <DrillRecordsDialog {...baseProps} open columns={columns} />,
    );
    await vi.waitFor(() => expect(buildMutateAsync).toHaveBeenCalledTimes(1));

    // Fresh array reference, same column names → must not re-fire (stable colKey).
    rerender(
      <DrillRecordsDialog {...baseProps} open columns={[{ name: "region", type: "varchar" }]} />,
    );
    await Promise.resolve();
    expect(buildMutateAsync).toHaveBeenCalledTimes(1);
  });
});
