import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

vi.mock("@/api/generated/policy-admin/policy-admin");

import { policyAdminListPrincipals } from "@/api/generated/policy-admin/policy-admin";
import type { Principal, PrincipalSet } from "@/api/generated/model";
import { PrincipalPicker, slugifyLocalpart } from "@/components/perm/PrincipalPicker";

const USERS: Principal[] = [
  { kind: "user", name: "alice.doe@example.com", label: "Alice Doe" },
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

  test("slugifyLocalpart mirrors the server's slugify_localpart", () => {
    expect(slugifyLocalpart("Alice@Corp.com")).toBe("alice");
    expect(slugifyLocalpart("john.doe@example.com")).toBe("john-doe");
    expect(slugifyLocalpart("@empty.com")).toBe("user");
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

  test("choosing group shows the not-enforced warning at the point of choice", async () => {
    const user = userEvent.setup();
    renderPicker();

    expect(screen.queryByText(/Not enforced yet/)).not.toBeInTheDocument();

    await user.click(screen.getByRole("combobox", { name: "Include kind" }));
    await user.click(await screen.findByRole("option", { name: "group" }));

    expect(await screen.findByText(/Not enforced yet/)).toBeInTheDocument();
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

  test("selecting a user suggestion stores the bare username, not the email, and shows which value is stored", async () => {
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
});
