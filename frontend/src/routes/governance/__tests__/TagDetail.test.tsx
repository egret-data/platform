import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

import TagDetail from "@/routes/governance/TagDetail";
import * as tagsApi from "@/api/tags";
import * as accessApi from "@/api/access";
import { useAuth } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";

vi.mock("@/api/tags");
vi.mock("@/api/access");
vi.mock("@/auth/useAuth");
vi.mock("react-router-dom", async () => {
  const actual = await vi.importActual<typeof import("react-router-dom")>("react-router-dom");
  return { ...actual, useParams: () => ({ id: "t1" }), useNavigate: () => vi.fn() };
});

const ADMIN_ME = {
  user_id: "1",
  email: "admin@example.com",
  platform_roles: ["platform_admin"],
  permissions: [],
} as unknown as Me;

const TAG: tagsApi.Tag = {
  id: "t1",
  key: "pii",
  name: "PII",
  description: "Personally identifiable information",
  color: "#e11d48",
  assignment_count: 3,
  created_at: "2026-07-01T00:00:00Z",
  required_roles: [],
  required_groups: [],
};

const ROLES = [
  { id: "r1", scope: "platform", name: "finance_cleared", description: "", permissionCount: 0, assigneeCount: 0, inheritedPermissions: [] },
] as unknown as accessApi.AccessRole[];
const GROUPS = [
  { id: "g1", name: "Security Team", source: "local", memberCount: 4, roles: [] },
] as unknown as accessApi.AccessGroup[];

const setMutate = vi.fn();

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <TagDetail />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("TagDetail", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(useAuth).mockReturnValue({ me: ADMIN_ME, isLoading: false } as ReturnType<typeof useAuth>);
    vi.mocked(tagsApi.useTags).mockReturnValue({
      data: [TAG],
      isLoading: false,
      isError: false,
      error: null,
    } as unknown as ReturnType<typeof tagsApi.useTags>);
    vi.mocked(accessApi.listRoles).mockResolvedValue(ROLES);
    vi.mocked(accessApi.listGroups).mockResolvedValue(GROUPS);
    const stub = (m: unknown) => ({ mutate: m, isPending: false }) as never;
    vi.mocked(tagsApi.useUpdateTag).mockReturnValue(stub(vi.fn()));
    vi.mocked(tagsApi.useSetTagAccessPolicy).mockReturnValue(stub(setMutate));
    vi.mocked(tagsApi.useClearTagAccessPolicy).mockReturnValue(stub(vi.fn()));
    vi.mocked(tagsApi.useDeleteTag).mockReturnValue(stub(vi.fn()));
  });

  test("renders the tag identity + immutable key", async () => {
    renderPage();
    expect(await screen.findByRole("heading", { name: "PII" })).toBeInTheDocument();
    expect(screen.getByText("pii")).toBeInTheDocument();
    // Key input is present and read-only.
    expect(screen.getByLabelText("Key")).toHaveAttribute("readonly");
  });

  test("links back to Governance → Classifications", async () => {
    renderPage();
    await screen.findByRole("heading", { name: "PII" });
    expect(screen.getByRole("link", { name: "Classifications" })).toHaveAttribute("href", "/governance/classifications");
  });

  test("picking a role and clicking Add persists the clearance", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByRole("heading", { name: "PII" });

    await user.click(screen.getByRole("combobox", { name: "Role to add" }));
    await user.click(await screen.findByText("finance_cleared"));
    await user.click(screen.getByRole("button", { name: "Add clearance" }));

    expect(setMutate).toHaveBeenCalledWith(
      { id: "t1", required_roles: ["finance_cleared"], required_groups: [] },
      expect.anything(),
    );
  });
});
