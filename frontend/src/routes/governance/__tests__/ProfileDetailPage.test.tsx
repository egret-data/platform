import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, it, expect, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { setPolicyEnabled, seedDefaultPolicies } from "@/api/policies";
import { ApiError } from "@/api/client";
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

// A profile whose origin flips per test (default: "manual") — set by each
// describe block below rather than hardcoded, since D10's whole point is
// that the SAME table behaves differently only for the compiler-owned ROW,
// never for every row just because the profile is generated.
let profileOrigin: string = "manual";

vi.mock("@/api/policies", async (orig) => ({
  ...(await orig<typeof import("@/api/policies")>()),
  listProfiles: () => Promise.resolve([
    {
      id: 1,
      name: "trino-prod",
      type: "trino",
      policy_version: 3,
      engines: [{ id: "e1", name: "prod", status: "running" }],
      origin: profileOrigin,
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
    {
      id: 43,
      name: "perm-grant-abc123",
      service: "trino-prod",
      policy_type: "access",
      resource_path: "iceberg.sales.*",
      principals: ["role:analyst"],
      permissions: ["select"],
      is_enabled: true,
      description: "compiled from a grant",
      labels: [],
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
  beforeEach(() => {
    profileOrigin = "manual";
  });

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

describe("ProfileDetailPage — D10, the guard is on the policy, not the profile", () => {
  it("does not mark a manual profile as generated", async () => {
    profileOrigin = "manual";
    wrap(<ProfileDetailPage />);
    await screen.findByText("allow-analysts");
    expect(screen.queryByText("Generated")).not.toBeInTheDocument();
  });

  it("marks a generated profile", async () => {
    profileOrigin = "generated";
    wrap(<ProfileDetailPage />);
    expect(await screen.findByText("Generated")).toBeInTheDocument();
  });

  it("does not offer a compiler-owned policy for edit inside a generated profile", async () => {
    profileOrigin = "generated";
    wrap(<ProfileDetailPage />);
    await screen.findByText("perm-grant-abc123");
    // Not a link — the compiler owns it, so it is not "offered for edit".
    expect(
      screen.queryByRole("link", { name: "perm-grant-abc123" }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete perm-grant-abc123" })).toBeDisabled();
  });

  // THE DISCRIMINATING CASE: a foreign (non perm-grant-/perm-rule-) policy
  // inside the SAME generated profile stays fully editable. A test that only
  // checks the "Generated" badge renders would pass even if the guard were
  // wrongly scoped to the whole profile (the one-way door SP4a had to
  // reopen) — this is the assertion that would catch that regression.
  it("still offers a non-compiler-owned policy for edit inside a generated profile", async () => {
    profileOrigin = "generated";
    wrap(<ProfileDetailPage />);
    const link = await screen.findByRole("link", { name: "allow-analysts" });
    expect(link).toHaveAttribute(
      "href",
      "/governance/policies/trino-prod/access/42",
    );
    expect(screen.getByRole("button", { name: "Delete allow-analysts" })).not.toBeDisabled();
  });

  it("renders the 409 conflict explanation, not a bare error, on a refused write", async () => {
    profileOrigin = "generated";
    // Driven on "allow-analysts" (the FE believes it foreign, so its Switch
    // is enabled) rather than the compiler-owned row, whose controls are
    // already disabled — the UI's own preemptive disablement must not be
    // the only thing standing between an operator and a bare error if the
    // server ever refuses a write this FE-side check did not anticipate
    // (e.g. the policy was claimed by the compiler after this page loaded).
    vi.mocked(setPolicyEnabled).mockRejectedValueOnce(
      new ApiError(
        409,
        'conflict: policy "allow-analysts" is compiled by the SP4a permission model — edit or delete that grant/rule instead of writing this policy directly',
      ),
    );
    const user = userEvent.setup();
    wrap(<ProfileDetailPage />);
    await screen.findByText("allow-analysts");
    await user.click(screen.getByRole("switch", { name: /toggle allow-analysts/i }));

    expect(await screen.findByText(/is compiled by the SP4a permission model/)).toBeInTheDocument();
    expect(screen.queryByText(/^conflict:/)).not.toBeInTheDocument();
  });
});
