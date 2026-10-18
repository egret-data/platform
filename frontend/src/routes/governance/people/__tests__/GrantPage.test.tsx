import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

/**
 * `/governance/grants/:id` (spec §2 "Single items", P8): one grant, as a
 * sentence with its principals linked, its scope and Open in Data, and Edit
 * and Delete with `policy:update`. The generated perm client is mocked, as the
 * Data permissions list's tests mocked it (this page carries those tests now,
 * for one grant). `GrantDialog` is a stand-in that says which grant it was
 * opened on; its own behaviour is `GrantDialog.test.tsx`'s.
 */
vi.mock("@/api/generated/perm/perm");
vi.mock("@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry");
vi.mock("@/api/generated/control-plane-governance/control-plane-governance");
vi.mock("@/routes/governance/GrantDialog", async () => {
  const actual = await vi.importActual<typeof import("@/routes/governance/GrantDialog")>("@/routes/governance/GrantDialog");
  return {
    ...actual,
    GrantDialog: ({ open, existing }: { open: boolean; existing: { id: string } | null }) =>
      open ? <div>{`editing ${existing?.id ?? "nothing"}`}</div> : null,
  };
});

import * as permApi from "@/api/generated/perm/perm";
import * as registry from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import * as govApi from "@/api/generated/control-plane-governance/control-plane-governance";
import type { Me } from "@/api/hooks";
import type { GrantView } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { AuthContext } from "@/auth/useAuth";
import GrantPage from "@/routes/governance/people/GrantPage";

const ADMIN: Me = { user_id: "u1", email: "a@example.com", platform_roles: ["platform_admin"], permissions: [] };
const WRITER: Me = { user_id: "u3", email: "w@example.com", platform_roles: [], permissions: ["policy:read", "policy:update"] };
const READER: Me = { user_id: "u2", email: "r@example.com", platform_roles: [], permissions: ["policy:read"] };

/**
 * A grant as the read surface hands it over, `catalog_name` included: the
 * server resolves it (SP4b-2 finding 1), so the page needs no catalog lookup
 * to word the sentence.
 */
function grant(over: Partial<GrantView> = {}): GrantView {
  return {
    id: "g1",
    effect: "allow",
    actions: ["read"],
    scope: { catalog_id: "cat-1", schema: "sales" },
    principals: { include: [{ kind: "role", ref: "analyst" }], exclude: [] },
    origin: { type: "explicit" },
    catalog_name: "wh",
    ...over,
  };
}

function serveGrants(rows: GrantView[]) {
  vi.mocked(permApi.usePermListGrants).mockReturnValue({
    data: { status: 200, data: rows },
    isLoading: false,
    isError: false,
    error: null,
    refetch: vi.fn(),
  } as unknown as ReturnType<typeof permApi.usePermListGrants>);
}

let deleteMutate: ReturnType<typeof vi.fn>;

function renderPage(id = "g1", me: Me | null = ADMIN) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={[`/governance/grants/${id}`]}>
          <Routes>
            <Route path="/governance/grants/:id" element={<GrantPage />} />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

const heading = () => screen.getByRole("heading", { level: 1 });

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(govApi.useControlPlaneGovernanceListCatalogs).mockReturnValue({
    data: { status: 200, data: [{ id: "cat-1", name: "wh", workspaces: [{ id: "ws9", name: "Analytics", slug: "analytics" }] }] },
  } as unknown as ReturnType<typeof govApi.useControlPlaneGovernanceListCatalogs>);
  deleteMutate = vi.fn();
  vi.mocked(permApi.usePermDeleteGrant).mockReturnValue({
    mutate: deleteMutate,
  } as unknown as ReturnType<typeof permApi.usePermDeleteGrant>);
});

