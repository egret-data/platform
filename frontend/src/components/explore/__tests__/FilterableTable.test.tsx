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

it("hides its own search box when the page owns the search", () => {
  const { rerender } = render(
    <FilterableTable rows={rows} columns={columns} getId={(r) => r.id} />,
  );
  expect(screen.getByPlaceholderText("Search…")).toBeInTheDocument();

  rerender(<FilterableTable rows={rows} columns={columns} getId={(r) => r.id} hideSearch />);
  expect(screen.queryByPlaceholderText("Search…")).toBeNull();
  // The rows themselves are untouched — only the box is gone.
  expect(screen.getByText("Alice")).toBeInTheDocument();
  expect(screen.getByText("Bob")).toBeInTheDocument();
});

it("still renders external filters when its own search box is hidden", () => {
  render(
    <FilterableTable
      rows={rows}
      columns={columns}
      getId={(r) => r.id}
      hideSearch
      filters={<button type="button">State</button>}
    />,
  );
  expect(screen.getByRole("button", { name: "State" })).toBeInTheDocument();
});

/**
 * "Select all" must act on the rows the table DREW, not on everything that
 * survived its search.
 *
 * Before this was fixed, `UsersPage` (`pageSize={25}`) let one click on a
 * 25-row table select every filtered user across every page — and its bulk
 * Disable then disabled all of them. There is no folder to collapse and no
 * deliberate act involved: pagination is the default state, so the gap between
 * what was drawn and what was selected opened on its own.
 *
 * The fixture is 5 rows over 2 pages, so a correct implementation and one that
 * selects nothing are distinguishable — both would leave rows 2-4 unselected,
 * but only the correct one selects rows 0-1.
 */
it("selects only the page it drew, not every row the search left", () => {
  const many: Row[] = Array.from({ length: 5 }, (_, i) => ({
    id: `r${i}`,
    name: `Row ${i}`,
  }));
  const onToggle = vi.fn();
  render(
    <FilterableTable
      rows={many}
      columns={columns}
      getId={(r) => r.id}
      pageSize={2}
      selection={{ selected: new Set<string>(), onToggle }}
    />,
  );

  // The table drew exactly two rows; the other three are on later pages.
  expect(screen.getByText("Row 0")).toBeInTheDocument();
  expect(screen.getByText("Row 1")).toBeInTheDocument();
  expect(screen.queryByText("Row 2")).toBeNull();

  fireEvent.click(screen.getByLabelText("Select all"));

  // Asserting the ids, not the call count: a header that toggled the whole
  // filtered set would also "have been called".
  expect(onToggle).toHaveBeenCalledWith("r0");
  expect(onToggle).toHaveBeenCalledWith("r1");
  expect(onToggle).not.toHaveBeenCalledWith("r2");
  expect(onToggle).not.toHaveBeenCalledWith("r3");
  expect(onToggle).not.toHaveBeenCalledWith("r4");
  expect(onToggle).toHaveBeenCalledTimes(2);
});

it("its checked state follows the drawn page, not the whole result set", () => {
  const many: Row[] = Array.from({ length: 5 }, (_, i) => ({
    id: `r${i}`,
    name: `Row ${i}`,
  }));
  render(
    <FilterableTable
      rows={many}
      columns={columns}
      getId={(r) => r.id}
      pageSize={2}
      // Exactly the drawn page is selected, and nothing else is.
      selection={{ selected: new Set(["r0", "r1"]), onToggle: vi.fn() }}
    />,
  );
  // Ticked, even though r2-r4 are unselected: they were not drawn.
  expect(screen.getByLabelText("Select all")).toBeChecked();
});

/**
 * The table owns the search box and the pager, so `rows` going in is a superset
 * of what comes out on screen. A consumer cannot compute the difference, which
 * is why the table has to report it — `UsersPage` and `MembersPanel` derived
 * their bulk targets from their own list and so acted on rows the user had
 * searched away, with Disable and Remove-from-workspace as the actions.
 *
 * Five rows over two pages: "reported the page it drew" and "reported every row
 * the search left" are then different lists. With a single page they are the
 * same list and this test would pass either way.
 */
it("reports the ids it drew, and re-reports them when the page changes", async () => {
  const many: Row[] = Array.from({ length: 5 }, (_, i) => ({ id: `r${i}`, name: `Row ${i}` }));
  const onDrawnChange = vi.fn();
  render(
    <FilterableTable
      rows={many}
      columns={columns}
      getId={(r) => r.id}
      pageSize={2}
      selection={{ selected: new Set<string>(), onToggle: vi.fn() }}
      onDrawnChange={onDrawnChange}
    />,
  );

  // The drawn page, not the five rows the table was handed.
  expect(onDrawnChange).toHaveBeenLastCalledWith(["r0", "r1"]);

  fireEvent.click(screen.getByRole("button", { name: /next/i }));
  expect(onDrawnChange).toHaveBeenLastCalledWith(["r2", "r3"]);
});

it("re-reports when a search narrows what is on screen", async () => {
  const rowsIn: Row[] = [
    { id: "a", name: "Alice" },
    { id: "b", name: "Bob" },
  ];
  const onDrawnChange = vi.fn();
  render(
    <FilterableTable
      rows={rowsIn}
      columns={columns}
      getId={(r) => r.id}
      selection={{ selected: new Set(["a", "b"]), onToggle: vi.fn() }}
      onDrawnChange={onDrawnChange}
    />,
  );
  expect(onDrawnChange).toHaveBeenLastCalledWith(["a", "b"]);

  // Searching Bob away must drop him from the report — this is the exact moment
  // a stale selection would otherwise keep him in a bulk action.
  fireEvent.change(screen.getByPlaceholderText(/search/i), { target: { value: "Alice" } });
  expect(onDrawnChange).toHaveBeenLastCalledWith(["a"]);
});

it("draws a refused row's checkbox disabled and leaves it out of Select all", () => {
  const onToggle = vi.fn();
  render(
    <FilterableTable
      rows={rows}
      columns={columns}
      getId={(r) => r.id}
      selection={{
        selected: new Set<string>(),
        onToggle,
        canSelect: (r) => r.id !== "a",
        rowLabel: (r) => `Select ${r.name}`,
      }}
    />,
  );
  // The refused row is drawn — disabled, not hidden — under its own name.
  expect(screen.getByRole("checkbox", { name: "Select Alice" })).toBeDisabled();
  expect(screen.getByRole("checkbox", { name: "Select Bob" })).toBeEnabled();

  fireEvent.click(screen.getByLabelText("Select all"));
  expect(onToggle).toHaveBeenCalledWith("b");
  expect(onToggle).not.toHaveBeenCalledWith("a");
});

it("disables Select all when no drawn row can be selected", () => {
  render(
    <FilterableTable
      rows={rows}
      columns={columns}
      getId={(r) => r.id}
      selection={{ selected: new Set<string>(), onToggle: vi.fn(), canSelect: () => false }}
    />,
  );
  expect(screen.getByRole("checkbox", { name: "Select all" })).toBeDisabled();
});
