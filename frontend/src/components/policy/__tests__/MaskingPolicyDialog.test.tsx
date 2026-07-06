import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import { MaskingPolicyDialog } from "../MaskingPolicyDialog";

const createPolicy = vi.fn((_i: unknown) => Promise.resolve({ id: 1 }));
vi.mock("@/api/policies", async (orig) => ({
  ...(await orig<typeof import("@/api/policies")>()),
  createPolicy: (i: unknown) => createPolicy(i),
  updatePolicy: vi.fn(() => Promise.resolve({ id: 1 })),
  listPrincipals: () => Promise.resolve([]),
}));

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

describe("MaskingPolicyDialog", () => {
  it("creates a masking policy", async () => {
    wrap(<MaskingPolicyDialog open service="trino" onOpenChange={() => {}} onSaved={() => {}} />);
    fireEvent.change(await screen.findByLabelText("Policy name"), { target: { value: "m1" } });
    fireEvent.change(screen.getByLabelText("Column"), { target: { value: "email" } });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    await waitFor(() => expect(createPolicy).toHaveBeenCalled());
    const arg = createPolicy.mock.calls[0][0] as { kind: string; resources: { column: { values: string[] } } };
    expect(arg.kind).toBe("masking");
    expect(arg.resources.column.values).toEqual(["email"]);
  });
});
