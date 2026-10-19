import { useState } from "react";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

import { MemoryRouter } from "react-router-dom";

vi.mock("@/api/generated/policy-admin/policy-admin");
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));
vi.mock("@/api/generated/perm/perm", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api/generated/perm/perm")>()),
  permListPeople: vi.fn(),
  permAllocateUsername: vi.fn(),
}));

import { toast } from "sonner";
import { ApiError } from "@/api/client";
import type { Me } from "@/api/hooks";
import { permAllocateUsername, permListPeople } from "@/api/generated/perm/perm";
import { policyAdminListPrincipals } from "@/api/generated/policy-admin/policy-admin";
import type { PersonRow, Principal, PrincipalSet } from "@/api/generated/model";
import { AuthContext } from "@/auth/useAuth";
import { PrincipalPicker } from "@/components/perm/PrincipalPicker";
import { Dialog, DialogContent, DialogDescription, DialogTitle } from "@/components/ui/dialog";

const USERS: Principal[] = [
  { kind: "user", name: "alice.doe@example.com", label: "Alice Doe", username: "alice-doe" },
];

function mockPrincipals(rows: Principal[] = USERS) {
  vi.mocked(policyAdminListPrincipals).mockResolvedValue({
    status: 200,
    data: rows,
    headers: new Headers(),
  } as Awaited<ReturnType<typeof policyAdminListPrincipals>>);
}

function viewer(permissions: string[]): Me {
  return { user_id: "steward-1", email: "steward@example.com", platform_roles: [], permissions };
}

/** Holds the value like a real dialog does, so a pick shows up as a chip. */
function StatefulPicker({ onChange, exempt }: { onChange: (next: PrincipalSet) => void; exempt?: boolean }) {
  const [value, setValue] = useState<PrincipalSet>({ include: [], exclude: [] });
  return (
    <PrincipalPicker
      value={value}
      exempt={exempt}
      onChange={(next) => {
        setValue(next);
        onChange(next);
      }}
    />
  );
}

function renderPicker(
  props: Partial<React.ComponentProps<typeof PrincipalPicker>> = {},
  { me = null, stateful = false }: { me?: Me | null; stateful?: boolean } = {},
) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const invalidate = vi.spyOn(qc, "invalidateQueries");
  const onChange = vi.fn();
  const value: PrincipalSet = props.value ?? { include: [], exclude: [] };
  render(
    <MemoryRouter>
      <AuthContext.Provider value={{ me, isLoading: false }}>
        <QueryClientProvider client={qc}>
          {stateful ? (
            <StatefulPicker onChange={onChange} exempt={props.exempt} />
          ) : (
            <PrincipalPicker value={value} onChange={onChange} {...props} />
          )}
        </QueryClientProvider>
      </AuthContext.Provider>
    </MemoryRouter>,
  );
  return { onChange, invalidate };
}

const BRAND_NEW: Principal = { kind: "user", name: "brandnew@example.com", label: "Brand New", username: null };

function personRow(key: string, detail: string, username: string | null = null): PersonRow {
  return { kind: "user", key, label: "Brand New", detail, username, serviceAccount: false };
}

function mockPeople(rows: PersonRow[]) {
  vi.mocked(permListPeople).mockResolvedValue({
    status: 200,
    data: { items: rows, total: rows.length },
    headers: new Headers(),
  } as Awaited<ReturnType<typeof permListPeople>>);
}

