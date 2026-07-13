import { render, screen, fireEvent } from "@testing-library/react";
import { FilterableTable, type Column } from "@/components/explore/FilterableTable";

interface Row {
  id: string;
  name: string;
}
const rows: Row[] = [
  { id: "a", name: "Alice" },
  { id: "b", name: "Bob" },
];
const columns: Column<Row>[] = [
  { key: "name", label: "Name", value: (r) => r.name, cell: (r) => r.name },
];

it("renders no checkbox column without selection", () => {
  render(<FilterableTable rows={rows} columns={columns} getId={(r) => r.id} />);
  expect(screen.queryByLabelText("Select all")).toBeNull();
});

it("renders checkboxes and fires selection callbacks", () => {
  const onToggle = vi.fn();
  const onToggleAll = vi.fn();
  render(
    <FilterableTable
      rows={rows}
      columns={columns}
      getId={(r) => r.id}
      selection={{ selected: new Set(["a"]), onToggle, allSelected: false, onToggleAll }}
    />,
  );
  fireEvent.click(screen.getByLabelText("Select all"));
  expect(onToggleAll).toHaveBeenCalled();
  const rowBoxes = screen.getAllByLabelText("Select row");
  fireEvent.click(rowBoxes[1]);
  expect(onToggle).toHaveBeenCalledWith("b");
});
