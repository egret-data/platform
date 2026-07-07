import { render, screen, fireEvent } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import type { AccessItem } from "@/api/policies";
import { PermissionTable } from "../PermissionTable";

vi.mock("@/api/policies", async () => {
  const actual = await vi.importActual<typeof import("@/api/policies")>("@/api/policies");
  return { ...actual, listPrincipals: vi.fn().mockResolvedValue([]) };
});

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

const baseItem: AccessItem = {
  users: [],
  groups: ["analysts"],
  roles: [],
  accesses: ["select"],
  effect: "allow",
  delegateAdmin: false,
};

describe("PermissionTable", () => {
  it("renders the column headers", () => {
    wrap(<PermissionTable items={[baseItem]} onChange={vi.fn()} accessTypes={["select", "insert"]} />);
    expect(screen.getByText("Roles")).toBeInTheDocument();
    expect(screen.getByText("Groups")).toBeInTheDocument();
    expect(screen.getByText("Users")).toBeInTheDocument();
    expect(screen.getByText("Permissions")).toBeInTheDocument();
    expect(screen.getByText("Delegate admin")).toBeInTheDocument();
  });

  it("renders a row's group chip", () => {
    wrap(<PermissionTable items={[baseItem]} onChange={vi.fn()} accessTypes={["select", "insert"]} />);
    expect(screen.getByText("analysts")).toBeInTheDocument();
  });

  it("appends a blank row on Add row", () => {
    const onChange = vi.fn();
    wrap(<PermissionTable items={[baseItem]} onChange={onChange} accessTypes={["select", "insert"]} />);
    fireEvent.click(screen.getByText("Add row"));
    expect(onChange).toHaveBeenCalledWith([
      baseItem,
      { users: [], groups: [], roles: [], accesses: [], effect: "allow", delegateAdmin: false },
    ]);
  });

  it("removes a row on remove click", () => {
    const onChange = vi.fn();
    wrap(<PermissionTable items={[baseItem]} onChange={onChange} accessTypes={["select", "insert"]} />);
    fireEvent.click(screen.getByLabelText("remove row 0"));
    expect(onChange).toHaveBeenCalledWith([]);
  });
});