describe("PrincipalPicker", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockPrincipals();
  });

  test("the kind picker never offers workspace_members", async () => {
    const user = userEvent.setup();
    renderPicker();

    await user.click(screen.getByRole("combobox", { name: "Include kind" }));
    expect(await screen.findByRole("option", { name: "user" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "group" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "role" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "workspace_members" })).not.toBeInTheDocument();
  });

  // J-F3 Task 1b: a group compiles to its members, so the point-of-choice notice
  // says the grant follows the membership — and the old "not enforced" warning,
  // now false, is gone.
  test("choosing group says it applies to the group's current members", async () => {
    const user = userEvent.setup();
    renderPicker();

    expect(screen.queryByText(/current members/)).not.toBeInTheDocument();

    await user.click(screen.getByRole("combobox", { name: "Include kind" }));
    await user.click(await screen.findByRole("option", { name: "group" }));

    expect(await screen.findByText(/current members/)).toBeInTheDocument();
    expect(screen.queryByText(/Not enforced yet/)).not.toBeInTheDocument();
  });

  test("includeOnly renders no Exclude list and still offers user, group and role; without it Exclude is there", async () => {
    const user = userEvent.setup();
    renderPicker({ includeOnly: true });

    expect(screen.queryByText("Exclude")).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "Exclude kind" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("combobox", { name: "Include kind" }));
    expect(await screen.findByRole("option", { name: "user" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "group" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "role" })).toBeInTheDocument();
  });

  test("without includeOnly the Exclude list is rendered", () => {
    renderPicker();
    expect(screen.getByRole("combobox", { name: "Exclude kind" })).toBeInTheDocument();
  });

  test("exempt mode renders no Exclude section, and Include offers no role kind", async () => {
    const user = userEvent.setup();
    renderPicker({ exempt: true });

    expect(screen.queryByText("Exclude")).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: "Exclude kind" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("combobox", { name: "Include kind" }));
    expect(await screen.findByRole("option", { name: "user" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "group" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "role" })).not.toBeInTheDocument();
  });

  test("selecting a user suggestion stores the server-allocated username, not the email, and shows which value is stored", async () => {
    const user = userEvent.setup();
    const { onChange } = renderPicker();

    await user.type(screen.getByRole("combobox", { name: "Include user search" }), "alice");
    expect(await screen.findByText("stored as alice-doe")).toBeInTheDocument();

    await user.click(await screen.findByText("Alice Doe"));

    await waitFor(() =>
      expect(onChange).toHaveBeenCalledWith({
        include: [{ kind: "user", ref: "alice-doe" }],
        exclude: [],
      }),
    );
  });

  // THE test that matters: a user whose stored username carries a collision
  // suffix (`ensure_username` appended `-2` because `alice-doe` was already
  // taken) must be offered with that EXACT suffixed value. A fixture where
  // every username happens to equal the naive slug of the email would pass
  // even with the old slugify-guess bug — that guess only diverges from the
  // truth on exactly this collision case.
  test("a colliding username's suffix is what gets shown and stored, never the naive slug", async () => {
    const user = userEvent.setup();
    mockPrincipals([
      {
        kind: "user",
        name: "alice.doe@second.example.com",
        label: "Alice Doe (Marketing)",
        username: "alice-doe-2",
      },
    ]);
    const { onChange } = renderPicker();

    await user.type(screen.getByRole("combobox", { name: "Include user search" }), "alice");
    expect(await screen.findByText("stored as alice-doe-2")).toBeInTheDocument();
    expect(screen.queryByText("stored as alice-doe")).not.toBeInTheDocument();

    await user.click(await screen.findByText("Alice Doe (Marketing)"));

    await waitFor(() =>
      expect(onChange).toHaveBeenCalledWith({
        include: [{ kind: "user", ref: "alice-doe-2" }],
        exclude: [],
      }),
    );
  });

  test("a user with no allocated username is shown but cannot be selected", async () => {
    const user = userEvent.setup();
    mockPrincipals([
      { kind: "user", name: "brandnew@example.com", label: "Brand New", username: null },
    ]);
    const { onChange } = renderPicker();

    await user.type(screen.getByRole("combobox", { name: "Include user search" }), "brand");
    expect(await screen.findByText("Brand New")).toBeInTheDocument();
    expect(screen.getByText("no allocated username yet")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Brand New/ })).not.toBeInTheDocument();

    await user.click(screen.getByText("Brand New"));
    expect(onChange).not.toHaveBeenCalled();
  });

  // D9a (spec P12): the list stays open after a pick while the field has focus,
  // so several principals are picked in a row; typing reopens it after Escape.
  test("picks several users in a row without leaving the field, and Escape closes the list", async () => {
    const user = userEvent.setup();
    mockPrincipals([
      { kind: "user", name: "alice.doe@example.com", label: "Alice Doe", username: "alice-doe" },
      { kind: "user", name: "bob.roe@example.com", label: "Bob Roe", username: "bob-roe" },
      { kind: "user", name: "carol.poe@example.com", label: "Carol Poe", username: "carol-poe" },
    ]);
    const { onChange } = renderPicker({}, { stateful: true });
    const field = screen.getByRole("combobox", { name: "Include user search" });

    await user.click(field);
    await user.type(field, "o");
    await user.click(await screen.findByRole("option", { name: /Alice Doe/ }));
    expect(screen.getByRole("button", { name: "Remove alice-doe from include" })).toBeInTheDocument();
    expect(field).toHaveFocus();
    expect(field).toHaveAttribute("aria-expanded", "true");

    await user.click(screen.getByRole("option", { name: /Bob Roe/ }));
    expect(screen.getByRole("button", { name: "Remove alice-doe from include" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove bob-roe from include" })).toBeInTheDocument();

    // The keyboard picks too: Carol is the third option.
    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}{Enter}");
    expect(screen.getByRole("button", { name: "Remove carol-poe from include" })).toBeInTheDocument();
    expect(onChange).toHaveBeenLastCalledWith({
      include: [
        { kind: "user", ref: "alice-doe" },
        { kind: "user", ref: "bob-roe" },
        { kind: "user", ref: "carol-poe" },
      ],
      exclude: [],
    });

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(field).toHaveAttribute("aria-expanded", "false");

    await user.type(field, "b");
    expect(await screen.findByRole("listbox")).toBeInTheDocument();
  });

  // D9b (spec P13, Q1): a user with no username is allocated one through
  // `POST /perm/people/users/{id}/username`, after a confirmation, then added
  // under the name the server returned.
  test("with policy:update, a user with no username can be allocated one and added, from the keyboard", async () => {
    const user = userEvent.setup();
    mockPrincipals([BRAND_NEW]);
    // A decoy whose email merely contains the picked one comes first: only the exact email is this user.
    mockPeople([personRow("u-decoy", "brandnew@example.com.au"), personRow("u-1", "BrandNew@example.com")]);
    vi.mocked(permAllocateUsername).mockResolvedValue({
      status: 200,
      data: { username: "brand-new", allocated: true },
      headers: new Headers(),
    } as Awaited<ReturnType<typeof permAllocateUsername>>);
    const { onChange, invalidate } = renderPicker({}, { me: viewer(["policy:read", "policy:update"]), stateful: true });
    const field = screen.getByRole("combobox", { name: "Include user search" });

    await user.type(field, "brand");
    const allocate = await screen.findByRole("button", { name: "Allocate username and add Brand New" });
    expect(allocate).toHaveTextContent("Allocate username and add");
    expect(screen.queryByText("no allocated username yet")).not.toBeInTheDocument();

    await user.tab();
    expect(allocate).toHaveFocus();
    await user.keyboard("{Enter}");

    const confirm = await screen.findByRole("alertdialog", { name: "Allocate a username to Brand New?" });
    expect(permAllocateUsername).not.toHaveBeenCalled();
    await user.click(within(confirm).getByRole("button", { name: "Allocate and add" }));

    await waitFor(() =>
      expect(onChange).toHaveBeenCalledWith({ include: [{ kind: "user", ref: "brand-new" }], exclude: [] }),
    );
    expect(permListPeople).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "user", q: "brandnew@example.com", includeServiceAccounts: true }),
    );
    expect(permAllocateUsername).toHaveBeenCalledTimes(1);
    expect(permAllocateUsername).toHaveBeenCalledWith("u-1");
    expect(await screen.findByText(/stored as brand-new/)).toBeInTheDocument();
    // FE-ACTION-2: the dialog stays, so a toast confirms it as well as the line under the field.
    expect(toast.success).toHaveBeenCalledWith("Brand New was allocated the username brand-new and added.");
    expect(screen.getByRole("button", { name: "Remove brand-new from include" })).toBeInTheDocument();
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["perm-principals"] });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["/api/control-plane/perm/people"] });
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["/api/control-plane/perm/people/users/u-1"] });
  });

  test("a 409 shows the server's refusal beside the field with links to the stored refs, and adds nothing", async () => {
    const user = userEvent.setup();
    mockPrincipals([BRAND_NEW]);
    mockPeople([personRow("u-1", "brandnew@example.com")]);
    const message =
      '2 stored grant principal(s) or rule exemption(s) already name "brand-new" as a user; they match nobody today and would start applying to this person. Remove or change them, then allocate again';
    vi.mocked(permAllocateUsername).mockRejectedValue(
      new ApiError(409, message, undefined, {
        error: message,
        refs: [
          { source: "grant", id: "g-7", position: "include", ref: "Brand-New" },
          { source: "rule", id: "r-3", position: "include", ref: "brand-new" },
        ],
      }),
    );
    const { onChange } = renderPicker({}, { me: viewer(["policy:update"]), stateful: true });
    const field = screen.getByRole("combobox", { name: "Include user search" });

    await user.type(field, "brand");
    await user.click(await screen.findByRole("button", { name: "Allocate username and add Brand New" }));
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Allocate and add" }),
    );

    expect(await screen.findByText(message)).toBeInTheDocument();
    expect(field).toHaveAccessibleDescription(expect.stringContaining("already name"));
    expect(screen.getByRole("link", { name: /grant g-7/ })).toHaveAttribute("href", "/governance/grants/g-7");
    expect(screen.getByRole("link", { name: /rule r-3/ })).toHaveAttribute("href", "/governance/rules/r-3");
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.queryByText(/stored as brand-new/)).not.toBeInTheDocument();
  });

  test("without policy:update, a user with no username offers no allocation and says how one is allocated", async () => {
    const user = userEvent.setup();
    mockPrincipals([BRAND_NEW]);
    const { onChange } = renderPicker({}, { me: viewer(["policy:read"]) });

    await user.type(screen.getByRole("combobox", { name: "Include user search" }), "brand");
    expect(await screen.findByText("no allocated username yet")).toBeInTheDocument();
    expect(screen.getByText(/first query/)).toBeInTheDocument();
    expect(screen.getByText(/policy:update/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Allocate username and add/ })).not.toBeInTheDocument();
    expect(onChange).not.toHaveBeenCalled();
  });

  // The grant and rule dialogs are Radix dialogs, which close on Escape from
  // the document's capture phase: Escape in the field must close the list only.
  test("Escape in the field closes the list, not the dialog around the picker", async () => {
    const user = userEvent.setup();
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const onOpenChange = vi.fn();
    render(
      <QueryClientProvider client={qc}>
        <Dialog open onOpenChange={onOpenChange}>
          <DialogContent>
            <DialogTitle>New grant</DialogTitle>
            <DialogDescription>Who gets access.</DialogDescription>
            <PrincipalPicker value={{ include: [], exclude: [] }} onChange={vi.fn()} />
          </DialogContent>
        </Dialog>
      </QueryClientProvider>,
    );
    const field = screen.getByRole("combobox", { name: "Include user search" });

    await user.type(field, "alice");
    expect(await screen.findByRole("listbox")).toBeInTheDocument();
    await user.keyboard("{Escape}");

    expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
    expect(onOpenChange).not.toHaveBeenCalled();
    expect(field).toHaveFocus();

    // With the list closed, Escape is the dialog's again.
    await user.keyboard("{Escape}");
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  // Task 13 review: the email→id lookup is GET /perm/people, which needs policy:read, not policy:update.
  test("a 403 on the user lookup names policy:read, and nothing is allocated", async () => {
    const user = userEvent.setup();
    mockPrincipals([BRAND_NEW]);
    vi.mocked(permListPeople).mockRejectedValue(new ApiError(403, "forbidden"));
    const { onChange } = renderPicker({}, { me: viewer(["policy:update"]) });

    await user.type(screen.getByRole("combobox", { name: "Include user search" }), "brand");
    await user.click(await screen.findByRole("button", { name: "Allocate username and add Brand New" }));
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Allocate and add" }),
    );

    expect(await screen.findByRole("alert")).toHaveTextContent("policy:read");
    expect(screen.getByRole("alert")).not.toHaveTextContent("policy:update");
    expect(permAllocateUsername).not.toHaveBeenCalled();
    expect(onChange).not.toHaveBeenCalled();
    expect(toast.success).not.toHaveBeenCalled();
  });

  // Task 13 review: aria-controls names a listbox that exists whenever the list is open, status text outside it.
  test("aria-controls names the rendered listbox while open, even when no row is pickable", async () => {
    const user = userEvent.setup();
    mockPrincipals([BRAND_NEW]);
    renderPicker({}, { me: viewer(["policy:read"]) });
    const field = screen.getByRole("combobox", { name: "Include user search" });
    expect(field).not.toHaveAttribute("aria-controls");

    await user.type(field, "brand");
    expect(await screen.findByText("no allocated username yet")).toBeInTheDocument();
    const list = screen.getByRole("listbox", { name: "Include user suggestions" });
    expect(field).toHaveAttribute("aria-controls", list.id);
    expect(within(list).queryAllByRole("option")).toHaveLength(0);
    expect(list).not.toHaveTextContent("Brand New");
  });

  // Final review minor d: a service account's People row shows "service account", not its email.
  test("a service account with no username is found by its label and allocated", async () => {
    const user = userEvent.setup();
    mockPrincipals([BRAND_NEW]);
    mockPeople([
      { ...personRow("u-other", "service account"), label: "Brand New Bot", serviceAccount: true },
      { ...personRow("u-sa", "service account"), serviceAccount: true },
    ]);
    vi.mocked(permAllocateUsername).mockResolvedValue({
      status: 200,
      data: { username: "brand-new", allocated: true },
      headers: new Headers(),
    } as Awaited<ReturnType<typeof permAllocateUsername>>);
    const { onChange } = renderPicker({}, { me: viewer(["policy:read", "policy:update"]) });

    await user.type(screen.getByRole("combobox", { name: "Include user search" }), "brand");
    await user.click(await screen.findByRole("button", { name: "Allocate username and add Brand New" }));
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Allocate and add" }),
    );

    await waitFor(() => expect(permAllocateUsername).toHaveBeenCalledWith("u-sa"));
    await waitFor(() => expect(onChange).toHaveBeenCalledWith({ include: [{ kind: "user", ref: "brand-new" }], exclude: [] }));
  });

  test("an allocation for a user People cannot find says so and adds nothing", async () => {
    const user = userEvent.setup();
    mockPrincipals([BRAND_NEW]);
    mockPeople([personRow("u-decoy", "brandnew@example.com.au")]);
    const { onChange } = renderPicker({}, { me: viewer(["policy:update"]) });

    await user.type(screen.getByRole("combobox", { name: "Include user search" }), "brand");
    await user.click(await screen.findByRole("button", { name: "Allocate username and add Brand New" }));
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Allocate and add" }),
    );

    expect(await screen.findByText(/Brand New could not be found among People/)).toHaveTextContent(
      "A service account that cannot be found here gets its username at its first token.",
    );
    expect(permAllocateUsername).not.toHaveBeenCalled();
    expect(onChange).not.toHaveBeenCalled();
  });
});
