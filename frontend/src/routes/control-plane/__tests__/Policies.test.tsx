import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import Policies from "../Policies";

vi.mock("@/api/policies", () => ({
  listProfiles: () => Promise.resolve([
    { id: 1, name: "trino", type: "trino", policy_version: 3, engines: [{ id: "e1", name: "prod", status: "running" }] },
  ]),
  listPolicies: () => Promise.resolve([
    { id: 42, name: "allow-analysts", service: "trino", policy_type: "access",
      resource_path: "iceberg.sales.orders.*", principals: ["analysts"], permissions: ["select"], is_enabled: true },
  ]),
}));

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

describe("Policies page", () => {
  it("renders the profile and a policy row", async () => {
    wrap(<Policies />);
    expect(await screen.findByText("allow-analysts")).toBeInTheDocument();
    expect(await screen.findByText("iceberg.sales.orders.*")).toBeInTheDocument();
  });
});
