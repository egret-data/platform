import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

import UsersPage from "@/routes/control-plane/access/UsersPage";
import * as api from "@/api/access";

vi.mock("@/api/access");

const USERS: api.AccessUser[] = [
  {
    id: "u1",
    email: "alice@example.com",
    name: "Alice",
    provider: "local",
    status: "active",
    roles: ["Admin"],
    groups: [],
  },
  {
    id: "u2",
    email: "bob@example.com",
    name: "Bob",
    provider: "oidc",
    status: "disabled",
    roles: [],
    groups: ["Engineering"],
  },
];

const ROLES: api.AccessRole[] = [
  { id: "r1", scope: "platform", name: "Admin", description: "", permissionCount: 0, assigneeCount: 1 },
  { id: "r2", scope: "platform", name: "Analyst", description: "", permissionCount: 0, assigneeCount: 0 },
];

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <UsersPage />
    </QueryClientProvider>,
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
    expect(screen.getByText("Engineering")).toBeInTheDocument();
    expect(screen.getByText("Admin")).toBeInTheDocument();
    expect(screen.getByText("active")).toBeInTheDocument();
    expect(screen.getByText("disabled")).toBeInTheDocument();
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

  test("Assign roles dialog diffs the selection into assign/unassign calls", async () => {
    const user = userEvent.setup();
    vi.mocked(api.assignUserRole).mockResolvedValue(undefined);
    vi.mocked(api.unassignUserRole).mockResolvedValue(undefined);

    renderPage();
    await screen.findByText("alice@example.com");

    await user.click(screen.getByRole("button", { name: /Assign roles to alice@example.com/i }));
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
});
