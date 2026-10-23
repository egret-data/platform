import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import UsersPage from "@/routes/control-plane/access/UsersPage";
import * as api from "@/api/access";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";

vi.mock("@/api/access");

// Role assignment is platform_admin-only; default to a platform_admin `me`
// so existing assertions (which exercise "Assign roles") keep passing.
// Tests that care about the non-admin view pass their own `me`.
const ADMIN_ME: Me = {
  user_id: "1",
  email: "admin@example.com",
  platform_roles: ["platform_admin"],
  permissions: [],
};

const USERS: api.AccessUser[] = [
  {
    id: "u1",
    email: "alice@example.com",
    name: "Alice",
    provider: "local",
    status: "active",
    roles: ["Admin"],
    groups: [],
    username: "alice",
  },
  {
    id: "u2",
    email: "bob@example.com",
    name: "Bob",
    provider: "oidc",
    status: "disabled",
    roles: [],
    groups: ["Engineering"],
    username: null,
  },
];

const ROLES: api.AccessRole[] = [
  { id: "r1", scope: "platform", name: "Admin", description: "", permissionCount: 0, assigneeCount: 1, inheritedPermissions: [] },
  { id: "r2", scope: "platform", name: "Analyst", description: "", permissionCount: 0, assigneeCount: 0, inheritedPermissions: [] },
];

function renderPage(me: Me | null = ADMIN_ME) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <MemoryRouter>
          <UsersPage />
        </MemoryRouter>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

