import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { describe, it, expect, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { setPolicyEnabled, seedDefaultPolicies } from "@/api/policies";
import ProfileDetailPage from "../ProfileDetailPage";

const navigate = vi.fn();
vi.mock("react-router-dom", async () => {
  const actual = await vi.importActual<typeof import("react-router-dom")>("react-router-dom");
  return {
    ...actual,
    useParams: () => ({ service: "trino-prod" }),
    useNavigate: () => navigate,
  };
});

vi.mock("@/api/policies", () => ({
  listProfiles: () => Promise.resolve([
    {
      id: 1,
      name: "trino-prod",
      type: "trino",
      policy_version: 3,
      engines: [{ id: "e1", name: "prod", status: "running" }],
    },
  ]),
  listPolicies: () => Promise.resolve([
    {
      id: 42,
      name: "allow-analysts",
      service: "trino-prod",
      policy_type: "access",
      resource_path: "iceberg.sales.orders.*",
      principals: ["analysts"],
      permissions: ["select"],
      is_enabled: true,
      description: "grants select to analysts",
      labels: ["pii"],
    },
  ]),
  deletePolicy: vi.fn(() => Promise.resolve(undefined)),
  setPolicyEnabled: vi.fn(() => Promise.resolve()),
  getTrinoConfig: vi.fn(() => Promise.resolve([
    { name: "access-control.properties", content: "access-control.name=ranger" },
  ])),
  seedDefaultPolicies: vi.fn(() => Promise.resolve({ created: 13 })),
}));

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter>{ui}</MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

describe("ProfileDetailPage", () => {
  it("renders the profile name and a policy row", async () => {
    wrap(<ProfileDetailPage />);
    expect(await screen.findByText("trino-prod")).toBeInTheDocument();
    expect(await screen.findByText("allow-analysts")).toBeInTheDocument();
    expect(screen.getByText("iceberg.sales.orders.*")).toBeInTheDocument();
  });

  it("navigates to the new-policy editor on New policy", async () => {
    const user = userEvent.setup();
    wrap(<ProfileDetailPage />);
    await screen.findByText("allow-analysts");
    await user.click(await screen.findByRole("button", { name: /new policy/i }));
    expect(navigate).toHaveBeenCalledWith("/governance/policies/trino-prod/access/new");
  });

  it("links back to the Governance policies landing", async () => {
    wrap(<ProfileDetailPage />);
    await screen.findByText("allow-analysts");
    expect(screen.getByRole("link", { name: "Policies" })).toHaveAttribute("href", "/governance/policies");
  });

  it("calls setPolicyEnabled when the enable toggle is clicked", async () => {
    const user = userEvent.setup();
    wrap(<ProfileDetailPage />);
    await screen.findByText("allow-analysts");
    const toggle = screen.getByRole("switch", { name: /toggle allow-analysts/i });
    await user.click(toggle);
    expect(setPolicyEnabled).toHaveBeenCalledWith(42, false);
  });

  it("seeds default policies when the header button is clicked", async () => {
    const user = userEvent.setup();
    wrap(<ProfileDetailPage />);
    await screen.findByText("allow-analysts");
    await user.click(screen.getByRole("button", { name: /seed default policies/i }));
    expect(seedDefaultPolicies).toHaveBeenCalledWith("trino-prod");
  });
});