describe("GrantPage: the sentence", () => {
  test("an allow grant reads as a sentence, each principal linked to its People page", () => {
    serveGrants([grant(), grant({ id: "g2", principals: { include: [{ kind: "user", ref: "zed" }], exclude: [] } })]);
    renderPage();
    expect(heading()).toHaveTextContent("role analyst can read wh.sales.*");
    expect(within(heading()).getByRole("link", { name: "analyst" })).toHaveAttribute(
      "href",
      "/governance/people/roles/analyst",
    );
    // The page shows the grant its URL names, not another one in the list.
    expect(screen.queryByText(/zed/)).not.toBeInTheDocument();
    expect(screen.getByText("Allow")).toBeInTheDocument();
  });

  test("a deny grant reads with 'cannot', and is marked Deny", () => {
    serveGrants([grant({ effect: "deny" })]);
    renderPage();
    expect(heading()).toHaveTextContent("role analyst cannot read wh.sales.*");
    expect(screen.getByText("Deny")).toBeInTheDocument();
  });

  test("an exclude is part of the sentence, because it changes its meaning, and links too", () => {
    serveGrants([
      grant({
        actions: ["read", "write"],
        principals: { include: [{ kind: "role", ref: "analyst" }], exclude: [{ kind: "group", ref: "contractors" }] },
      }),
    ]);
    renderPage();
    expect(heading()).toHaveTextContent("role analyst can read and write wh.sales.* except group contractors");
    expect(within(heading()).getByRole("link", { name: "contractors" })).toHaveAttribute(
      "href",
      "/governance/people/groups/contractors",
    );
  });

  // Live check D5: the sentence read `workspace_members 220f80e1-… can read ice.*`.
  test("a workspace's members read by the workspace's name, and by its id only when it is unknown", () => {
    serveGrants([
      grant({
        scope: { catalog_id: "cat-1" },
        principals: { include: [{ kind: "workspace_members", ref: "ws9" }], exclude: [] },
        origin: { type: "workspace_assignment", workspace_id: "ws9" },
      }),
      grant({
        id: "g6",
        scope: { catalog_id: "cat-1" },
        principals: { include: [{ kind: "workspace_members", ref: "ws-gone" }], exclude: [] },
        origin: { type: "workspace_assignment", workspace_id: "ws-gone" },
      }),
    ]);
    const { unmount } = renderPage("g1");
    expect(heading()).toHaveTextContent("members of workspace Analytics can read wh.*");
    expect(screen.getByText("Set by workspace assignment · Analytics")).toBeInTheDocument();
    expect(screen.queryByText(/ws9/)).not.toBeInTheDocument();
    unmount();
    renderPage("g6");
    expect(heading()).toHaveTextContent("members of workspace ws-gone can read wh.*");
  });
});

describe("GrantPage: the sentence must not need a second permission", () => {
  /**
   * SP4b-2 finding 1. `GET /control-plane/catalogs` gates on
   * `catalog_registry:read`, this page on `policy:read`; here the registry
   * answers 403, so the name can only come from `GrantView.catalog_name`.
   */
  test("the catalog's name renders when the catalog registry answers 403", () => {
    vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: new ApiError(403, "forbidden"),
    } as unknown as ReturnType<typeof registry.useControlPlaneCatalogRegistryListRegistryCatalogs>);
    serveGrants([grant({ scope: { catalog_id: "cat-1", schema: "sales", table: "orders" } })]);
    renderPage("g1", READER);
    expect(heading()).toHaveTextContent("role analyst can read wh.sales.orders");
    expect(screen.queryByText(/cat-1/)).not.toBeInTheDocument();
  });

  test("does not call the catalog registry at all", () => {
    serveGrants([grant()]);
    renderPage();
    expect(heading()).toHaveTextContent("role analyst can read wh.sales.*");
    expect(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).not.toHaveBeenCalled();
  });

  test("with no name on the payload the id is shown, never a guess", () => {
    serveGrants([grant({ catalog_name: null })]);
    renderPage();
    expect(heading()).toHaveTextContent("role analyst can read cat-1.sales.*");
  });
});

