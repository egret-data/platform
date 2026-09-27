import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ApiError } from "@/api/client";

const navigate = vi.fn();
let routeParams: Record<string, string | undefined> = { service: "trino", kind: "access" };

vi.mock("react-router-dom", async (orig) => ({
  ...(await orig<typeof import("react-router-dom")>()),
  useParams: () => routeParams,
  useNavigate: () => navigate,
}));

const createPolicy = vi.fn((_input: unknown) => Promise.resolve({ id: 1 }));
const updatePolicy = vi.fn((_id: number, _input: unknown) => Promise.resolve({ id: 1 }));
const deletePolicy = vi.fn(() => Promise.resolve(undefined));
const getPolicy = vi.fn(() => Promise.resolve({}));

vi.mock("@/api/policies", async (orig) => ({
  ...(await orig<typeof import("@/api/policies")>()),
  createPolicy: (i: unknown) => createPolicy(i),
  updatePolicy: (id: number, i: unknown) => updatePolicy(id, i),
  deletePolicy: () => deletePolicy(),
  getPolicy: () => getPolicy(),
  getServiceDef: () =>
    Promise.resolve({
      accessTypes: [{ name: "select" }, { name: "insert" }],
      // catalog.schema.table.column hierarchy plus two standalone roots
      // (role, trinouser) — trinouser restricted to "impersonate" so its
      // Permissions popover naturally offers only that one option.
      resources: [
        { name: "catalog", excludesSupported: true, mandatory: true },
        { name: "schema", parent: "catalog", excludesSupported: true, mandatory: false },
        { name: "table", parent: "schema", excludesSupported: true, mandatory: false },
        { name: "column", parent: "table", excludesSupported: true, mandatory: false },
        { name: "trinouser", accessTypeRestrictions: ["impersonate"] },
        { name: "role" },
      ],
      // Real Ranger defs (trino.json/starrocks.json) ship these subsets as flat
      // NAME-ONLY refs (no parent/level) — the editor enriches them from the
      // top-level `resources` above so ResourceSelector can still cascade.
      dataMaskDef: {
        resources: [
          { name: "catalog" },
          { name: "schema" },
          { name: "table" },
          { name: "column" },
        ],
      },
      rowFilterDef: {
        resources: [{ name: "catalog" }, { name: "schema" }, { name: "table" }],
      },
    }),
  // Serves both the PrincipalPicker and the resource-value autocomplete.
  listPrincipals: () => Promise.resolve([{ kind: "user", name: "bob", label: "bob" }]),
  resourceLookup: () => Promise.resolve([]),
}));

vi.mock("@/api/tags", () => ({
  useTags: () => ({
    data: [
      { id: "t1", key: "pii", name: "pii", description: "", color: "#f00", assignment_count: 0, created_at: "" },
      { id: "t2", key: "phi", name: "phi", description: "", color: "#0f0", assignment_count: 0, created_at: "" },
    ],
    isLoading: false,
  }),
}));

import PolicyEditorPage from "../PolicyEditorPage";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";

const ADMIN: Me = {
  user_id: "u1",
  email: "a@egret.local",
  platform_roles: ["platform_admin"],
  permissions: [],
};
/** The `policy:read`-only persona: can open this editor by URL, cannot write. */
const READER: Me = {
  user_id: "u2",
  email: "r@egret.local",
  platform_roles: [],
  permissions: ["policy:read"],
};

/** Default `me` is the admin, because every test that SAVES is measuring the
 *  admin's page — a measurement taken as `platform_admin` says nothing about a
 *  reader's (SP4b-2 finding 3). */
function wrap(ui: React.ReactNode, me: Me | null = ADMIN) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <TooltipProvider>
          <MemoryRouter>{ui}</MemoryRouter>
        </TooltipProvider>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

