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

test("the spanning header cell is a rowheader, not an ordinary cell", () => {
  // `getByRole` runs the real role + accessible-name computation, so this fails
  // if the attribute is dropped OR if it is put somewhere the ARIA table model
  // rejects — reading `getAttribute("role")` would only prove the string is
  // present, which is not the same claim.
  //
  // This is the whole of what the COMPONENT can be held to: it emits one cell,
  // so "the folder header is the only rowheader in the table" is not a claim it
  // can fail. That assertion used to live here over a fixture containing
  // nothing but the header row, where it could only have failed if this
  // component emitted two rowheaders — which it structurally cannot. It is now
  // in `routes/explore/__tests__/Charts.test.tsx`, over a table that has real
  // data rows for a stray role to appear on.
  //
  // The `colSpan` prop is deliberately NOT asserted here. `toHaveAttribute
  // ("colspan", "4")` pins React's prop-to-attribute lowercasing — a library's
  // behaviour, not this file's.
  render(<table><tbody>
    <FolderHeaderRow name="Revenue" count={3} colSpan={4} collapsed={false} onToggle={vi.fn()} />
  </tbody></table>);
  expect(screen.getByRole("rowheader", { name: /Revenue/ })).toBeInTheDocument();
});
