import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

import Classifications from "@/routes/governance/Classifications";
import * as tagsApi from "@/api/tags";
import * as accessApi from "@/api/access";
import { useAuth } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";
import { toast } from "sonner";
import { ApiError } from "@/api/client";

vi.mock("@/api/tags");
vi.mock("@/api/access");
vi.mock("@/auth/useAuth");
vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }));

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
      <MemoryRouter>
        <Classifications />
      </MemoryRouter>
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
    vi.clearAllMocks();
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
    // Assignment count in PII's row (scoped — the SummaryStrip also shows totals).
    const piiRow = screen.getByText("PII").closest("tr");
    expect(piiRow).toHaveTextContent("3");

    expect(screen.getByText("Internal")).toBeInTheDocument();
    expect(screen.getByText("internal")).toBeInTheDocument();
  });

  // View governance Task 12 (live finding M-6): `assignment_count` counts BI
  // objects only, never catalog columns (base or derived from a view's
  // lineage), so the header said "0 tagged objects" while columns carried the
  // tag. The labels name what is counted.
  test("the object counts say they count BI objects, not catalog columns", async () => {
    renderPage();
    await screen.findByText("PII");
    expect(screen.getByText(/tagged BI objects/)).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: /BI objects/ })).toBeInTheDocument();
  });

  test("links each tag name to its Governance detail page", async () => {
    renderPage();
    expect(await screen.findByRole("link", { name: "PII" })).toHaveAttribute("href", "/governance/classifications/t1");
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

  test("the New-tag key auto-fills (slugified) from the name until edited by hand", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("PII");

    await user.click(screen.getByRole("button", { name: /New tag/i }));
    await screen.findByRole("heading", { name: /New tag/i });

    await user.type(screen.getByLabelText("Name"), "Personally Identifiable Info");
    expect(screen.getByLabelText("Key")).toHaveValue("personally-identifiable-info");

    // Editing the key by hand stops the auto-derive.
    await user.clear(screen.getByLabelText("Key"));
    await user.type(screen.getByLabelText("Key"), "pii");
    await user.type(screen.getByLabelText("Name"), "!");
    expect(screen.getByLabelText("Key")).toHaveValue("pii");
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

    await waitFor(() => expect(mutate).toHaveBeenCalledWith("t1", expect.anything()));
  });

  // Live check D4: perm.rules.tag_key references tags(key), so the server refuses with 409 while a
  // rule selects the tag. The dialog used to close with nothing said and the tag still listed.
  test("a refused delete shows the server's sentence and points to the tag's rule list", async () => {
    const user = userEvent.setup();
    const sentence = "tag pii is still selected by a permission rule; delete or retarget that rule first";
    const mutate = vi.fn((_id: string, opts?: { onError?: (e: Error) => void }) =>
      opts?.onError?.(new ApiError(409, sentence)),
    );
    vi.mocked(tagsApi.useDeleteTag).mockReturnValue({ mutate, isPending: false } as never);
    mockUseAuth({ user_id: "3", email: "s@example.com", platform_roles: [], permissions: ["tag:manage", "policy:read"] });

    renderPage();
    await user.click(await screen.findByRole("button", { name: /Actions for PII/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    await user.type(screen.getByPlaceholderText("delete"), "delete");
    await user.click(screen.getByRole("button", { name: /^Delete$/i }));

    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
    const [message, opts] = vi.mocked(toast.error).mock.calls[0] as unknown as [string, { description: string; action: { label: string } }];
    expect(message).toBe(sentence);
    expect(opts.description).toBe(
      'The "Mask and filter rules" section on the tag\'s page lists the rules that select this tag.',
    );
    expect(opts.action.label).toBe("Show the rules");
    expect(toast.success).not.toHaveBeenCalled();
    expect(screen.getByText("PII")).toBeInTheDocument();
  });

  test("a refused delete for any other reason shows the server's sentence alone", async () => {
    const user = userEvent.setup();
    const mutate = vi.fn((_id: string, opts?: { onError?: (e: Error) => void }) =>
      opts?.onError?.(new ApiError(500, "database unavailable")),
    );
    vi.mocked(tagsApi.useDeleteTag).mockReturnValue({ mutate, isPending: false } as never);

    renderPage();
    await user.click(await screen.findByRole("button", { name: /Actions for PII/i }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    await user.type(screen.getByPlaceholderText("delete"), "delete");
    await user.click(screen.getByRole("button", { name: /^Delete$/i }));

    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("database unavailable"));
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
    // Concise badge ("Gated · 1 role") with the full names in its tooltip.
    // Match the capital-G badge (not the lowercase "gated" summary label).
    const badge = screen.getByText(/Gated/);
    expect(badge).toHaveTextContent("1 role");
    expect(badge.getAttribute("title")).toContain("finance_cleared");
  });

  test("a tag with empty required_roles shows no gated badge", async () => {
    renderPage();

    await screen.findByText("PII");
    // "PII" and "Internal" both have required_roles: [] — no gated badge
    // anywhere in their row. Only the "Finance" row (required_roles: [...])
    // should render one.
    expect(screen.queryAllByText(/Gated/)).toHaveLength(1);
  });

  test("a tag with required_groups (and no required_roles) shows a gated badge with the group name", async () => {
    mockTagsQuery({ data: [GROUP_GATED_TAG] });
    renderPage();

    expect(await screen.findByText("Security")).toBeInTheDocument();
    const badge = await screen.findByText(/Gated/);
    expect(badge).toHaveTextContent("1 group");
    expect(badge.getAttribute("title")).toContain("Security Team");
  });

  test("a tag gated by both a role and a group shows both in the badge", async () => {
    mockTagsQuery({ data: [BOTH_GATED_TAG] });
    renderPage();

    expect(await screen.findByText("BothGated")).toBeInTheDocument();
    // Match the badge (middle-dot) — this tag's description also contains "Gated".
    const badge = await screen.findByText(/Gated ·/);
    expect(badge).toHaveTextContent("1 role, 1 group");
    expect(badge.getAttribute("title")).toContain("finance_cleared");
    expect(badge.getAttribute("title")).toContain("Security Team");
  });

});