describe("GrantPage: scope and Open in Data", () => {
  test("the scope is stated, and a schema grant opens that schema's Permissions tab", () => {
    serveGrants([grant()]);
    renderPage();
    expect(screen.getByText("Scope").nextElementSibling).toHaveTextContent("wh.sales.*");
    expect(screen.getByRole("link", { name: /Open in Data/ })).toHaveAttribute(
      "href",
      "/governance/data/wh/sales?tab=permissions",
    );
  });

  test("a column grant opens the table with ?column=", () => {
    serveGrants([grant({ scope: { catalog_id: "cat-1", schema: "sales", table: "orders", column: "email" } })]);
    renderPage();
    expect(screen.getByRole("link", { name: /Open in Data/ })).toHaveAttribute(
      "href",
      "/governance/data/wh/sales/orders?column=email&tab=permissions",
    );
  });

  test("a pattern table opens the schema, says so, and says how far the grant reaches", () => {
    serveGrants([grant({ scope: { catalog_id: "cat-1", schema: "sales", table: "*" } })]);
    renderPage();
    const link = screen.getByRole("link", { name: /nearest level above its pattern/ });
    expect(link).toHaveAttribute("href", "/governance/data/wh/sales?tab=permissions");
    expect(screen.getByText("Covers every table matching “*”, not one object.")).toBeInTheDocument();
  });

  test("a catalog the Data area does not list gets no link", () => {
    serveGrants([grant({ scope: { catalog_id: "other", schema: "sales" } })]);
    renderPage();
    expect(screen.queryByRole("link", { name: /Open in Data/ })).not.toBeInTheDocument();
  });
});

describe("GrantPage: Edit and Delete", () => {
  test("policy:update offers Edit and Delete on an explicit grant", () => {
    serveGrants([grant()]);
    renderPage("g1", WRITER);
    expect(screen.getByRole("button", { name: "Edit" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete" })).toBeInTheDocument();
  });

  test("policy:read alone offers neither, and says what editing needs", () => {
    serveGrants([grant()]);
    renderPage("g1", READER);
    expect(screen.queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
    expect(screen.getByText("Editing or deleting a grant needs the policy:update permission.")).toBeInTheDocument();
  });

  test("a workspace-assignment grant is read-only even for a platform_admin, and says who owns it", () => {
    serveGrants([grant({ origin: { type: "workspace_assignment", workspace_id: "ws9" } })]);
    renderPage("g1", ADMIN);
    expect(screen.getByText("Set by workspace assignment · Analytics")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
    expect(screen.getByText(/The workspace assignment sync writes this grant/)).toBeInTheDocument();
  });

  test("Edit opens the grant dialog on this grant", async () => {
    serveGrants([grant()]);
    renderPage("g1", WRITER);
    await userEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(screen.getByText("editing g1")).toBeInTheDocument();
  });

  test("Delete asks first, then deletes this grant", async () => {
    serveGrants([grant()]);
    renderPage("g1", WRITER);
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    const confirm = await screen.findByRole("alertdialog");
    expect(confirm).toHaveTextContent('Delete "role analyst can read wh.sales.*"?');
    expect(deleteMutate).not.toHaveBeenCalled();
    await userEvent.type(within(confirm).getByRole("textbox"), "delete");
    await userEvent.click(within(confirm).getByRole("button", { name: "Delete" }));
    expect(deleteMutate).toHaveBeenCalledWith({ id: "g1" });
  });
});

describe("GrantPage: states", () => {
  test("an id no grant has says so", () => {
    serveGrants([grant()]);
    renderPage("g-gone");
    expect(screen.getByText("No grant has this id; it may have been deleted.")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "People & groups" })).toHaveAttribute("href", "/governance/people");
  });

  test("a failed read shows its error with a retry, never the missing state", () => {
    vi.mocked(permApi.usePermListGrants).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: new ApiError(500, "boom"),
      refetch: vi.fn(),
    } as unknown as ReturnType<typeof permApi.usePermListGrants>);
    renderPage();
    expect(screen.getByText("The grants could not be loaded. Retry, or reload the page.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
    expect(screen.queryByText(/No grant has this id/)).not.toBeInTheDocument();
  });

  test("loading shows a skeleton, not the missing state", () => {
    vi.mocked(permApi.usePermListGrants).mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      error: null,
    } as unknown as ReturnType<typeof permApi.usePermListGrants>);
    renderPage();
    expect(screen.queryByText(/No grant has this id/)).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 1 })).toHaveTextContent("Grant");
  });
});
