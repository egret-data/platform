import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

import Classifications from "@/routes/control-plane/governance/Classifications";
import * as tagsApi from "@/api/tags";
import * as accessApi from "@/api/access";
import { useAuth } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";

vi.mock("@/api/tags");
vi.mock("@/api/access");
vi.mock("@/auth/useAuth");

const ADMIN_ME: Me = {
  user_id: "1",
  email: "admin@example.com",
  platform_roles: ["platform_admin"],
  permissions: [],
};

const TAGS: tagsApi.Tag[] = [
  {
    id: "t1",
    key: "pii",
    name: "PII",
    description: "Personally identifiable information",
    color: "#e11d48",
    assignment_count: 3,
    created_at: "2026-07-01T00:00:00Z",
    required_roles: [],
    required_groups: [],
  },
  {
    id: "t2",
    key: "internal",
    name: "Internal",
    description: "Internal use only",
    color: "#0ea5e9",
    assignment_count: 0,
    created_at: "2026-07-02T00:00:00Z",
    required_roles: [],
    required_groups: [],
  },
  {
    id: "t3",
    key: "finance",
    name: "Finance",
    description: "Finance-restricted data",
    color: "#16a34a",
    assignment_count: 1,
    created_at: "2026-07-03T00:00:00Z",
    required_roles: ["finance_cleared"],
    required_groups: [],
  },
];

const GROUP_GATED_TAG: tagsApi.Tag = {
  id: "t4",
  key: "security",
  name: "Security",
  description: "Security-restricted data",
  color: "#a855f7",
  assignment_count: 2,
  created_at: "2026-07-04T00:00:00Z",
  required_roles: [],
  required_groups: ["g1"],
};

const BOTH_GATED_TAG: tagsApi.Tag = {
  id: "t5",
  key: "both",
  name: "BothGated",
  description: "Gated by role and group",
  color: "#f59e0b",
  assignment_count: 0,
  created_at: "2026-07-05T00:00:00Z",
  required_roles: ["finance_cleared"],
  required_groups: ["g1"],
};

const ROLES: accessApi.AccessRole[] = [
  {
    id: "r1",
    scope: "platform",
    name: "finance_cleared",
    description: "Finance clearance",
    permissionCount: 0,
    assigneeCount: 0,
    inheritedPermissions: [],
  },
  {
    id: "r2",
    scope: "platform",
    name: "security_cleared",
    description: "Security clearance",
    permissionCount: 0,
    assigneeCount: 0,
    inheritedPermissions: [],
  },
];

const GROUPS: accessApi.AccessGroup[] = [
  { id: "g1", name: "Security Team", source: "local", memberCount: 4, roles: [] },
  { id: "g2", name: "Finance Team", source: "local", memberCount: 6, roles: [] },
];

function mockUseAuth(me: Me | null = ADMIN_ME) {
  vi.mocked(useAuth).mockReturnValue({ me, isLoading: false });
}

function makeQueryError(status: number, message = "error") {
  const err = new Error(message) as Error & { status: number };
  err.status = status;
  return err;
}

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <Classifications />
    </QueryClientProvider>,
  );
}

function mockTagsQuery(overrides: Partial<ReturnType<typeof tagsApi.useTags>> = {}) {
  vi.mocked(tagsApi.useTags).mockReturnValue({
    data: TAGS,
    isLoading: false,
    isError: false,
    error: null,
    ...overrides,
  } as ReturnType<typeof tagsApi.useTags>);
}

