import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, it, expect, vi } from "vitest";
import Policies from "../Policies";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";

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
    ]),
  createProfile: (name: string, type: string) => createProfile(name, type),
  deleteProfile: (name: string) => deleteProfile(name),
}));

function wrap(ui: React.ReactNode, me: Me | null = null) {
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
const ADMIN: Me = { user_id: "u2", email: "admin@egret.local", platform_roles: ["platform_admin"], permissions: [] };
const ENGINE_CREATOR: Me = {
  user_id: "u3",
  email: "infra@egret.local",
  platform_roles: [],
  permissions: ["policy:read", "engine:create"],
};

describe("Policies page (profiles landing)", () => {
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
});
