import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, test, vi } from "vitest";

// Mock @/api/sqllab so we control what useDatabases returns without a real server.
vi.mock("@/api/sqllab", () => ({
  useDatabases: () => ({ data: [{ id: "db1", name: "wh", engine: "trino" }] }),
  useCatalogs: () => ({ data: [] }),
  useSchemas: () => ({ data: [] }),
  useTables: () => ({ data: [] }),
}));

// Mock @/api/datasets so the mutation hook doesn't fire real fetch calls.
vi.mock("@/api/datasets", () => ({
  useCreateDataset: () => ({ mutateAsync: vi.fn(), isPending: false }),
}));

import { CreateDatasetDialog } from "@/components/dataset/CreateDatasetDialog";

function renderDialog(props: Partial<React.ComponentProps<typeof CreateDatasetDialog>> = {}) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/datasets"]}>
        <CreateDatasetDialog ws="eng" open={true} onOpenChange={() => {}} {...props} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

// --- Default (physical) mode: pick a table, no SQL editing here. ---

test("physical mode: name input + database picker are present", () => {
  renderDialog();
  expect(screen.getByPlaceholderText("Dataset name")).toBeInTheDocument();
  expect(screen.getByText("Connection")).toBeInTheDocument();
});

test("physical mode: no SQL editor, but a link to SQL Lab for custom queries", () => {
  renderDialog();
  // Virtual/SQL datasets are made in SQL Lab now — not via a tab here.
  expect(screen.queryByRole("tab", { name: "From SQL" })).not.toBeInTheDocument();
  expect(screen.queryByPlaceholderText("SELECT …")).not.toBeInTheDocument();
  const link = screen.getByRole("link", { name: /SQL Lab/i });
  expect(link).toHaveAttribute("href", "/explore/eng/sql-lab");
});

test("physical mode: Create is disabled until a table is chosen", () => {
  renderDialog();
  expect(screen.getByRole("button", { name: "Create" })).toBeDisabled();
});

// --- SQL / "Save as dataset" mode: query is supplied (from SQL Lab / Duplicate). ---

test("sql mode: shows the query read-only and a name field", () => {
  renderDialog({ initialSql: "SELECT 1", initialDbId: "db1" });
  expect(screen.getByText("Save as dataset")).toBeInTheDocument();
  expect(screen.getByText("SELECT 1")).toBeInTheDocument();
  expect(screen.getByPlaceholderText("Dataset name")).toBeInTheDocument();
});

test("sql mode: Create dataset disabled until a name is entered", () => {
  renderDialog({ initialSql: "SELECT 1", initialDbId: "db1" });
  // Name starts empty → disabled.
  expect(screen.getByRole("button", { name: "Create dataset" })).toBeDisabled();
});

test("sql mode: Create dataset enabled once a name is supplied", () => {
  renderDialog({ initialSql: "SELECT 1", initialDbId: "db1", initialName: "My dataset" });
  expect(screen.getByRole("button", { name: "Create dataset" })).not.toBeDisabled();
});
