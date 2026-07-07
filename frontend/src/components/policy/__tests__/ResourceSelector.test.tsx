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
      table: { values: [] },
    });
  });
});
