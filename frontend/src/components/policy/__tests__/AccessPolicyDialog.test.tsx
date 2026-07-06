import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import { AccessPolicyDialog } from "../AccessPolicyDialog";

const createPolicy = vi.fn((_input: unknown) => Promise.resolve({ id: 1 }));
vi.mock("@/api/policies", () => ({
  createPolicy: (a: unknown) => createPolicy(a),
  updatePolicy: vi.fn(() => Promise.resolve({ id: 1 })),
  getServiceDef: () => Promise.resolve({ accessTypes: [{ name: "select" }, { name: "insert" }] }),
  listPrincipals: () => Promise.resolve([]),
  resourceLookup: () => Promise.resolve([]),
}));

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

describe("AccessPolicyDialog", () => {
  it("creates a policy from the form", async () => {
    wrap(<AccessPolicyDialog open service="trino" onOpenChange={() => {}} onSaved={() => {}} />);
    fireEvent.change(await screen.findByLabelText("Policy name"), { target: { value: "p1" } });

    // The four ResourceLevelEditors share the same placeholder; catalog is first in DOM order.
    const catalogInput = screen.getAllByPlaceholderText("value or * (Enter to add)")[0];
    fireEvent.change(catalogInput, { target: { value: "iceberg" } });
    fireEvent.keyDown(catalogInput, { key: "Enter" });

    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    await waitFor(() => expect(createPolicy).toHaveBeenCalled());
    const arg = createPolicy.mock.calls[0][0] as {
      kind: string; name: string; resources: { catalog: { values: string[] } };
    };
    expect(arg.kind).toBe("access");
    expect(arg.name).toBe("p1");
    expect(arg.resources.catalog.values).toEqual(["iceberg"]);
  });

  it("hides the Deny tab when 'Deny all else' is enabled", async () => {
    wrap(<AccessPolicyDialog open service="trino" onOpenChange={() => {}} onSaved={() => {}} />);
    await screen.findByLabelText("Policy name");

    expect(screen.getByRole("tab", { name: "Deny" })).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("Deny all else"));
    expect(screen.queryByRole("tab", { name: "Deny" })).not.toBeInTheDocument();
  });

  it("posts an added Allow exception in the saved payload", async () => {
    wrap(<AccessPolicyDialog open service="trino" onOpenChange={() => {}} onSaved={() => {}} />);
    fireEvent.change(await screen.findByLabelText("Policy name"), { target: { value: "p2" } });

    fireEvent.click(screen.getByRole("button", { name: "Exceptions" }));
    fireEvent.click(await screen.findByRole("button", { name: /add exception/i }));

    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    await waitFor(() => expect(createPolicy).toHaveBeenCalled());
    const arg = createPolicy.mock.calls[createPolicy.mock.calls.length - 1][0] as {
      allowExceptions: unknown[];
    };
    expect(arg.allowExceptions.length).toBe(1);
  });
});
