import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

vi.mock("@/api/generated/policy-admin/policy-admin");

import { policyAdminListPrincipals } from "@/api/generated/policy-admin/policy-admin";
import type { Principal, PrincipalSet } from "@/api/generated/model";
import { PrincipalPicker } from "@/components/perm/PrincipalPicker";

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

function renderPicker(props: Partial<React.ComponentProps<typeof PrincipalPicker>> = {}) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onChange = vi.fn();
  const value: PrincipalSet = props.value ?? { include: [], exclude: [] };
  render(
    <QueryClientProvider client={qc}>
      <PrincipalPicker value={value} onChange={onChange} {...props} />
    </QueryClientProvider>,
  );
  return { onChange };
}

describe("PrincipalPicker", () => {
  beforeEach(() => {
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

    await user.type(screen.getByRole("textbox", { name: "Include user search" }), "alice");
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

    await user.type(screen.getByRole("textbox", { name: "Include user search" }), "alice");
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

    await user.type(screen.getByRole("textbox", { name: "Include user search" }), "brand");
    expect(await screen.findByText("Brand New")).toBeInTheDocument();
    expect(screen.getByText("no allocated username yet")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Brand New/ })).not.toBeInTheDocument();

    await user.click(screen.getByText("Brand New"));
    expect(onChange).not.toHaveBeenCalled();
  });
});
