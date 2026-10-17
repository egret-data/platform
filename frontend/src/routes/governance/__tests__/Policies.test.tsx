import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, describe, it, expect, vi } from "vitest";
import Policies from "../Policies";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";
import { ApiError } from "@/api/client";
import type { StoredOverlap } from "@/api/generated/model";

const createProfile = vi.fn((_name: string, _type: string) =>
  Promise.resolve({ id: 3, name: "sr", type: "starrocks" }),
);
const deleteProfile = vi.fn((_name: string) => Promise.resolve());

vi.mock("@/api/policies", () => ({
  listProfiles: () =>
    Promise.resolve([
      {
        id: 1,
        name: "trino",
        type: "hive",
        policy_version: 3,
        policy_count: 3,
        engines: [{ id: "e1", name: "prod", status: "running" }],
        origin: "manual",
      },
      {
        id: 2,
        name: "starrocks",
        type: "olap",
        policy_version: 1,
        policy_count: 0,
        engines: [],
        origin: "generated",
      },
      // A trino-TYPED profile, so the `Download config` item (a `policy:read`
      // GET, offered to a steward) and the `Delete profile` item (a
      // `policy:delete` write, not offered) can be distinguished in the same
      // menu. The two rows above are typed `hive`/`olap` and carry no
      // download item at all.
      {
        id: 3,
        name: "trino-live",
        type: "trino",
        policy_version: 2,
        policy_count: 5,
        engines: [],
        origin: "manual",
      },
    ]),
  createProfile: (name: string, type: string) => createProfile(name, type),
  deleteProfile: (name: string) => deleteProfile(name),
  // `policyWrites.refusalMessage` consults this to let D10's 409 keep its own
  // explanation; the real one is covered by `ProfileDetailPage.test.tsx`.
  compilerOwnedConflict: () => null,
}));

// The every-profile overlap report (moved here from Mask & filter rules, DG
// redesign #2 P8); its own behaviour is `OverlapReport.test.tsx`'s.
let overlapRows: StoredOverlap[] = [];
vi.mock("@/api/generated/perm/perm", () => ({
  usePermListOverlaps: () => ({ data: { status: 200, data: overlapRows }, isLoading: false, isError: false, error: null }),
}));
afterEach(() => {
  overlapRows = [];
});

const ADMIN: Me = { user_id: "u2", email: "admin@egret.local", platform_roles: ["platform_admin"], permissions: [] };

/** Default `me` is the admin, because every test below that drives a WRITE is
 *  measuring the admin's page — SP4b-2 finding 3's whole lesson is that a
 *  measurement taken as `platform_admin` says nothing about a steward's. */
function wrap(ui: React.ReactNode, me: Me | null = ADMIN) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <AuthContext.Provider value={{ me, isLoading: false }}>
        <MemoryRouter>{ui}</MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
}

const STEWARD: Me = { user_id: "u1", email: "steward@egret.local", platform_roles: [], permissions: ["policy:read"] };
const ENGINE_CREATOR: Me = {
  user_id: "u3",
  email: "infra@egret.local",
  platform_roles: [],
  permissions: ["policy:read", "engine:create"],
};

