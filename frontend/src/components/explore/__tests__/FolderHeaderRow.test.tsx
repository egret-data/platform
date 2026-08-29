import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { FolderHeaderRow } from "@/components/explore/FolderHeaderRow";

test("renders name + count and toggles", async () => {
  const onToggle = vi.fn();
  render(<table><tbody>
    <FolderHeaderRow name="Revenue" count={3} colSpan={4} collapsed={false} onToggle={onToggle} />
  </tbody></table>);
  expect(screen.getByText("Revenue")).toBeInTheDocument();
  expect(screen.getByText("(3)")).toBeInTheDocument();
  await userEvent.click(screen.getByLabelText("Toggle Revenue"));
  expect(onToggle).toHaveBeenCalledTimes(1);
});

test("actions menu shows Share…, Rename, Delete and fires onShare", async () => {
  const onShare = vi.fn();
  const onRename = vi.fn();
  const onDelete = vi.fn();
  render(<table><tbody>
    <FolderHeaderRow
      name="Revenue" count={3} colSpan={4} collapsed={false} onToggle={vi.fn()}
      onShare={onShare} onRename={onRename} onDelete={onDelete}
    />
  </tbody></table>);
  await userEvent.click(screen.getByLabelText("Folder Revenue actions"));
  expect(screen.getByText("Share…")).toBeInTheDocument();
  expect(screen.getByText("Rename")).toBeInTheDocument();
  expect(screen.getByText("Delete")).toBeInTheDocument();
  await userEvent.click(screen.getByText("Share…"));
  expect(onShare).toHaveBeenCalledTimes(1);
});

test("the spanning header cell is a rowheader, not another data cell", () => {
  // `getByRole` runs the real role + accessible-name computation, so this
  // fails if the attribute is dropped OR if it is put somewhere the ARIA table
  // model rejects — reading `getAttribute("role")` would only prove the string
  // is present, which is not the same claim.
  render(<table><tbody>
    <FolderHeaderRow name="Revenue" count={3} colSpan={4} collapsed={false} onToggle={vi.fn()} />
  </tbody></table>);
  const header = screen.getByRole("rowheader", { name: /Revenue/ });
  expect(header).toHaveAttribute("colspan", "4");
  // And the rows it heads are NOT rowheaders — a test that only asserted
  // presence would pass just as well if every cell in the table had the role.
  expect(screen.getAllByRole("rowheader")).toHaveLength(1);
});
