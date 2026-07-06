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
}));

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

describe("AccessPolicyDialog", () => {
  it("creates a policy from the form", async () => {
    wrap(<AccessPolicyDialog open service="trino" onOpenChange={() => {}} onSaved={() => {}} />);
    fireEvent.change(await screen.findByLabelText("Policy name"), { target: { value: "p1" } });
    fireEvent.change(screen.getByLabelText("Catalog"), { target: { value: "iceberg" } });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    await waitFor(() => expect(createPolicy).toHaveBeenCalled());
    const arg = createPolicy.mock.calls[0][0] as { name: string; resources: { catalog: { values: string[] } } };
    expect(arg.name).toBe("p1");
    expect(arg.resources.catalog.values).toEqual(["iceberg"]);
  });
});
