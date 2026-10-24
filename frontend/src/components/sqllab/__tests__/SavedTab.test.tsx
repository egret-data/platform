import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, test, it, vi } from "vitest";

const savedData = [
  { id: "q1", database_id: "db1", name: "Monthly revenue", sql: "SELECT 1", description: "rev by month", created_by: null, folder_id: "f1", updated_at: new Date(Date.now() - 3 * 3600_000).toISOString() },
];
const updateMock = vi.fn().mockResolvedValue({});
const deleteMock = vi.fn();
vi.mock("@/api/sqllab", () => ({
  useSavedQueries: () => ({ data: savedData }),
  useUpdateSavedQuery: () => ({ mutateAsync: updateMock }),
  useDeleteSavedQuery: () => ({ mutate: deleteMock }),
}));

const setFolderMock = vi.fn().mockResolvedValue({});
const createFolderMock = vi.fn().mockResolvedValue({ id: "f2", name: "F2" });
const foldersMock = vi.fn(() => ({ data: [{ id: "f1", name: "Reports" }] }));
vi.mock("@/api/folders", () => ({
  useFolders: () => foldersMock(),
  useSetFolder: () => ({ mutateAsync: setFolderMock }),
  useFolderMutations: () => ({
    create: { mutateAsync: createFolderMock },
    rename: { mutateAsync: vi.fn().mockResolvedValue({}) },
    remove: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
  }),
}));

// ShareDialog talks to the generated shares hooks + the principal picker; stub
// them so opening the dialog renders without a network round-trip.
vi.mock("@/api/generated/shares/shares", () => ({
  getSharesGrantListQueryKey: () => ["grants"],
  useSharesGrantList: () => ({ data: { data: [] }, isLoading: false, isError: false, error: null }),
  useSharesGrantPut: () => ({ mutate: vi.fn(), isPending: false }),
  useSharesGrantDelete: () => ({ mutate: vi.fn(), isPending: false }),
}));

vi.mock("@/api/policies", () => ({
  listPrincipals: vi.fn(() => Promise.resolve([])),
}));

import { SavedTab } from "@/components/sqllab/SavedTab";

const databases = [{ id: "db1", name: "trino-local", engine: "trino" }];

function renderTab(onOpen = vi.fn()) {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <SavedTab ws="eng" databases={databases} onOpen={onOpen} />
    </QueryClientProvider>,
  );
}

test("renders name, db, and description", () => {
  renderTab();
  expect(screen.getByText("Monthly revenue")).toBeInTheDocument();
  expect(screen.getByText("trino-local")).toBeInTheDocument();
  expect(screen.getByText("rev by month")).toBeInTheDocument();
});

test("clicking the row opens the query", () => {
  const onOpen = vi.fn();
  renderTab(onOpen);
  fireEvent.click(screen.getByText("Monthly revenue"));
  // KI-083: the query's name travels with it, so the new tab is titled after it.
  expect(onOpen).toHaveBeenCalledWith("SELECT 1", "db1", "Monthly revenue");
});

test("edit opens the dialog prefilled and saving calls update with the full body", async () => {
  updateMock.mockClear();
  renderTab();
  fireEvent.click(screen.getByRole("button", { name: "Edit" }));
  expect(screen.getByText("Edit query")).toBeInTheDocument();
  expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Monthly revenue");
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(updateMock).toHaveBeenCalledWith(
    expect.objectContaining({ id: "q1", database_id: "db1", name: "Monthly revenue", sql: "SELECT 1", description: "rev by month" }),
  ));
});

test("delete asks for confirmation, then deletes", () => {
  deleteMock.mockClear();
  renderTab();
  fireEvent.click(screen.getByRole("button", { name: "Delete" }));
  const dialog = screen.getByRole("alertdialog");
  expect(within(dialog).getByText("Delete saved query?")).toBeInTheDocument();
  expect(deleteMock).not.toHaveBeenCalled();
  fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
  expect(deleteMock).toHaveBeenCalledWith("q1");
});

it("groups saved queries under folder headers and offers a Share action", async () => {
  const user = userEvent.setup();
  renderTab();

  // Folder grouping: the "Reports" folder header renders.
  expect(screen.getByText("Reports")).toBeInTheDocument();

  // The row's actions menu exposes a Share… item that opens the ShareDialog.
  await user.click(screen.getByRole("button", { name: "Actions" }));
  await user.click(await screen.findByText("Share…"));
  expect(await screen.findByText('Share "Monthly revenue"')).toBeInTheDocument();
  expect(
    screen.getByText("Grant people and groups access to this saved_query."),
  ).toBeInTheDocument();
});

/** KI-093: a folder can be made from the list itself, as on the Datasets list. */
test("New folder creates an empty folder without moving a query", async () => {
  createFolderMock.mockClear(); setFolderMock.mockClear();
  const user = userEvent.setup();
  renderTab();
  await user.click(screen.getByRole("button", { name: "New folder" }));
  const dialog = screen.getByRole("dialog");
  await user.type(within(dialog).getByPlaceholderText("Folder name"), "u4-folder");
  await user.click(within(dialog).getByRole("button", { name: "Create" }));
  await waitFor(() => expect(createFolderMock).toHaveBeenCalledWith("u4-folder"));
  expect(setFolderMock).not.toHaveBeenCalled();
});
