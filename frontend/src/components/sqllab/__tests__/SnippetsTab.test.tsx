import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";

const deleteMock = vi.fn().mockResolvedValue(undefined);
vi.mock("@/api/sqllab", () => ({
  useSnippets: () => ({ data: [{ id: "s1", name: "u4-snip", body: "SELECT 1", description: "", updated_at: "" }] }),
  useCreateSnippet: () => ({ mutateAsync: vi.fn() }),
  useUpdateSnippet: () => ({ mutateAsync: vi.fn() }),
  useDeleteSnippet: () => ({ mutateAsync: deleteMock }),
}));

import { SnippetsTab } from "@/components/sqllab/SnippetsTab";

/** KI-091: deleting a snippet asks first, as deleting a saved query does. */
test("delete asks for confirmation, then deletes", async () => {
  render(<SnippetsTab ws="eng" currentSql="" onInsert={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Delete u4-snip" }));
  const dialog = screen.getByRole("alertdialog");
  expect(within(dialog).getByText("Delete snippet?")).toBeInTheDocument();
  expect(deleteMock).not.toHaveBeenCalled();
  fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
  await waitFor(() => expect(deleteMock).toHaveBeenCalledWith("s1"));
});

test("cancelling the confirmation keeps the snippet", () => {
  deleteMock.mockClear();
  render(<SnippetsTab ws="eng" currentSql="" onInsert={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Delete u4-snip" }));
  fireEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
  expect(deleteMock).not.toHaveBeenCalled();
});