describe("Classifications", () => {
  beforeEach(() => {
    mockUseAuth();
    mockTagsQuery();
    vi.mocked(tagsApi.useCreateTag).mockReturnValue({
      mutateAsync: vi.fn().mockResolvedValue(TAGS[0]),
      isPending: false,
    } as unknown as ReturnType<typeof tagsApi.useCreateTag>);
    vi.mocked(tagsApi.useUpdateTag).mockReturnValue({
      mutateAsync: vi.fn().mockResolvedValue(TAGS[0]),
      isPending: false,
    } as unknown as ReturnType<typeof tagsApi.useUpdateTag>);
    vi.mocked(tagsApi.useDeleteTag).mockReturnValue({
      mutate: vi.fn(),
      mutateAsync: vi.fn().mockResolvedValue(undefined),
      isPending: false,
    } as unknown as ReturnType<typeof tagsApi.useDeleteTag>);
    vi.mocked(tagsApi.useSetTagAccessPolicy).mockReturnValue({
      mutateAsync: vi.fn().mockResolvedValue(undefined),
      isPending: false,
    } as unknown as ReturnType<typeof tagsApi.useSetTagAccessPolicy>);
    vi.mocked(tagsApi.useClearTagAccessPolicy).mockReturnValue({
      mutateAsync: vi.fn().mockResolvedValue(undefined),
      isPending: false,
    } as unknown as ReturnType<typeof tagsApi.useClearTagAccessPolicy>);
    vi.mocked(accessApi.listRoles).mockResolvedValue(ROLES);
    vi.mocked(accessApi.listGroups).mockResolvedValue(GROUPS);
  });

  test("renders tags in the table with name, key, description, and assignment count", async () => {
    renderPage();

    expect(await screen.findByText("PII")).toBeInTheDocument();
    expect(screen.getByText("pii")).toBeInTheDocument();
    expect(screen.getByText("Personally identifiable information")).toBeInTheDocument();
    expect(screen.getByText("3")).toBeInTheDocument();

    expect(screen.getByText("Internal")).toBeInTheDocument();
    expect(screen.getByText("internal")).toBeInTheDocument();
  });

  test('"New tag" opens a create dialog and submitting calls useCreateTag with the entered fields', async () => {
    const user = userEvent.setup();
    const mutateAsync = vi.fn().mockResolvedValue(TAGS[0]);
    vi.mocked(tagsApi.useCreateTag).mockReturnValue({
      mutateAsync,
      isPending: false,
    } as unknown as ReturnType<typeof tagsApi.useCreateTag>);

    renderPage();
    await screen.findByText("PII");

    await user.click(screen.getByRole("button", { name: /New tag/i }));
    await screen.findByRole("heading", { name: /New tag/i });

    await user.type(screen.getByLabelText("Key"), "confidential");
    await user.type(screen.getByLabelText("Name"), "Confidential");
    await user.type(screen.getByLabelText("Description"), "Highly sensitive data");

    await user.click(screen.getByRole("button", { name: /^Create$/i }));

    await waitFor(() =>
      expect(mutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({
          key: "confidential",
          name: "Confidential",
          description: "Highly sensitive data",
        }),
      ),
    );
  });

  test("deleting a tag via ConfirmDeleteDialog calls useDeleteTag", async () => {
    const user = userEvent.setup();
    const mutate = vi.fn();
    vi.mocked(tagsApi.useDeleteTag).mockReturnValue({
      mutate,
      mutateAsync: vi.fn(),
      isPending: false,
    } as unknown as ReturnType<typeof tagsApi.useDeleteTag>);

    renderPage();
    await screen.findByText("PII");

    await user.click(screen.getByRole("button", { name: /Actions for PII/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));

    await screen.findByRole("heading", { name: /Delete "PII"\?/i });
    await user.type(screen.getByPlaceholderText("delete"), "delete");
    await user.click(screen.getByRole("button", { name: /^Delete$/i }));

    await waitFor(() => expect(mutate).toHaveBeenCalledWith("t1"));
  });

  test("shows a 403 message when the viewer lacks tag:read", async () => {
    mockTagsQuery({
      data: undefined,
      isLoading: false,
      isError: true,
      error: makeQueryError(403),
    });

    renderPage();

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/You need the/i);
    expect(alert).toHaveTextContent("tag:read");
  });

  test("shows a loading skeleton while tags load", () => {
    mockTagsQuery({ data: undefined, isLoading: true, isError: false, error: null });
    renderPage();
    expect(screen.getByTestId("list-skeleton")).toBeInTheDocument();
  });

  test("shows an empty state when there are no tags", async () => {
    mockTagsQuery({ data: [], isLoading: false, isError: false, error: null });
    renderPage();
    expect(await screen.findByText("No tags yet.")).toBeInTheDocument();
  });

  test("a non-admin viewer without tag:manage sees no New tag button and no Actions menu", async () => {
    mockUseAuth({
      user_id: "2",
      email: "viewer@example.com",
      platform_roles: [],
      permissions: ["tag:read"],
    });
    renderPage();

    expect(await screen.findByText("PII")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /New tag/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Actions for PII/i })).not.toBeInTheDocument();
  });

  test("a viewer with tag:manage (but not platform_admin) sees write actions", async () => {
    mockUseAuth({
      user_id: "3",
      email: "steward@example.com",
      platform_roles: [],
      permissions: ["tag:manage"],
    });
    renderPage();

    expect(await screen.findByText("PII")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /New tag/i })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Actions for PII/i })).toBeInTheDocument();
  });

  test("a tag with required_roles shows a gated badge in the table", async () => {
    renderPage();

    expect(await screen.findByText("Finance")).toBeInTheDocument();
    expect(screen.getByText(/Gated.*finance_cleared/i)).toBeInTheDocument();
  });

  test("a tag with empty required_roles shows no gated badge", async () => {
    renderPage();

    await screen.findByText("PII");
    // "PII" and "Internal" both have required_roles: [] — no gated badge
    // anywhere in their row. Only the "Finance" row (required_roles: [...])
    // should render one.
    expect(screen.queryAllByText(/Gated/i)).toHaveLength(1);
  });

  test("editing a tag, selecting a role, and saving calls useSetTagAccessPolicy with the selected roles", async () => {
    const user = userEvent.setup();
    const mutateAsync = vi.fn().mockResolvedValue(undefined);
    vi.mocked(tagsApi.useSetTagAccessPolicy).mockReturnValue({
      mutateAsync,
      isPending: false,
    } as unknown as ReturnType<typeof tagsApi.useSetTagAccessPolicy>);

    renderPage();
    await screen.findByText("PII");

    await user.click(screen.getByRole("button", { name: /Actions for PII/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Edit" }));
    await screen.findByRole("heading", { name: /Edit tag — PII/i });

    const roleCheckbox = await screen.findByRole("checkbox", { name: "finance_cleared" });
    await user.click(roleCheckbox);

    await user.click(screen.getByRole("button", { name: /^Save$/i }));

    await waitFor(() =>
      expect(mutateAsync).toHaveBeenCalledWith({
        id: "t1",
        required_roles: ["finance_cleared"],
        required_groups: [],
      }),
    );
  });

  test("a tag with required_groups (and no required_roles) shows a gated badge with the group name", async () => {
    mockTagsQuery({ data: [GROUP_GATED_TAG] });
    renderPage();

    expect(await screen.findByText("Security")).toBeInTheDocument();
    expect(await screen.findByText(/Gated.*Security Team/i)).toBeInTheDocument();
  });

  test("a tag gated by both a role and a group shows both in the badge", async () => {
    mockTagsQuery({ data: [BOTH_GATED_TAG] });
    renderPage();

    expect(await screen.findByText("BothGated")).toBeInTheDocument();
    const badge = await screen.findByText(/Gated.*Security Team/i);
    expect(badge).toHaveTextContent("finance_cleared");
    expect(badge).toHaveTextContent("Security Team");
  });

  test("editing a tag, selecting a group, and saving calls useSetTagAccessPolicy with the selected roles and groups", async () => {
    const user = userEvent.setup();
    const mutateAsync = vi.fn().mockResolvedValue(undefined);
    vi.mocked(tagsApi.useSetTagAccessPolicy).mockReturnValue({
      mutateAsync,
      isPending: false,
    } as unknown as ReturnType<typeof tagsApi.useSetTagAccessPolicy>);

    renderPage();
    await screen.findByText("PII");

    await user.click(screen.getByRole("button", { name: /Actions for PII/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Edit" }));
    await screen.findByRole("heading", { name: /Edit tag — PII/i });

    const groupCheckbox = await screen.findByRole("checkbox", { name: "Security Team" });
    await user.click(groupCheckbox);

    await user.click(screen.getByRole("button", { name: /^Save$/i }));

    await waitFor(() =>
      expect(mutateAsync).toHaveBeenCalledWith({
        id: "t1",
        required_roles: [],
        required_groups: ["g1"],
      }),
    );
  });
});