describe("PolicyEditorPage", () => {
  beforeEach(() => {
    navigate.mockClear();
    createPolicy.mockClear();
    updatePolicy.mockClear();
    deletePolicy.mockClear();
    getPolicy.mockClear();
    routeParams = { service: "trino", kind: "access" };
  });

  it("creates an access policy from the form (new, no id)", async () => {
    wrap(<PolicyEditorPage />);
    fireEvent.change(await screen.findByLabelText("Policy name"), { target: { value: "p1" } });

    // The four ResourceLevelEditors share the same placeholder; catalog is first in DOM order.
    const catalogInput = screen.getAllByPlaceholderText("value or * (Enter to add)")[0];
    fireEvent.change(catalogInput, { target: { value: "iceberg" } });
    fireEvent.keyDown(catalogInput, { key: "Enter" });

    // Permissions tab: the Allow accordion is open by default with one blank
    // row; grant it via the Permissions popover (PermissionPickerCell) and
    // give it a user through the row's Users TagCombobox.
    fireEvent.click(screen.getByRole("button", { name: /permissions/i }));
    fireEvent.click(screen.getByText("Add permissions"));
    fireEvent.click(screen.getByLabelText("select"));
    fireEvent.click(screen.getByLabelText("Confirm"));
    fireEvent.change(screen.getByLabelText("user 0"), { target: { value: "alice" } });
    fireEvent.keyDown(screen.getByLabelText("user 0"), { key: "Enter" });

    // Catalog is mandatory per the mocked service-def; Save stays disabled
    // until both the def has loaded and catalog is filled.
    await waitFor(() =>
      expect(screen.getByRole("button", { name: /^save$/i })).not.toBeDisabled(),
    );
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() => expect(createPolicy).toHaveBeenCalled());

    const arg = createPolicy.mock.calls[0][0] as {
      kind: string;
      name: string;
      resources: { catalog: { values: string[] } };
      items: { users: string[]; accesses: string[] }[];
    };
    expect(arg.kind).toBe("access");
    expect(arg.name).toBe("p1");
    expect(arg.resources.catalog.values).toEqual(["iceberg"]);
    expect(arg.items[0].accesses).toEqual(["select"]);
    expect(arg.items[0].users).toEqual(["alice"]);
    expect(updatePolicy).not.toHaveBeenCalled();

    await waitFor(() =>
      expect(navigate).toHaveBeenCalledWith("/governance/policies/trino"),
    );
  });

  it("hides the Deny accordion when 'Deny all else' is enabled", async () => {
    wrap(<PolicyEditorPage />);
    await screen.findByLabelText("Policy name");

    // Deny-all-else and the Allow/Deny accordions live under the
    // "Permissions" section of the outer (plain-button) left-nav.
    fireEvent.click(screen.getByRole("button", { name: /permissions/i }));
    expect(screen.getByRole("button", { name: "Deny conditions" })).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("Deny all else"));
    expect(screen.queryByRole("button", { name: "Deny conditions" })).not.toBeInTheDocument();
  });

  it("adds an additional resource block via 'Add resource'", async () => {
    wrap(<PolicyEditorPage />);
    await screen.findByLabelText("Policy name");

    // Resource is the default section; the primary block starts with the full
    // default catalog/schema/table/column chain (4 levels — column is shown by
    // default and the user can drop it via "— none —") once the service-def
    // (which ResourceSelector needs to build the hierarchy) has loaded. "Add
    // resource" appends a blank entry, which ResourceSelector defaults to a
    // single catalog level (nothing chosen yet to cascade further).
    expect(await screen.findAllByPlaceholderText("value or * (Enter to add)")).toHaveLength(4);
    fireEvent.click(screen.getByRole("button", { name: /add resource/i }));
    expect(screen.getAllByPlaceholderText("value or * (Enter to add)")).toHaveLength(5);
    expect(screen.getByRole("button", { name: /remove resource/i })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /remove resource/i }));
    expect(screen.getAllByPlaceholderText("value or * (Enter to add)")).toHaveLength(4);
  });

  it("updates an existing policy when an id param is present", async () => {
    routeParams = { service: "trino", kind: "access", id: "42" };
    getPolicy.mockResolvedValueOnce({
      name: "existing",
      isEnabled: true,
      resources: { catalog: { values: ["iceberg"] } },
      policyItems: [],
      denyPolicyItems: [],
    });

    wrap(<PolicyEditorPage />);
    await screen.findByDisplayValue("existing");

    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() => expect(updatePolicy).toHaveBeenCalledWith(42, expect.any(Object)));
    expect(createPolicy).not.toHaveBeenCalled();
  });

  it("renders the 409 compiler-owned explanation, not a bare error (D10)", async () => {
    routeParams = { service: "trino", kind: "access", id: "42" };
    getPolicy.mockResolvedValueOnce({
      name: "perm-grant-abc123",
      isEnabled: true,
      resources: { catalog: { values: ["iceberg"] } },
      policyItems: [],
      denyPolicyItems: [],
    });
    updatePolicy.mockRejectedValueOnce(
      new ApiError(
        409,
        'conflict: policy "perm-grant-abc123" is compiled by the SP4a permission model — edit or delete that grant/rule instead of writing this policy directly',
      ),
    );

    wrap(<PolicyEditorPage />);
    await screen.findByDisplayValue("perm-grant-abc123");
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));

    expect(await screen.findByText(/owned by the permission model/i)).toBeInTheDocument();
    expect(screen.getByText(/edit or delete that grant\/rule instead/)).toBeInTheDocument();
    // The raw wire prefix must not leak through as a second, redundant "conflict:".
    expect(screen.queryByText(/^conflict:/)).not.toBeInTheDocument();
  });

  it("renders a plain, non-conflict save error unchanged (not mistaken for a 409)", async () => {
    routeParams = { service: "trino", kind: "access", id: "42" };
    getPolicy.mockResolvedValueOnce({
      name: "existing",
      isEnabled: true,
      resources: { catalog: { values: ["iceberg"] } },
      policyItems: [],
      denyPolicyItems: [],
    });
    updatePolicy.mockRejectedValueOnce(new ApiError(500, "internal error"));

    wrap(<PolicyEditorPage />);
    await screen.findByDisplayValue("existing");
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));

    expect(await screen.findByText("internal error")).toBeInTheDocument();
    expect(screen.queryByText(/owned by the permission model/i)).not.toBeInTheDocument();
  });

  it("renders a masking editor with mask-type items for kind=masking", async () => {
    routeParams = { service: "trino", kind: "masking" };
    wrap(<PolicyEditorPage />);
    await screen.findByLabelText("Policy name");
    // Kind-specific rules live under the "Masking" left-nav section (a plain
    // button, unmounted by default — Resource is the default section).
    fireEvent.click(screen.getByRole("button", { name: /masking/i }));
    expect(screen.getByLabelText("mask type 0")).toBeInTheDocument();
  });

  it("renders the full resource cascade for masking even though the mask subset is parentless", async () => {
    // Regression for the parentless-subset bug: dataMaskDef.resources is flat
    // name-only, so without enriching it from the top-level hierarchy the
    // ResourceSelector would render only the catalog level. Enriched, it
    // cascades to catalog/schema/table/column — four resource-value inputs.
    routeParams = { service: "trino", kind: "masking" };
    wrap(<PolicyEditorPage />);
    await screen.findByLabelText("Policy name");
    await waitFor(() =>
      expect(screen.getAllByPlaceholderText("value or * (Enter to add)")).toHaveLength(4),
    );
  });

  it("renders a row-filter editor with a filter-expr input for kind=rowfilter", async () => {
    routeParams = { service: "trino", kind: "rowfilter" };
    wrap(<PolicyEditorPage />);
    await screen.findByLabelText("Policy name");
    // Kind-specific rules live under the "Row filter" left-nav section (a
    // plain button, unmounted by default).
    fireEvent.click(screen.getByRole("button", { name: /row filter/i }));
    expect(screen.getByLabelText("filter 0")).toBeInTheDocument();
  });

  it("creates an access policy whose resource type is trinouser, restricting Permissions to 'impersonate'", async () => {
    wrap(<PolicyEditorPage />);
    fireEvent.change(await screen.findByLabelText("Policy name"), {
      target: { value: "allow-impersonate-bob" },
    });

    // Switch the level-0 resource type from Catalog to Trino user — this
    // truncates the default catalog/schema/table/column chain down to just
    // the trinouser leaf.
    fireEvent.click(screen.getByLabelText("Resource type at level 1"));
    fireEvent.click(screen.getByRole("option", { name: "Trino user" }));

    const trinouserInput = screen.getByLabelText("Trino user value");
    fireEvent.change(trinouserInput, { target: { value: "bob" } });
    fireEvent.keyDown(trinouserInput, { key: "Enter" });

    // Permissions: since the selected leaf is trinouser, its
    // accessTypeRestrictions (["impersonate"]) is the only option offered —
    // no special-cased impersonation UI, just a restricted picker.
    fireEvent.click(screen.getByRole("button", { name: /permissions/i }));
    fireEvent.click(screen.getByText("Add permissions"));
    fireEvent.click(screen.getByLabelText("impersonate"));
    fireEvent.click(screen.getByLabelText("Confirm"));
    fireEvent.change(screen.getByLabelText("user 0"), { target: { value: "alice" } });
    fireEvent.keyDown(screen.getByLabelText("user 0"), { key: "Enter" });

    await waitFor(() =>
      expect(screen.getByRole("button", { name: /^save$/i })).not.toBeDisabled(),
    );
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() => expect(createPolicy).toHaveBeenCalled());

    const arg = createPolicy.mock.calls[0][0] as {
      resources: Record<string, { values: string[] }>;
      items: { users: string[]; accesses: string[] }[];
    };
    expect(arg.resources.trinouser.values).toContain("bob");
    expect(Object.keys(arg.resources)).toEqual(["trinouser"]);
    expect(arg.items[0].accesses).toEqual(["impersonate"]);
    expect(updatePolicy).not.toHaveBeenCalled();
  });

  it("renders the trinouser resource (not empty catalog inputs) when editing an existing trinouser-based policy", async () => {
    routeParams = { service: "trino", kind: "access", id: "7" };
    getPolicy.mockResolvedValueOnce({
      name: "existing-impersonation",
      isEnabled: true,
      resources: { trinouser: { values: ["*"] } },
      policyItems: [],
      denyPolicyItems: [],
    });

    wrap(<PolicyEditorPage />);
    await screen.findByDisplayValue("existing-impersonation");

    // Only one resource-value input renders (the trinouser leaf) — not the
    // 4-level catalog/schema/table/column chain a fresh policy starts with.
    expect(await screen.findAllByPlaceholderText("value or * (Enter to add)")).toHaveLength(1);
    expect(screen.getByText("*")).toBeInTheDocument();
    expect(screen.getAllByText("Trino user").length).toBeGreaterThan(0);
  });

  it("renders the Override switch (unchecked by default) in the Details section", async () => {
    wrap(<PolicyEditorPage />);
    await screen.findByLabelText("Policy name");
    // Override lives under the "Details" left-nav section (a plain button,
    // unmounted by default — Resource is the default section).
    fireEvent.click(screen.getByRole("button", { name: /details/i }));
    const override = screen.getByLabelText("Override") as HTMLElement;
    expect(override).toBeInTheDocument();
    expect(override).not.toBeChecked();
  });

  it("toggling to Tag mode shows the tag select and hides the resource picker", async () => {
    wrap(<PolicyEditorPage />);
    await screen.findByLabelText("Policy name");

    // Resource mode (default) renders the catalog/schema/table/column chain.
    expect((await screen.findAllByPlaceholderText("value or * (Enter to add)")).length).toBeGreaterThan(0);

    fireEvent.click(screen.getByRole("button", { name: /by tag/i }));

    expect(screen.queryByPlaceholderText("value or * (Enter to add)")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Tag")).toBeInTheDocument();
  });

  it("submits a tag + masking policy with kind:tag-masking, tag, service, and mask items", async () => {
    routeParams = { service: "trino", kind: "masking" };
    wrap(<PolicyEditorPage />);
    fireEvent.change(await screen.findByLabelText("Policy name"), {
      target: { value: "mask-pii" },
    });

    fireEvent.click(screen.getByRole("button", { name: /by tag/i }));
    fireEvent.click(screen.getByLabelText("Tag"));
    fireEvent.click(await screen.findByRole("option", { name: "pii" }));

    await waitFor(() =>
      expect(screen.getByRole("button", { name: /^save$/i })).not.toBeDisabled(),
    );
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() => expect(createPolicy).toHaveBeenCalled());

    const arg = createPolicy.mock.calls[0][0] as {
      kind: string;
      tag: string;
      service: string;
      name: string;
      items: unknown[];
    };
    expect(arg.kind).toBe("tag-masking");
    expect(arg.tag).toBe("pii");
    expect(arg.service).toBe("trino");
    expect(arg.name).toBe("mask-pii");
    expect(arg.items.length).toBeGreaterThan(0);
    expect(updatePolicy).not.toHaveBeenCalled();
  });

  it("submits kind:masking with resources in Resource mode (no regression)", async () => {
    routeParams = { service: "trino", kind: "masking" };
    wrap(<PolicyEditorPage />);
    fireEvent.change(await screen.findByLabelText("Policy name"), {
      target: { value: "mask-catalog" },
    });

    const catalogInput = screen.getAllByPlaceholderText("value or * (Enter to add)")[0];
    fireEvent.change(catalogInput, { target: { value: "iceberg" } });
    fireEvent.keyDown(catalogInput, { key: "Enter" });

    await waitFor(() =>
      expect(screen.getByRole("button", { name: /^save$/i })).not.toBeDisabled(),
    );
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() => expect(createPolicy).toHaveBeenCalled());

    const arg = createPolicy.mock.calls[0][0] as {
      kind: string;
      tag?: string;
      resources: { catalog: { values: string[] } };
    };
    expect(arg.kind).toBe("masking");
    expect(arg.resources.catalog.values).toEqual(["iceberg"]);
    expect(arg.tag).toBeUndefined();
  });
});

