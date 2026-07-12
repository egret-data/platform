import { render, screen, fireEvent } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, test, vi } from "vitest";

// Controllable mock of the generated mutation hook. Each test sets `mockState`
// before rendering; `mockMutate` is asserted for the mount + Refresh calls.
const mockMutate = vi.fn();
let mockState: Record<string, unknown> = {};

vi.mock("@/api/generated/datasets/datasets", () => ({
  useDatasetsDsData: () => ({ mutate: mockMutate, ...mockState }),
}));

import { PreviewTab } from "@/components/dataset/PreviewTab";

function renderWithProviders(ui: React.ReactElement) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>{ui}</MemoryRouter>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  mockMutate.mockReset();
  mockState = {};
});

test("PreviewTab: success renders column header + cell value and runs on mount", () => {
  mockState = {
    isPending: false,
    isError: false,
    error: null,
    data: {
      status: 200,
      data: {
        columns: [{ name: "order_id", type: "bigint" }],
        rows: [[42]],
      },
    },
  };
  renderWithProviders(<PreviewTab ws="eng" datasetId="ds1" />);

  // Ran once on mount.
  expect(mockMutate).toHaveBeenCalledTimes(1);
  expect(mockMutate).toHaveBeenCalledWith({ workspace: "eng", id: "ds1", data: { limit: 100 } });
  // Column header + a cell value.
  expect(screen.getByText("order_id")).toBeInTheDocument();
  expect(screen.getByText("42")).toBeInTheDocument();
});

test("PreviewTab: error state shows the Alert message", () => {
  mockState = {
    isPending: false,
    isError: true,
    error: new Error("Access denied"),
    data: undefined,
  };
  renderWithProviders(<PreviewTab ws="eng" datasetId="ds1" />);

  expect(screen.getByRole("alert")).toBeInTheDocument();
  expect(screen.getByText("Access denied")).toBeInTheDocument();
});

test("PreviewTab: Refresh click re-invokes mutate", () => {
  mockState = {
    isPending: false,
    isError: false,
    error: null,
    data: { status: 200, data: { columns: [{ name: "c", type: "text" }], rows: [["v"]] } },
  };
  renderWithProviders(<PreviewTab ws="eng" datasetId="ds1" />);

  // Once from mount.
  expect(mockMutate).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
  // Once more from the Refresh click.
  expect(mockMutate).toHaveBeenCalledTimes(2);
});
