import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const removeMock = vi.fn().mockResolvedValue(undefined);
// c1 is newest → first row under the default "updated_at desc" sort.
const charts = [
  { id: "c1", dataset_id: "d1", name: "Revenue bar", description: "", viz_type: "bar", params: {}, created_by: null, updated_at: "2026-06-21T00:00:00Z" },
  { id: "c2", dataset_id: "d1", name: "Orders line", description: "", viz_type: "line", params: {}, created_by: null, updated_at: "2026-06-20T00:00:00Z" },
];
vi.mock("@/api/charts", () => ({
  useCharts: () => ({ data: charts, isLoading: false }),
  useChartMutations: () => ({ remove: { mutateAsync: removeMock } }),
}));

import Charts from "../Charts";

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/charts"]}>
        <Routes><Route path="/explore/:workspace/charts" element={<Charts />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("Charts list", () => {
  it("renders chart rows with their type labels", () => {
    renderAt();
    expect(screen.getByText("Revenue bar")).toBeInTheDocument();
    expect(screen.getByText("Orders line")).toBeInTheDocument();
    expect(screen.getByText("Bar")).toBeInTheDocument();
  });
  it("search filters by name", () => {
    renderAt();
    fireEvent.change(screen.getByPlaceholderText("Search charts…"), { target: { value: "orders" } });
    expect(screen.queryByText("Revenue bar")).toBeNull();
    expect(screen.getByText("Orders line")).toBeInTheDocument();
  });
  it("Delete asks to confirm, then deletes the first row", async () => {
    removeMock.mockClear();
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getAllByRole("button", { name: "Actions" })[0]);
    await user.click(await screen.findByText("Delete"));
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(removeMock).toHaveBeenCalledWith("c1");
  });
});
