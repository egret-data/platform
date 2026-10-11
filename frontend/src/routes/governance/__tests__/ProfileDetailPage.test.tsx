import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, it, expect, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";
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
/** The TAG service's own origin, which is what the Tags tab's guard must read
 *  (review M-5). Independent of `profileOrigin` on purpose: the two services
 *  are two `authz_services` rows and the compiler can have written to one and
 *  not the other. */
let tagServiceOrigin: string = "manual";

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
      tag_service: "trino-prod_tag",
    },
    // The tag service is its own row in `policy.authz_services`, and
    // `list_profiles` returns every service — which is how the FE resolves its
    // `origin` without a second request.
    {
      id: 2,
      name: "trino-prod_tag",
      type: "tag",
      policy_version: 1,
      engines: [],
      origin: tagServiceOrigin,
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

const ADMIN: Me = {
  user_id: "u1",
  email: "a@example.com",
  platform_roles: ["platform_admin"],
  permissions: [],
};

/** A reader: the `policy:read`-only persona SP4b-2 finding 3 is about. */
const READER: Me = {
  user_id: "u2",
  email: "r@example.com",
  platform_roles: [],
  permissions: ["policy:read"],
};

function wrap(ui: React.ReactNode, me: Me | null = ADMIN) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <TooltipProvider>
          <MemoryRouter>{ui}</MemoryRouter>
        </TooltipProvider>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

describe("ProfileDetailPage", () => {
  beforeEach(() => {
    profileOrigin = "manual";
    tagServiceOrigin = "manual";
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

  it("links to the profile's migrate page, for a policy:read account too", async () => {
    wrap(<ProfileDetailPage />, READER);
    await screen.findByText("allow-analysts");
    expect(screen.getByRole("link", { name: "Migrate policies" })).toHaveAttribute(
      "href",
      "/governance/policies/trino-prod/migrate",
    );
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

  /**
   * **THE OTHER HALF OF THE CONJUNCTION** (SP4b-2 review M-3: a mutation that
   * dropped `profileOrigin === "generated"` from `compilerOwned` killed
   * nothing, so this case was untested).
   *
   * A `perm-grant-`-named policy in a **manual** profile stays fully editable,
   * because the backend permits exactly that: `refuse_if_compiler_owned`
   * (`crates/egret-policy/src/store.rs`) returns `Ok` unless the profile's own
   * `origin` is `'generated'` — a policy hand-named with the prefix in a
   * profile no compile has ever swept is not at risk of being overwritten, and
   * freezing it would be the same one-way door SP4a had to reopen. Marking it
   * read-only here would make the UI refuse a write the API accepts, which is
   * the opposite failure from the one the `Generated` badge exists to prevent.
   */
  it("leaves a perm-grant- named policy editable in a MANUAL profile", async () => {
    profileOrigin = "manual";
    wrap(<ProfileDetailPage />);

    const link = await screen.findByRole("link", { name: "perm-grant-abc123" });
    expect(link).toHaveAttribute("href", "/governance/policies/trino-prod/access/43");
    expect(screen.getByRole("button", { name: "Delete perm-grant-abc123" })).not.toBeDisabled();
    expect(screen.getByRole("switch", { name: /toggle perm-grant-abc123/i })).toBeEnabled();
    // …and the same row IS frozen once the profile is generated, which is the
    // assertion `does not offer a compiler-owned policy for edit inside a
    // generated profile` above makes. Both halves, or the conjunction is
    // half-tested.
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

describe("ProfileDetailPage — a write this account cannot make is not offered (SP4b-2 finding 3)", () => {
  beforeEach(() => {
    profileOrigin = "manual";
    tagServiceOrigin = "manual";
  });

  /**
   * **THE DISCRIMINATING CASE.** Every other test in this file renders as a
   * `platform_admin`, who bypasses the PDP and so passes all four keys — which
   * is why this page shipped offering a reader `New policy`, `Seed default
   * policies`, `Delete` and an enabled `Enabled` switch, and answering the
   * refused toggle with the single word `forbidden`. This renders the same page
   * for the persona the browser run used.
   */
  it("offers a policy:read-only account no create, seed or delete, and a disabled toggle", async () => {
    wrap(<ProfileDetailPage />, READER);
    await screen.findByText("allow-analysts");

    expect(screen.queryByRole("button", { name: /new policy/i })).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /seed default policies/i }),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete allow-analysts" })).not.toBeInTheDocument();
    expect(screen.getByRole("switch", { name: /toggle allow-analysts/i })).toBeDisabled();
  });

  /** The same page for an admin still offers all four — a fix that hid them
   *  from everybody would pass the test above and break the page. */
  it("still offers all of them to a platform_admin", async () => {
    wrap(<ProfileDetailPage />);
    await screen.findByText("allow-analysts");

    expect(screen.getByRole("button", { name: /new policy/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /seed default policies/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete allow-analysts" })).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: /toggle allow-analysts/i })).toBeEnabled();
  });

  /**
   * Hiding the control is not the whole fix: a permission snapshot can be
   * stale, or the server can refuse for a reason this client did not predict.
   * When a 403 does come back it must name the permission — the server's own
   * body is the bare word `forbidden`, which tells an operator nothing about
   * what to ask for.
   */
  it("names the permission a refused toggle needs, instead of echoing 'forbidden'", async () => {
    vi.mocked(setPolicyEnabled).mockRejectedValueOnce(new ApiError(403, "forbidden"));
    const user = userEvent.setup();
    wrap(<ProfileDetailPage />);
    await screen.findByText("allow-analysts");
    await user.click(screen.getByRole("switch", { name: /toggle allow-analysts/i }));

    expect(await screen.findByText(/needs the policy:update permission/)).toBeInTheDocument();
    expect(screen.queryByText(/^forbidden$/)).not.toBeInTheDocument();
  });
});

describe("ProfileDetailPage — the Tags tab guards on the TAG service's origin (review M-5)", () => {
  beforeEach(() => {
    profileOrigin = "manual";
    tagServiceOrigin = "manual";
  });

  /**
   * **THE DISCRIMINATING CASE.** A compiled tag policy is stored under the tag
   * service's own `service_id`, so `refuse_if_compiler_owned` reads THAT
   * service's origin. The Tags tab passed no origin at all, so a
   * compiler-owned tag policy was offered for edit and the operator discovered
   * the 409 by hitting it. Note the main profile is `manual` here: a fix that
   * reused `profileOrigin` would mark nothing and pass nothing.
   */
  it("freezes a compiler-owned tag policy when the TAG service is generated", async () => {
    profileOrigin = "manual";
    tagServiceOrigin = "generated";
    const user = userEvent.setup();
    wrap(<ProfileDetailPage />);
    await screen.findByText("allow-analysts");

    await user.click(screen.getByRole("tab", { name: "Tags" }));
    await screen.findByRole("heading", { name: /Tag policies/ });

    expect(screen.queryByRole("link", { name: "perm-grant-abc123" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete perm-grant-abc123" })).toBeDisabled();
    expect(screen.getByRole("switch", { name: /toggle perm-grant-abc123/i })).toBeDisabled();

    // …and the foreign policy in that same generated tag service stays
    // editable, which is the distinction SP4a moved to the policy.
    expect(screen.getByRole("link", { name: "allow-analysts" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete allow-analysts" })).not.toBeDisabled();
  });

  /** And a MANUAL tag service leaves even a `perm-grant-`-named row editable,
   *  exactly as the backend does — the same conjunction as the Access tab. */
  it("leaves a compiler-named tag policy editable when the tag service is manual", async () => {
    profileOrigin = "generated";
    tagServiceOrigin = "manual";
    const user = userEvent.setup();
    wrap(<ProfileDetailPage />);
    await screen.findByText("allow-analysts");

    await user.click(screen.getByRole("tab", { name: "Tags" }));
    await screen.findByRole("heading", { name: /Tag policies/ });

    expect(screen.getByRole("link", { name: "perm-grant-abc123" })).toBeInTheDocument();
  });
});