describe("Policies page (profiles landing)", () => {
  it("shows the stored overlapping pairs of every profile, a tag service's included", async () => {
    overlapRows = [
      {
        serviceId: 99,
        serviceName: "orphan_tag",
        kind: "mask",
        coordinate: "catalog=[wh] column=[email] schema=[sales] table=[orders]",
        rules: [
          { id: "r-a", policyId: 11, name: "mask emails" },
          { id: "r-b", policyId: 12, name: "null orders email" },
        ],
        servedToday: null,
        sentence: "two mask policies ...",
        howToFix: "Delete one of the two.",
      },
    ];
    wrap(<Policies />);
    await screen.findByRole("link", { name: "trino" });
    expect(screen.getByText("1 pair of rules overlaps")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "rule mask emails" })).toHaveAttribute("href", "/governance/rules/r-a");
  });

  it("renders each profile as a link to its detail page", async () => {
    wrap(<Policies />);
    const link = await screen.findByRole("link", { name: "trino" });
    expect(link).toHaveAttribute("href", "/governance/policies/trino");
  });

  it("renders the profile type badge", async () => {
    wrap(<Policies />);
    await screen.findByRole("link", { name: "trino" });
    expect(screen.getByText("hive")).toBeInTheDocument();
    expect(screen.getByText("olap")).toBeInTheDocument();
  });

  it("marks a generated profile, and only a generated one (D10)", async () => {
    wrap(<Policies />);
    const trinoRow = (await screen.findByRole("link", { name: "trino" })).closest("tr");
    const starrocksRow = screen.getByRole("link", { name: "starrocks" }).closest("tr");
    expect(within(starrocksRow!).getByText("Generated")).toBeInTheDocument();
    expect(within(trinoRow!).queryByText("Generated")).not.toBeInTheDocument();
  });

  it("renders bound engine names", async () => {
    wrap(<Policies />);
    await screen.findByRole("link", { name: "trino" });
    expect(screen.getByText("prod")).toBeInTheDocument();
  });

  it("renders an em dash for a profile with no bound engines", async () => {
    wrap(<Policies />);
    const link = await screen.findByRole("link", { name: "starrocks" });
    const row = link.closest("tr");
    expect(row).not.toBeNull();
    expect(row?.textContent).toContain("—");
  });

  it("renders the policy version", async () => {
    wrap(<Policies />);
    await screen.findByRole("link", { name: "trino" });
    expect(screen.getByText("v3")).toBeInTheDocument();
  });

  it("renders the policy count badge", async () => {
    wrap(<Policies />);
    // Scope counts to their own rows — the SummaryStrip also renders totals.
    const trinoRow = (await screen.findByRole("link", { name: "trino" })).closest("tr");
    expect(trinoRow?.textContent).toContain("3");
    const starrocksRow = screen.getByRole("link", { name: "starrocks" }).closest("tr");
    expect(starrocksRow?.textContent).toContain("0");
  });

  it("deletes an unbound profile via the actions menu", async () => {
    const user = userEvent.setup();
    wrap(<Policies />);
    await screen.findByRole("link", { name: "starrocks" });

    // starrocks has no bound engine → delete is offered.
    await user.click(screen.getByRole("button", { name: "Actions for starrocks" }));
    await user.click(await screen.findByRole("menuitem", { name: /delete profile/i }));

    // ConfirmDeleteDialog gates on typing "delete".
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/Delete "starrocks"\?/)).toBeInTheDocument();
    await user.type(within(dialog).getByPlaceholderText("delete"), "delete");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));

    expect(deleteProfile).toHaveBeenCalledWith("starrocks");
  });

  it("disables delete for a profile still bound to an engine", async () => {
    const user = userEvent.setup();
    wrap(<Policies />);
    await screen.findByRole("link", { name: "trino" });

    // trino is enforced by an engine → its delete item is disabled.
    await user.click(screen.getByRole("button", { name: "Actions for trino" }));
    const del = await screen.findByRole("menuitem", { name: /delete profile/i });
    expect(del).toHaveAttribute("aria-disabled", "true");
  });

  it("opens the New profile dialog with a name field and a type select", async () => {
    const user = userEvent.setup();
    wrap(<Policies />);
    await screen.findByRole("link", { name: "trino" });
    await user.click(screen.getByRole("button", { name: /New profile/i }));
    const dialog = await screen.findByRole("dialog");
    // Scope to the dialog — the list page also renders a "Type" facet filter.
    expect(within(dialog).getByRole("heading", { name: /New profile/i })).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Name")).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Type")).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: /Create profile/i })).toBeInTheDocument();
  });

  it("hides Provision an engine from a steward with no engine:create", async () => {
    wrap(<Policies />, STEWARD);
    await screen.findByRole("link", { name: "trino" });
    expect(screen.queryByRole("link", { name: /Provision an engine/i })).not.toBeInTheDocument();
  });

  it("shows Provision an engine for a platform_admin", async () => {
    wrap(<Policies />, ADMIN);
    await screen.findByRole("link", { name: "trino" });
    expect(screen.getByRole("link", { name: /Provision an engine/i })).toHaveAttribute(
      "href",
      "/control-plane/engines",
    );
  });

  it("shows Provision an engine for a user holding engine:create", async () => {
    wrap(<Policies />, ENGINE_CREATOR);
    await screen.findByRole("link", { name: "trino" });
    expect(screen.getByRole("link", { name: /Provision an engine/i })).toHaveAttribute(
      "href",
      "/control-plane/engines",
    );
  });

  /**
   * **THE DISCRIMINATING CASE (SP4b-2 finding 3's sibling path).** Every write
   * test above renders as `platform_admin`, who bypasses the PDP and passes
   * all five `policy:*` keys — which is exactly why this page kept the defect
   * the detail page had fixed: a `policy:read` steward was offered `New
   * profile` and `Delete profile`, filled the dialog in, submitted, and read
   * `forbidden`.
   */
  it("offers a policy:read steward no New profile and no Delete profile", async () => {
    const user = userEvent.setup();
    wrap(<Policies />, STEWARD);
    await screen.findByRole("link", { name: "starrocks" });

    expect(screen.queryByRole("button", { name: /New profile/i })).not.toBeInTheDocument();

    // The menu still opens for a trino-typed profile (Download config is a
    // `policy:read` GET) but carries no delete.
    await user.click(screen.getByRole("button", { name: "Actions for trino-live" }));
    expect(await screen.findByRole("menuitem", { name: /download config/i })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /delete profile/i })).not.toBeInTheDocument();
  });

  /** A menu whose every item is gated away must not offer a trigger either —
   *  `starrocks` here is not trino-typed, so a steward has nothing to open. */
  it("renders no actions trigger at all when every item is gated away", async () => {
    const { unmount } = wrap(<Policies />, STEWARD);
    await screen.findByRole("link", { name: "starrocks" });
    // `starrocks` is typed `olap`: no download item, and no delete for a
    // steward, so there is nothing for a trigger to open.
    expect(
      screen.queryByRole("button", { name: "Actions for starrocks" }),
    ).not.toBeInTheDocument();

    // …while the same row DOES give an admin one — otherwise this test would
    // pass just as well if the trigger had been deleted outright.
    unmount();
    wrap(<Policies />, ADMIN);
    await screen.findByRole("link", { name: "starrocks" });
    expect(screen.getByRole("button", { name: "Actions for starrocks" })).toBeInTheDocument();
  });

  /**
   * Hiding is not the whole fix: a permission snapshot can be stale, so a
   * refused write must name the permission instead of echoing the server's
   * bare `forbidden` body. Driven on the delete path, whose error surfaces on
   * the page rather than inside the dialog.
   */
  it("names the permission a refused delete needs, not 'forbidden'", async () => {
    deleteProfile.mockRejectedValueOnce(new ApiError(403, "forbidden"));
    const user = userEvent.setup();
    wrap(<Policies />);
    await screen.findByRole("link", { name: "starrocks" });

    await user.click(screen.getByRole("button", { name: "Actions for starrocks" }));
    await user.click(await screen.findByRole("menuitem", { name: /delete profile/i }));
    const dialog = await screen.findByRole("alertdialog");
    await user.type(within(dialog).getByPlaceholderText("delete"), "delete");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));

    expect(await screen.findByText(/needs the policy:delete permission/)).toBeInTheDocument();
    expect(screen.queryByText(/^forbidden$/)).not.toBeInTheDocument();
  });

  /** The create dialog's own refusal, the second call site (M-1). */
  it("names the permission a refused create needs, not 'forbidden'", async () => {
    createProfile.mockRejectedValueOnce(new ApiError(403, "forbidden"));
    const user = userEvent.setup();
    wrap(<Policies />);
    await screen.findByRole("link", { name: "trino" });

    await user.click(screen.getByRole("button", { name: /New profile/i }));
    const dialog = await screen.findByRole("dialog");
    await user.type(within(dialog).getByLabelText("Name"), "sr-2");
    await user.click(within(dialog).getByRole("button", { name: /Create profile/i }));

    expect(
      await within(dialog).findByText(/needs the policy:create permission/),
    ).toBeInTheDocument();
    expect(within(dialog).queryByText(/^forbidden$/)).not.toBeInTheDocument();
  });
});
