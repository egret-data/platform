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
