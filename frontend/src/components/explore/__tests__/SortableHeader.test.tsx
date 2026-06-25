import { render, screen, fireEvent } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { SortableHeader } from "@/components/explore/SortableHeader";

test("calls onSort with its key when clicked", () => {
  const onSort = vi.fn();
  render(<table><thead><tr>
    <SortableHeader sortKey="name" label="Name" current={{ key: "updated", dir: "desc" }} onSort={onSort} />
  </tr></thead></table>);
  fireEvent.click(screen.getByText("Name"));
  expect(onSort).toHaveBeenCalledWith("name");
});

test("shows a direction arrow only on the active key", () => {
  const { container, rerender } = render(<table><thead><tr>
    <SortableHeader sortKey="name" label="Name" current={{ key: "name", dir: "asc" }} onSort={() => {}} />
  </tr></thead></table>);
  expect(container.querySelector("svg")).toBeTruthy();
  rerender(<table><thead><tr>
    <SortableHeader sortKey="name" label="Name" current={{ key: "updated", dir: "asc" }} onSort={() => {}} />
  </tr></thead></table>);
  expect(container.querySelector("svg")).toBeNull();
});
