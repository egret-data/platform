import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";

const savedData = [
  { id: "q1", database_id: "db1", name: "Monthly revenue", sql: "SELECT 1", description: "rev by month", created_by: null, updated_at: new Date(Date.now() - 3 * 3600_000).toISOString() },
];
const updateMock = vi.fn().mockResolvedValue({});
const deleteMock = vi.fn();
vi.mock("@/api/sqllab", () => ({
  useSavedQueries: () => ({ data: savedData }),
  useUpdateSavedQuery: () => ({ mutateAsync: updateMock }),
  useDeleteSavedQuery: () => ({ mutate: deleteMock }),
}));

import { SavedTab } from "@/components/sqllab/SavedTab";

const databases = [{ id: "db1", name: "trino-local", engine: "trino" }];

test("renders name, db, and description", () => {
  render(<SavedTab ws="eng" databases={databases} onOpen={vi.fn()} />);
  expect(screen.getByText("Monthly revenue")).toBeInTheDocument();
  expect(screen.getByText("trino-local")).toBeInTheDocument();
  expect(screen.getByText("rev by month")).toBeInTheDocument();
});

test("clicking the row opens the query", () => {
  const onOpen = vi.fn();
  render(<SavedTab ws="eng" databases={databases} onOpen={onOpen} />);
  fireEvent.click(screen.getByText("Monthly revenue"));
  expect(onOpen).toHaveBeenCalledWith("SELECT 1", "db1");
});

test("edit opens the dialog prefilled and saving calls update with the full body", async () => {
  updateMock.mockClear();
  render(<SavedTab ws="eng" databases={databases} onOpen={vi.fn()} />);
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
  render(<SavedTab ws="eng" databases={databases} onOpen={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Delete" }));
  const dialog = screen.getByRole("alertdialog");
  expect(within(dialog).getByText("Delete saved query?")).toBeInTheDocument();
  expect(deleteMock).not.toHaveBeenCalled();
  fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
  expect(deleteMock).toHaveBeenCalledWith("q1");
});
