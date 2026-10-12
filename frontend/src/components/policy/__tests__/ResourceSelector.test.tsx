import { render, screen, fireEvent } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import type { ServiceDefResource } from "@/api/policies";
import { ResourceSelector } from "../ResourceSelector";

vi.mock("@/api/policies", () => ({ resourceLookup: () => Promise.resolve([]) }));

function wrap(u: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{u}</QueryClientProvider>);
}

// A small slice of the Trino service-def: the catalog.schema.table.column
// hierarchy plus two standalone roots (role, trinouser).
const RESOURCE_DEFS: ServiceDefResource[] = [
  { name: "catalog", excludesSupported: true, mandatory: true, isValidLeaf: true },
  { name: "schema", parent: "catalog", level: 1, excludesSupported: true, mandatory: true, isValidLeaf: true },
  { name: "table", parent: "schema", level: 2, excludesSupported: true, mandatory: true, isValidLeaf: true },
  { name: "column", parent: "table", level: 3, excludesSupported: true, mandatory: false, isValidLeaf: true },
  { name: "role", excludesSupported: false, mandatory: true, isValidLeaf: true },
  { name: "trinouser", excludesSupported: false, mandatory: true, isValidLeaf: true },
];

describe("ResourceSelector", () => {
  it("renders the catalog level with its chip and a level-0 type selector offering the roots", () => {
    wrap(
      <ResourceSelector
        resourceDefs={RESOURCE_DEFS}
        service="trino"
        value={{ catalog: { values: ["iceberg"] } }}
        onChange={vi.fn()}
      />,
    );
    expect(screen.getByText("iceberg")).toBeInTheDocument();

    fireEvent.click(screen.getByLabelText("Resource type at level 1"));
    expect(screen.getByRole("option", { name: "Catalog" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Role" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Trino user" })).toBeInTheDocument();
  });

  it("renders a standalone root (role) chain and reports its leaf", () => {
    const onLeafChange = vi.fn();
    wrap(
      <ResourceSelector
        resourceDefs={RESOURCE_DEFS}
        service="trino"
        value={{ role: { values: ["*"] } }}
        onChange={vi.fn()}
        onLeafChange={onLeafChange}
      />,
    );
    expect(screen.getByText("*")).toBeInTheDocument();
    // A root with no children (role) offers no "add level" selector.
    expect(screen.queryByLabelText("Add resource level")).not.toBeInTheDocument();
    expect(onLeafChange).toHaveBeenCalledWith("role");
  });

  it("switching the level-0 type to Role truncates catalog and installs role", () => {
    const onChange = vi.fn();
    wrap(
      <ResourceSelector
        resourceDefs={RESOURCE_DEFS}
        service="trino"
        value={{ catalog: { values: ["iceberg"] } }}
        onChange={onChange}
      />,
    );
    fireEvent.click(screen.getByLabelText("Resource type at level 1"));
    fireEvent.click(screen.getByRole("option", { name: "Role" }));
    expect(onChange).toHaveBeenCalledWith({ role: { values: [] } });
  });

  it("offers an add-level selector at a leaf with children, defaulting to None (stop here)", () => {
    const onChange = vi.fn();
    wrap(
      <ResourceSelector
        resourceDefs={RESOURCE_DEFS}
        service="trino"
        value={{ catalog: { values: ["iceberg"] }, schema: { values: ["sales"] } }}
        onChange={onChange}
      />,
    );
    fireEvent.click(screen.getByLabelText("Add resource level"));
    fireEvent.click(screen.getByRole("option", { name: "Table" }));
    expect(onChange).toHaveBeenCalledWith({
      catalog: { values: ["iceberg"] },
      schema: { values: ["sales"] },
      table: { values: ["*"], isExcludes: false, isRecursive: false },
    });
  });

  it("offers '— none —' on a level whose parent isValidLeaf, truncating the chain there when picked", () => {
    const onChange = vi.fn();
    wrap(
      <ResourceSelector
        resourceDefs={RESOURCE_DEFS}
        service="trino"
        value={{
          catalog: { values: ["c"] },
          schema: { values: ["*"] },
          table: { values: ["*"] },
          column: { values: ["*"] },
        }}
        onChange={onChange}
      />,
    );

    // Column's parent (table) is a valid leaf, so Column's Type select offers "— none —".
    fireEvent.click(screen.getByLabelText("Resource type at level 4"));
    const noneOption = screen.getByRole("option", { name: "— none —" });
    expect(noneOption).toBeInTheDocument();
    fireEvent.click(noneOption);
    expect(onChange).toHaveBeenCalledWith({
      catalog: { values: ["c"] },
      schema: { values: ["*"] },
      table: { values: ["*"] },
    });
  });

  it("never offers '— none —' on the root level (a policy needs at least one resource)", () => {
    wrap(
      <ResourceSelector
        resourceDefs={RESOURCE_DEFS}
        service="trino"
        value={{ catalog: { values: ["c"] } }}
        onChange={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByLabelText("Resource type at level 1"));
    expect(screen.queryByRole("option", { name: "— none —" })).not.toBeInTheDocument();
  });
});

describe("ResourceSelector — absent levels (SP4d-4 B1)", () => {
  const star = { values: ["*"] };

  it("says an EMPTY non-root level is saved as *, and says nothing for a * level", () => {
    wrap(
      <ResourceSelector
        resourceDefs={RESOURCE_DEFS}
        service="trino"
        value={{ catalog: { values: ["iceberg"] }, schema: { values: [] }, table: star }}
        onChange={vi.fn()}
      />,
    );
    const hint = screen.getByText(/Empty means every schema — saved as \*\./);
    expect(screen.getByLabelText("Schema value").getAttribute("aria-describedby")).toBe(hint.id);
    // The control: the table level holds `*` and gets no hint.
    expect(screen.queryByText(/Empty means every table/)).not.toBeInTheDocument();
    expect(screen.getByLabelText("Table value")).not.toHaveAttribute("aria-describedby");
  });

  it("states that an empty column grants column SELECT when select is granted, not otherwise", () => {
    const value = { catalog: { values: ["iceberg"] }, schema: star, table: star, column: { values: [] } };
    const { unmount } = wrap(
      <ResourceSelector resourceDefs={RESOURCE_DEFS} service="trino" value={value}
        onChange={vi.fn()} grantedAccessTypes={["select"]} />,
    );
    expect(screen.getByText(/Column SELECT is then granted on every column/)).toBeInTheDocument();
    unmount();
    wrap(
      <ResourceSelector resourceDefs={RESOURCE_DEFS} service="trino" value={value}
        onChange={vi.fn()} grantedAccessTypes={["insert"]} />,
    );
    expect(screen.getByText(/Empty means every column — saved as \*/)).toBeInTheDocument();
    expect(screen.queryByText(/Column SELECT is then granted/)).not.toBeInTheDocument();
  });

  it("an EMPTY root is invalid and says why, rather than being saved as *", () => {
    wrap(
      <ResourceSelector resourceDefs={RESOURCE_DEFS} service="trino"
        value={{ catalog: { values: [] }, schema: star }} onChange={vi.fn()} />,
    );
    expect(
      screen.getByText("Name a catalog. It is required: * here would cover every catalog this profile serves."),
    ).toBeInTheDocument();
    // The root never gets the "saved as *" hint.
    expect(screen.queryByText(/Empty means every catalog/)).not.toBeInTheDocument();
  });

  it("a named catalog has no required message", () => {
    wrap(
      <ResourceSelector resourceDefs={RESOURCE_DEFS} service="trino"
        value={{ catalog: { values: ["iceberg"] } }} onChange={vi.fn()} />,
    );
    expect(screen.queryByText(/Name a catalog/)).not.toBeInTheDocument();
  });

  it("explains a gap: a named level under an absent one", () => {
    wrap(
      <ResourceSelector resourceDefs={RESOURCE_DEFS} service="trino"
        value={{ catalog: { values: ["iceberg"] }, table: { values: ["orders"] } }} onChange={vi.fn()} />,
    );
    expect(screen.getByRole("alert")).toHaveTextContent(
      "Table is set but schema is not, so Ranger cannot match this policy",
    );
  });

  it("shows no gap message for a complete chain", () => {
    wrap(
      <ResourceSelector resourceDefs={RESOURCE_DEFS} service="trino"
        value={{ catalog: { values: ["iceberg"] }, schema: star, table: { values: ["orders"] } }} onChange={vi.fn()} />,
    );
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  const AT_TABLE = { catalog: { values: ["iceberg"] }, schema: star, table: star };
  const STOPS = /Stops above column: grants no column reads, only table-level checks such as/;

  it("warns that a select chain stopping at table grants no column reads", () => {
    wrap(
      <ResourceSelector resourceDefs={RESOURCE_DEFS} service="trino" value={AT_TABLE}
        onChange={vi.fn()} grantedAccessTypes={["select", "insert"]} />,
    );
    expect(screen.getByText(STOPS)).toBeInTheDocument();
  });

  it("warns for `all`, which implies select", () => {
    wrap(
      <ResourceSelector resourceDefs={RESOURCE_DEFS} service="trino" value={AT_TABLE}
        onChange={vi.fn()} grantedAccessTypes={["all"]} />,
    );
    expect(screen.getByText(STOPS)).toBeInTheDocument();
  });

  it("does not warn for an insert-only chain stopping at table", () => {
    wrap(
      <ResourceSelector resourceDefs={RESOURCE_DEFS} service="trino" value={AT_TABLE}
        onChange={vi.fn()} grantedAccessTypes={["insert"]} />,
    );
    expect(screen.queryByText(STOPS)).not.toBeInTheDocument();
  });

  it("does not warn when access types are not supplied (masking, row filter)", () => {
    wrap(<ResourceSelector resourceDefs={RESOURCE_DEFS} service="trino" value={AT_TABLE} onChange={vi.fn()} />);
    expect(screen.queryByText(STOPS)).not.toBeInTheDocument();
  });

  it("does not warn once the chain reaches column, or for a role chain", () => {
    const { unmount } = wrap(
      <ResourceSelector resourceDefs={RESOURCE_DEFS} service="trino"
        value={{ ...AT_TABLE, column: star }} onChange={vi.fn()} grantedAccessTypes={["select"]} />,
    );
    expect(screen.queryByText(STOPS)).not.toBeInTheDocument();
    unmount();
    wrap(
      <ResourceSelector resourceDefs={RESOURCE_DEFS} service="trino"
        value={{ role: star }} onChange={vi.fn()} grantedAccessTypes={["select"]} />,
    );
    expect(screen.queryByText(STOPS)).not.toBeInTheDocument();
  });
});

