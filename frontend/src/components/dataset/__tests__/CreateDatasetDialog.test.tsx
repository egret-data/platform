import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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

test("renders both tab triggers", () => {
  renderDialog();
  expect(screen.getByRole("tab", { name: "From table" })).toBeInTheDocument();
  expect(screen.getByRole("tab", { name: "From SQL" })).toBeInTheDocument();
});

test("dataset name input is present", () => {
  renderDialog();
  expect(screen.getByPlaceholderText("Dataset name")).toBeInTheDocument();
});

test("From SQL tab shows Create button disabled when SQL textarea is empty", async () => {
  renderDialog({ initialTab: "sql" });
  // Click "From SQL" tab to activate it
  await userEvent.click(screen.getByRole("tab", { name: "From SQL" }));
  // Find the Create button inside the SQL tab — it should be disabled because sql is empty
  const createButtons = screen.getAllByRole("button", { name: "Create" });
  // The SQL tab's Create button is disabled when SQL is empty
  expect(createButtons.some((btn) => btn.hasAttribute("disabled"))).toBe(true);
});

test("SQL Create button enabled after typing SQL", async () => {
  renderDialog({ initialTab: "sql" });
  await userEvent.click(screen.getByRole("tab", { name: "From SQL" }));
  const textarea = screen.getByPlaceholderText("SELECT …");
  await userEvent.type(textarea, "SELECT 1");
  // After typing, no Create button should be disabled (the SQL one is now enabled)
  const createButtons = screen.getAllByRole("button", { name: "Create" });
  const sqlCreate = createButtons[createButtons.length - 1]; // SQL Create is the last one
  expect(sqlCreate).not.toBeDisabled();
});
