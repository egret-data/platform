import { render, screen, within } from "@testing-library/react";
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

/** Return the SQL tab panel — the one that is currently visible (not hidden). */
function getSqlTabPanel() {
  const panels = screen.getAllByRole("tabpanel");
  // Radix may keep both panels in the DOM; pick the one that contains the SQL textarea.
  return panels.find((p) => within(p).queryByPlaceholderText("SELECT …") !== null)!;
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
  // Click "From SQL" tab to make sure it is active
  await userEvent.click(screen.getByRole("tab", { name: "From SQL" }));
  // Scope to the SQL tab panel specifically
  const sqlPanel = getSqlTabPanel();
  const createBtn = within(sqlPanel).getByRole("button", { name: "Create" });
  expect(createBtn).toBeDisabled();
});

test("SQL Create button enabled after typing SQL", async () => {
  renderDialog({ initialTab: "sql" });
  await userEvent.click(screen.getByRole("tab", { name: "From SQL" }));
  const textarea = screen.getByPlaceholderText("SELECT …");
  await userEvent.type(textarea, "SELECT 1");
  // Scope to the SQL tab panel to get the right Create button
  const sqlPanel = getSqlTabPanel();
  const createBtn = within(sqlPanel).getByRole("button", { name: "Create" });
  expect(createBtn).not.toBeDisabled();
});
