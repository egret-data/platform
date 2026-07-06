import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import { setPolicyEnabled } from "@/api/policies";
import Policies from "../Policies";

const navigate = vi.fn();
vi.mock("react-router-dom", async () => {
  const actual = await vi.importActual<typeof import("react-router-dom")>("react-router-dom");
  return { ...actual, useNavigate: () => navigate };
});

vi.mock("@/api/policies", () => ({
  listProfiles: () => Promise.resolve([
    { id: 1, name: "trino", type: "trino", policy_version: 3, engines: [{ id: "e1", name: "prod", status: "running" }] },
  ]),
  listPolicies: () => Promise.resolve([
    { id: 42, name: "allow-analysts", service: "trino", policy_type: "access",
      resource_path: "iceberg.sales.orders.*", principals: ["analysts"], permissions: ["select"], is_enabled: true,
      description: "grants select to analysts", labels: ["pii"] },
  ]),
  deletePolicy: vi.fn(() => Promise.resolve(undefined)),
  setPolicyEnabled: vi.fn(() => Promise.resolve()),
  getTrinoConfig: vi.fn(() => Promise.resolve([
    { name: "access-control.properties", content: "access-control.name=ranger" },
    { name: "ranger-trino-security.xml", content: "<configuration/>" },
  ])),
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

  it("navigates to the full-page editor on New policy", async () => {
    const user = userEvent.setup();
    wrap(<Policies />);
    await screen.findByText("allow-analysts");
    await user.click(await screen.findByRole("button", { name: /new policy/i }));
    expect(navigate).toHaveBeenCalledWith("/control-plane/policies/trino/access/new");
  });

  it("navigates to the full-page editor on Edit", async () => {
    const user = userEvent.setup();
    wrap(<Policies />);
    await screen.findByText("allow-analysts");
    await user.click(await screen.findByRole("button", { name: /edit allow-analysts/i }));
    expect(navigate).toHaveBeenCalledWith("/control-plane/policies/trino/access/42");
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

  it("navigates to the masking editor on the masking tab", async () => {
    const user = userEvent.setup();
    wrap(<Policies />);
    await screen.findByText("allow-analysts");
    await user.click(screen.getByRole("tab", { name: /masking/i }));
    const newButton = await screen.findByRole("button", { name: /new policy/i });
    expect(newButton).toBeInTheDocument();
    await user.click(newButton);
    expect(navigate).toHaveBeenCalledWith("/control-plane/policies/trino/masking/new");
  });
});