describe("UsersPage", () => {
  beforeEach(() => {
    vi.mocked(api.listUsers).mockResolvedValue(USERS);
    vi.mocked(api.listRoles).mockResolvedValue(ROLES);
  });

  test("renders rows from the mocked user list", async () => {
    renderPage();
    expect(await screen.findByText("alice@example.com")).toBeInTheDocument();
    expect(screen.getByText("bob@example.com")).toBeInTheDocument();
    expect(screen.getByText("Alice")).toBeInTheDocument();
    expect(screen.getByText("Admin")).toBeInTheDocument();
    // "active"/"disabled" also appear as SummaryStrip labels, so scope the
    // status-badge assertions to each user's row.
    const aliceRow = screen.getByText("alice@example.com").closest("tr") as HTMLElement;
    expect(within(aliceRow).getByText("active")).toBeInTheDocument();
    const bobRow = screen.getByText("bob@example.com").closest("tr") as HTMLElement;
    expect(within(bobRow).getByText("disabled")).toBeInTheDocument();
  });

  test("renders a user's group membership as a count chip in the Groups column", async () => {
    renderPage();
    await screen.findByText("bob@example.com");

    expect(screen.getByRole("columnheader", { name: "Groups" })).toBeInTheDocument();

    // Bob belongs to one group ("Engineering"): the cell shows the count "1"
    // with the full list on hover (title), not a wide row of chips.
    const bobRow = screen.getByText("bob@example.com").closest("tr") as HTMLElement;
    const groupChip = within(bobRow).getByTitle("Engineering");
    expect(groupChip).toHaveTextContent("1");

    // Alice has no groups, so her Groups cell falls back to a dash.
    const aliceRow = screen.getByText("alice@example.com").closest("tr");
    expect(aliceRow).not.toBeNull();
    expect(within(aliceRow as HTMLElement).getByText("—")).toBeInTheDocument();
  });

  test("renders an empty state when there are no users", async () => {
    vi.mocked(api.listUsers).mockResolvedValue([]);
    renderPage();
    expect(await screen.findByText("No users yet")).toBeInTheDocument();
  });

  test("the summary strip shows the total user count", async () => {
    renderPage();
    await screen.findByText("alice@example.com");
    const strip = screen.getByText("users").closest("span")?.parentElement as HTMLElement;
    expect(within(strip).getByText("2")).toBeInTheDocument();
    expect(within(strip).getByText("users")).toBeInTheDocument();
  });

  test("typing in the search box narrows the visible rows", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("alice@example.com");

    await user.type(screen.getByPlaceholderText("Search users…"), "bob");

    expect(screen.getByText("bob@example.com")).toBeInTheDocument();
    expect(screen.queryByText("alice@example.com")).not.toBeInTheDocument();
  });

  test("the Status facet filters rows to the chosen status", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("alice@example.com");

    // Alice is active, Bob is disabled. Pick "Active" → only Alice remains.
    await user.click(screen.getByRole("combobox", { name: "Status" }));
    await user.click(await screen.findByRole("option", { name: "Active" }));

    expect(screen.getByText("alice@example.com")).toBeInTheDocument();
    expect(screen.queryByText("bob@example.com")).not.toBeInTheDocument();
  });

  test("New user dialog calls createUser with the entered fields", async () => {
    const user = userEvent.setup();
    vi.mocked(api.createUser).mockResolvedValue({
      id: "u3",
      email: "carol@example.com",
      name: "Carol",
      provider: "local",
      status: "active",
      roles: [],
      groups: [],
    });

    renderPage();
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("button", { name: /New user/i }));
    await screen.findByRole("heading", { name: /New user/i });

    await user.type(screen.getByLabelText("Email"), "carol@example.com");
    await user.type(screen.getByLabelText("Name"), "Carol");
    await user.type(screen.getByLabelText("Password"), "s3cret!");
    await user.click(screen.getByRole("button", { name: /^Create$/i }));

    await waitFor(() =>
      expect(api.createUser).toHaveBeenCalledWith({
        email: "carol@example.com",
        name: "Carol",
        password: "s3cret!",
      }),
    );
  });

  test("New user: Create stays disabled until email, name and password are valid", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("button", { name: /New user/i }));
    await screen.findByRole("heading", { name: /New user/i });

    const create = screen.getByRole("button", { name: /^Create$/i });
    expect(create).toBeDisabled();

    await user.type(screen.getByLabelText("Email"), "not-an-email");
    await user.type(screen.getByLabelText("Name"), "Carol");
    await user.type(screen.getByLabelText("Password"), "pw");
    // Email is still malformed, so submit stays gated.
    expect(create).toBeDisabled();

    await user.clear(screen.getByLabelText("Email"));
    await user.type(screen.getByLabelText("Email"), "carol@example.com");
    expect(create).toBeEnabled();
  });

  test("New user: the password show/hide toggle flips the input type", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("button", { name: /New user/i }));
    await screen.findByRole("heading", { name: /New user/i });

    const pw = screen.getByLabelText("Password") as HTMLInputElement;
    expect(pw.type).toBe("password");
    await user.click(screen.getByRole("button", { name: "Show password" }));
    expect(pw.type).toBe("text");
    await user.click(screen.getByRole("button", { name: "Hide password" }));
    expect(pw.type).toBe("password");
  });

  test("Assign roles dialog diffs the selection into assign/unassign calls", async () => {
    const user = userEvent.setup();
    vi.mocked(api.assignUserRole).mockResolvedValue(undefined);
    vi.mocked(api.unassignUserRole).mockResolvedValue(undefined);

    renderPage();
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("button", { name: /Actions for alice@example.com/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Assign roles" }));
    await screen.findByRole("heading", { name: /Assign roles/i });

    // Alice currently has "Admin" as a chip; remove it.
    await user.click(screen.getByRole("button", { name: "remove Admin" }));

    // Open the combobox and add "Analyst" from the suggestion list. The
    // combobox trigger is opened with a raw click event rather than
    // userEvent.click on the nested input: userEvent's full pointerdown /
    // focus / mouseup sequence interacts badly with the surrounding Dialog's
    // focus trap in jsdom (focus bounces back to the trigger before the
    // popover's open state settles), whereas a plain click event does not.
    const roleInput = screen.getByLabelText("Add role");
    fireEvent.click(roleInput.closest('[role="button"]') as HTMLElement);
    await user.click(await screen.findByText("Analyst"));

    await user.click(screen.getByRole("button", { name: /^Save$/i }));

    await waitFor(() => expect(api.assignUserRole).toHaveBeenCalledWith("u1", "r2"));
    await waitFor(() => expect(api.unassignUserRole).toHaveBeenCalledWith("u1", "r1"));
  });

  test("bulk-selecting two users and clicking Disable calls updateUser for both", async () => {
    const user = userEvent.setup();
    vi.mocked(api.updateUser).mockResolvedValue(USERS[0]);

    renderPage();
    await screen.findByText("alice@example.com");

    // Enter bulk mode → a checkbox column appears.
    await user.click(screen.getByRole("button", { name: /Bulk select/i }));

    const aliceRow = screen.getByText("alice@example.com").closest("tr") as HTMLElement;
    const bobRow = screen.getByText("bob@example.com").closest("tr") as HTMLElement;
    await user.click(within(aliceRow).getByRole("checkbox", { name: "Select row" }));
    await user.click(within(bobRow).getByRole("checkbox", { name: "Select row" }));

    // The bulk action bar reports the selection and offers Disable.
    expect(screen.getByText("2 selected")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /^Disable$/ }));

    await waitFor(() =>
      expect(api.updateUser).toHaveBeenCalledWith("u1", { status: "disabled" }),
    );
    await waitFor(() =>
      expect(api.updateUser).toHaveBeenCalledWith("u2", { status: "disabled" }),
    );
  });

  /**
   * The search box and the `pageSize={25}` pager live INSIDE `FilterableTable`,
   * so the list this page hands it is a superset of what is on screen. Targets
   * used to be derived from that superset, which meant a user selected while
   * visible and then searched away was still Disabled — a platform-wide account
   * action, applied to someone the operator could no longer see.
   *
   * Two users and a search term that separates them, so "acted on the right
   * row" and "acted on row 0" are different claims.
   */
  test("does not disable a user who was selected and then searched away", async () => {
    const user = userEvent.setup();
    vi.mocked(api.updateUser).mockResolvedValue(USERS[0]);

    renderPage();
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("button", { name: /Bulk select/i }));
    const aliceRow = screen.getByText("alice@example.com").closest("tr") as HTMLElement;
    const bobRow = screen.getByText("bob@example.com").closest("tr") as HTMLElement;
    await user.click(within(aliceRow).getByRole("checkbox", { name: "Select row" }));
    await user.click(within(bobRow).getByRole("checkbox", { name: "Select row" }));
    expect(screen.getByText("2 selected")).toBeInTheDocument();

    // Narrow the table's own search until Bob is no longer drawn.
    await user.type(screen.getByPlaceholderText(/Search users/i), "alice");
    await waitFor(() => expect(screen.queryByText("bob@example.com")).toBeNull());

    // The count must follow the screen, not the page's unfiltered list.
    expect(screen.getByText("1 selected")).toBeInTheDocument();

    // This file has no `clearAllMocks`, and an earlier test legitimately calls
    // updateUser("u2"). Without this the negative assertion below would fail on
    // THAT call and pass in isolation — a test that reports the bug is still
    // present when it is not. Clearing here scopes both assertions to this click.
    vi.mocked(api.updateUser).mockClear();

    await user.click(screen.getByRole("button", { name: /^Disable$/ }));

    await waitFor(() =>
      expect(api.updateUser).toHaveBeenCalledWith("u1", { status: "disabled" }),
    );
    // The half that matters: Bob is off screen, so Bob is not touched.
    expect(api.updateUser).not.toHaveBeenCalledWith("u2", { status: "disabled" });
  });

  test("shows a loading skeleton while the user list is pending", () => {
    // Never-resolving fetch keeps the query in its loading state.
    vi.mocked(api.listUsers).mockReturnValue(new Promise(() => {}));
    renderPage();
    expect(screen.getByTestId("list-skeleton")).toBeInTheDocument();
  });

  test("Delete confirmation dialog requires typing 'delete' then calls deleteUser", async () => {
    const user = userEvent.setup();
    vi.mocked(api.deleteUser).mockResolvedValue(undefined);

    renderPage();
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("button", { name: /Actions for alice@example.com/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    await screen.findByRole("heading", { name: /Delete "alice@example.com"/i });

    // The destructive confirm button stays disabled until "delete" is typed.
    const confirmButton = screen.getByRole("button", { name: /^Delete$/i });
    expect(confirmButton).toBeDisabled();

    await user.type(screen.getByPlaceholderText("delete"), "delete");
    expect(confirmButton).toBeEnabled();
    await user.click(confirmButton);

    await waitFor(() => expect(api.deleteUser).toHaveBeenCalledWith("u1"));
  });

  test("cannot delete your own account: the Delete menu item is disabled for the signed-in user", async () => {
    const SELF_ME: Me = {
      user_id: "u1",
      email: "alice@example.com",
      platform_roles: ["platform_admin"],
      permissions: [],
    };
    const user = userEvent.setup();
    renderPage(SELF_ME);
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("button", { name: /Actions for alice@example.com/i }));
    expect(await screen.findByRole("menuitem", { name: "Delete" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
  });

  test("a non-admin user:read viewer can still create/edit users but not assign roles", async () => {
    const NON_ADMIN_ME: Me = {
      user_id: "2",
      email: "viewer@example.com",
      platform_roles: [],
      permissions: ["user:read", "user:create", "user:update"],
    };
    const user = userEvent.setup();
    renderPage(NON_ADMIN_ME);

    expect(await screen.findByText("alice@example.com")).toBeInTheDocument();

    // Delegatable controls remain.
    expect(screen.getByRole("button", { name: /New user/i })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Actions for alice@example.com/i }));
    expect(await screen.findByRole("menuitem", { name: "Edit" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "Assign roles" })).not.toBeInTheDocument();
  });

  // Username takeover (spec 2026-10-23 §4.4).
  test("the row delete dialog names the retired username and offers Disable instead", async () => {
    vi.mocked(api.deleteUser).mockClear();
    vi.mocked(api.updateUser).mockClear();
    vi.mocked(api.updateUser).mockResolvedValue(USERS[0]);
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("button", { name: /Actions for alice@example.com/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const deleteDialog = await screen.findByRole("alertdialog");
    expect(deleteDialog).toHaveTextContent("Their engine username alice is retired for good");

    await user.click(within(deleteDialog).getByRole("button", { name: "Disable instead" }));
    const disableDialog = await screen.findByRole("alertdialog");
    expect(disableDialog).toHaveTextContent("Their engine username stays theirs.");
    await user.click(within(disableDialog).getByRole("button", { name: "Disable user" }));
    await waitFor(() => expect(api.updateUser).toHaveBeenCalledWith("u1", { status: "disabled" }));
    expect(api.deleteUser).not.toHaveBeenCalled();
  });

  test("a disabled user's row delete dialog has no username and no Disable instead", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("bob@example.com");

    await user.click(screen.getByRole("button", { name: /Actions for bob@example.com/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("They have no engine username yet.");
    expect(within(dialog).queryByRole("button", { name: "Disable instead" })).not.toBeInTheDocument();
  });

  test("the row menu disables an active user after confirming", async () => {
    vi.mocked(api.updateUser).mockClear();
    vi.mocked(api.updateUser).mockResolvedValue(USERS[0]);
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("button", { name: /Actions for alice@example.com/i }));
    expect(screen.queryByRole("menuitem", { name: "Enable" })).not.toBeInTheDocument();
    await user.click(await screen.findByRole("menuitem", { name: "Disable" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent(/signed out/i);
    expect(api.updateUser).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole("button", { name: "Disable user" }));
    await waitFor(() => expect(api.updateUser).toHaveBeenCalledWith("u1", { status: "disabled" }));
  });

  test("the row menu enables a disabled user", async () => {
    vi.mocked(api.updateUser).mockClear();
    vi.mocked(api.updateUser).mockResolvedValue(USERS[1]);
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("bob@example.com");

    await user.click(screen.getByRole("button", { name: /Actions for bob@example.com/i }));
    expect(screen.queryByRole("menuitem", { name: "Disable" })).not.toBeInTheDocument();
    await user.click(await screen.findByRole("menuitem", { name: "Enable" }));
    await waitFor(() => expect(api.updateUser).toHaveBeenCalledWith("u2", { status: "active" }));
  });

  test("you cannot disable yourself from the row menu, and it says why", async () => {
    const SELF_ME: Me = {
      user_id: "u1",
      email: "alice@example.com",
      platform_roles: ["platform_admin"],
      permissions: [],
    };
    const user = userEvent.setup();
    renderPage(SELF_ME);
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("button", { name: /Actions for alice@example.com/i }));
    const item = await screen.findByRole("menuitem", { name: /^Disable/ });
    expect(item).toHaveAttribute("aria-disabled", "true");
    // Visible in the item, not only in a title a disabled item never shows.
    expect(within(item).getByText("You cannot disable your own account")).toBeVisible();
  });

  test("the row delete dialog offers no Disable instead to a viewer who cannot disable", async () => {
    const NON_ADMIN_ME: Me = { user_id: "2", email: "ops@example.com", platform_roles: [], permissions: [] };
    const user = userEvent.setup();
    renderPage(NON_ADMIN_ME);
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("button", { name: /Actions for alice@example.com/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).queryByRole("button", { name: "Disable instead" })).not.toBeInTheDocument();
    expect(dialog).not.toHaveTextContent(/disable them instead/);
  });

  test("Disable instead says why for the last active platform admin and does nothing", async () => {
    vi.mocked(api.listUsers).mockResolvedValue([{ ...USERS[0], roles: ["platform_admin"] }, USERS[1]]);
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("button", { name: /Actions for alice@example.com/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog");
    const button = within(dialog).getByRole("button", { name: "Disable instead" });
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveAccessibleDescription(/the last active platform admin cannot be disabled/);
    await user.click(button);
    expect(screen.getAllByRole("alertdialog")).toHaveLength(1);
    expect(screen.queryByRole("button", { name: "Disable user" })).not.toBeInTheDocument();
  });
});
