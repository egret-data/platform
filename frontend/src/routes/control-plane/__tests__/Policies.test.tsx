import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import { setPolicyEnabled } from "@/api/policies";
import Policies from "../Policies";

vi.mock("@/api/policies", () => ({
  listProfiles: () => Promise.resolve([
    { id: 1, name: "trino", type: "trino", policy_version: 3, engines: [{ id: "e1", name: "prod", status: "running" }] },
  ]),
  listPolicies: () => Promise.resolve([
    { id: 42, name: "allow-analysts", service: "trino", policy_type: "access",
      resource_path: "iceberg.sales.orders.*", principals: ["analysts"], permissions: ["select"], is_enabled: true,
      description: "grants select to analysts", labels: ["pii"] },
  ]),
  createPolicy: vi.fn(() => Promise.resolve({ id: 99 })),
  updatePolicy: vi.fn(() => Promise.resolve({ id: 42 })),
  deletePolicy: vi.fn(() => Promise.resolve(undefined)),
  setPolicyEnabled: vi.fn(() => Promise.resolve()),
  getPolicy: vi.fn(() => Promise.resolve({
    name: "allow-analysts",
    isEnabled: true,
    resources: {
      catalog: { values: ["iceberg"] },
      schema: { values: ["sales"] },
      table: { values: ["orders"] },
      column: { values: ["*"] },
    },
    policyItems: [{ users: [], groups: ["analysts"], accesses: [{ type: "select" }] }],
    denyPolicyItems: [],
  })),
  getServiceDef: vi.fn(() => Promise.resolve({ accessTypes: [{ name: "select" }] })),
  listPrincipals: vi.fn(() => Promise.resolve([])),
  getTrinoConfig: vi.fn(() => Promise.resolve([
    { name: "access-control.properties", content: "access-control.name=ranger" },
    { name: "ranger-trino-security.xml", content: "<configuration/>" },
  ])),
  MASK_TYPES: [
    { name: "MASK_HASH", label: "Hash" },
    { name: "CUSTOM", label: "Custom expression" },
  ],
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

  it("opens the access policy editor on New policy", async () => {
    const user = userEvent.setup();
    wrap(<Policies />);
    await screen.findByText("allow-analysts");
    await user.click(await screen.findByRole("button", { name: /new policy/i }));
    expect(await screen.findByText(/new access policy/i)).toBeInTheDocument();
  });

  it("renders a Delete action for each access policy row", async () => {
    wrap(<Policies />);
    await screen.findByText("allow-analysts");
    expect(screen.getByRole("button", { name: /delete allow-analysts/i })).toBeInTheDocument();
  });

  it("opens the Trino config dialog on Download Trino config", async () => {
    const user = userEvent.setup();
    wrap(<Policies />);
    await screen.findByText("allow-analysts");
    await user.click(await screen.findByRole("button", { name: /download trino config/i }));
    expect(await screen.findByRole("heading", { name: /trino config/i })).toBeInTheDocument();
  });

  it("renders a label badge for the policy row", async () => {
    wrap(<Policies />);
    expect(await screen.findByText("allow-analysts")).toBeInTheDocument();
    expect(screen.getByText("pii")).toBeInTheDocument();
  });

  it("calls setPolicyEnabled when the enable toggle is clicked", async () => {
    const user = userEvent.setup();
    wrap(<Policies />);
    await screen.findByText("allow-analysts");
    const toggle = screen.getByRole("switch", { name: /toggle allow-analysts/i });
    await user.click(toggle);
    expect(setPolicyEnabled).toHaveBeenCalledWith(42, false);
  });

  it("shows New policy on the masking tab and opens the masking editor", async () => {
    const user = userEvent.setup();
    wrap(<Policies />);
    await screen.findByText("allow-analysts");
    await user.click(screen.getByRole("tab", { name: /masking/i }));
    const newButton = await screen.findByRole("button", { name: /new policy/i });
    expect(newButton).toBeInTheDocument();
    await user.click(newButton);
    expect(await screen.findByText(/new masking policy/i)).toBeInTheDocument();
  });
});
