import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import { RowFilterPolicyDialog } from "../RowFilterPolicyDialog";

const createPolicy = vi.fn((_i: unknown) => Promise.resolve({ id: 1 }));
vi.mock("@/api/policies", async (orig) => ({
  ...(await orig<typeof import("@/api/policies")>()),
  createPolicy: (i: unknown) => createPolicy(i),
  updatePolicy: vi.fn(() => Promise.resolve({ id: 1 })),
  listPrincipals: () => Promise.resolve([]),
  resourceLookup: () => Promise.resolve([]),
}));

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

describe("RowFilterPolicyDialog", () => {
  it("creates a row-filter policy", async () => {
    wrap(<RowFilterPolicyDialog open service="trino" onOpenChange={() => {}} onSaved={() => {}} />);
    fireEvent.change(await screen.findByLabelText("Policy name"), { target: { value: "rf1" } });
    fireEvent.change(screen.getByLabelText("filter 0"), { target: { value: "country = 'US'" } });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    await waitFor(() => expect(createPolicy).toHaveBeenCalled());
    const arg = createPolicy.mock.calls[0][0] as { kind: string; items: { filterExpr: string }[] };
    expect(arg.kind).toBe("rowfilter");
    expect(arg.items[0].filterExpr).toBe("country = 'US'");
  });
});
