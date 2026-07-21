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

it("does not paginate when pageSize is omitted", () => {
  render(<FilterableTable rows={rows} columns={columns} getId={(r) => r.id} />);
  expect(screen.queryByRole("button", { name: /next/i })).toBeNull();
  expect(screen.getByText("Alice")).toBeInTheDocument();
  expect(screen.getByText("Bob")).toBeInTheDocument();
});

it("caps rows to pageSize and pages through the rest", () => {
  const many: Row[] = Array.from({ length: 5 }, (_, i) => ({
    id: `r${i}`,
    name: `Row ${i}`,
  }));
  render(
    <FilterableTable rows={many} columns={columns} getId={(r) => r.id} pageSize={2} />,
  );
  // Page 1: first two rows only.
  expect(screen.getByText("Row 0")).toBeInTheDocument();
  expect(screen.queryByText("Row 2")).toBeNull();
  expect(screen.getByText(/Showing/)).toHaveTextContent("Showing 1–2 of 5");

  fireEvent.click(screen.getByRole("button", { name: /next/i }));
  expect(screen.getByText("Row 2")).toBeInTheDocument();
  expect(screen.queryByText("Row 0")).toBeNull();

  // Filtering resets back to the first page.
  fireEvent.change(screen.getByPlaceholderText("Search…"), {
    target: { value: "Row 4" },
  });
  expect(screen.getByText("Row 4")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /next/i })).toBeNull();
});

it("distinguishes an empty dataset from a search miss", () => {
  const { rerender } = render(
    <FilterableTable
      rows={[]}
      columns={columns}
      getId={(r) => r.id}
      emptyText="Nothing here yet"
      noMatchText="No matches."
    />,
  );
  // No data at all → the first-run emptyText.
  expect(screen.getByText("Nothing here yet")).toBeInTheDocument();
  expect(screen.queryByText("No matches.")).toBeNull();

  // Data exists but the search misses → noMatchText, not the first-run copy.
  rerender(
    <FilterableTable
      rows={rows}
      columns={columns}
      getId={(r) => r.id}
      emptyText="Nothing here yet"
      noMatchText="No matches."
    />,
  );
  fireEvent.change(screen.getByPlaceholderText("Search…"), {
    target: { value: "nobody-matches-this" },
  });
  expect(screen.getByText("No matches.")).toBeInTheDocument();
  expect(screen.queryByText("Nothing here yet")).toBeNull();
});

it("renders no checkbox column without selection", () => {
  render(<FilterableTable rows={rows} columns={columns} getId={(r) => r.id} />);
  expect(screen.queryByLabelText("Select all")).toBeNull();
});

it("renders checkboxes and fires selection callbacks", () => {
  const onToggle = vi.fn();
  render(
    <FilterableTable
      rows={rows}
      columns={columns}
      getId={(r) => r.id}
      selection={{ selected: new Set(["a"]), onToggle }}
    />,
  );
  // "Select all" toggles only the visible rows not already selected.
  fireEvent.click(screen.getByLabelText("Select all"));
  expect(onToggle).toHaveBeenCalledWith("b");
  expect(onToggle).not.toHaveBeenCalledWith("a");

  onToggle.mockClear();
  const rowBoxes = screen.getAllByLabelText("Select row");
  fireEvent.click(rowBoxes[1]);
  expect(onToggle).toHaveBeenCalledWith("b");
});