describe("PolicyEditorPage — a reader can read it but not save it (review I-1)", () => {
  beforeEach(() => {
    routeParams = { service: "trino", kind: "access" };
  });

  /**
   * **THE DISCRIMINATING CASE.** This editor is reachable by URL for anyone who
   * can READ a policy — the browser run opened it that way — so `Save` was
   * offered to an account whose write 403s. Disabled WITH a reason rather than
   * hidden: the form is a legitimate read for a `policy:read` holder, and a
   * missing button would leave the page looking broken instead of explaining
   * itself.
   */
  it("disables Save and says which permission is missing", async () => {
    wrap(<PolicyEditorPage />, READER);
    await screen.findByLabelText("Policy name");

    const save = screen.getByRole("button", { name: "Save" });
    expect(save).toBeDisabled();
    expect(save).toHaveAttribute("title", "Saving needs the policy:create permission.");
    expect(screen.getByText(/read-only for this account/)).toHaveTextContent(/policy:create/);
  });

  /** Editing an EXISTING policy is a different route and a different key. */
  it("names policy:update, not policy:create, when editing an existing policy", async () => {
    routeParams = { service: "trino", kind: "access", id: "42" };
    wrap(<PolicyEditorPage />, READER);
    await screen.findByLabelText("Policy name");

    expect(screen.getByRole("button", { name: "Save" })).toHaveAttribute(
      "title",
      "Saving needs the policy:update permission.",
    );
    // …and Delete, a third key again, is not offered at all.
    expect(screen.queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
  });

  /** The same page for an admin: Save enabled once the form is valid, Delete
   *  offered, and no read-only notice — otherwise a fix that disabled Save for
   *  everybody would pass the two tests above. */
  it("still lets a platform_admin save and delete", async () => {
    routeParams = { service: "trino", kind: "access", id: "42" };
    wrap(<PolicyEditorPage />);
    await screen.findByLabelText("Policy name");

    expect(screen.queryByText(/read-only for this account/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Delete" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save" })).not.toHaveAttribute(
      "title",
      expect.stringContaining("permission"),
    );
  });
});
