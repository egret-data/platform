import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";

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
      resources: [
        { name: "catalog", excludesSupported: true, mandatory: true },
        { name: "schema", excludesSupported: true, mandatory: false },
        { name: "table", excludesSupported: true, mandatory: false },
        { name: "column", excludesSupported: true, mandatory: false },
      ],
    }),
  // Serves both the PrincipalPicker (who gets to impersonate) and the
  // impersonation resource autocomplete (who may be impersonated).
  listPrincipals: () => Promise.resolve([{ kind: "user", name: "bob", label: "bob" }]),
  resourceLookup: () => Promise.resolve([]),
}));

import PolicyEditorPage from "../PolicyEditorPage";

function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter>{ui}</MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
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
      expect(navigate).toHaveBeenCalledWith("/control-plane/policies/trino"),
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

    // Resource is the default section; only one set of 4 level editors
    // (the primary block) exists until "Add resource" appends another.
    expect(screen.getAllByPlaceholderText("value or * (Enter to add)")).toHaveLength(4);
    fireEvent.click(screen.getByRole("button", { name: /add resource/i }));
    expect(screen.getAllByPlaceholderText("value or * (Enter to add)")).toHaveLength(8);
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

  it("renders a masking editor with mask-type items for kind=masking", async () => {
    routeParams = { service: "trino", kind: "masking" };
    wrap(<PolicyEditorPage />);
    await screen.findByLabelText("Policy name");
    // Kind-specific rules live under the "Masking" left-nav section (a plain
    // button, unmounted by default — Resource is the default section).
    fireEvent.click(screen.getByRole("button", { name: /masking/i }));
    expect(screen.getByLabelText("mask type 0")).toBeInTheDocument();
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

  it("saves a user-impersonation policy with resourceMode + trinouser + forced accesses", async () => {
    wrap(<PolicyEditorPage />);
    fireEvent.change(await screen.findByLabelText("Policy name"), {
      target: { value: "allow-impersonate-bob" },
    });

    fireEvent.click(screen.getByRole("button", { name: /user impersonation/i }));

    const trinouserInput = screen.getByLabelText("Trino user value");
    fireEvent.change(trinouserInput, { target: { value: "bob" } });
    fireEvent.keyDown(trinouserInput, { key: "Enter" });

    // Give the allow item a principal so it's a non-empty grant. The
    // Permissions cell is hidden in impersonation mode (PermissionTable
    // renders a static "impersonate" badge instead of the picker).
    fireEvent.click(screen.getByRole("button", { name: /permissions/i }));
    fireEvent.change(screen.getByLabelText("user 0"), { target: { value: "alice" } });
    fireEvent.keyDown(screen.getByLabelText("user 0"), { key: "Enter" });

    // The access-type checkbox grid (and its popover) is hidden in
    // impersonation mode.
    expect(screen.queryByText("Add permissions")).not.toBeInTheDocument();
    expect(screen.getByText("impersonate")).toBeInTheDocument();

    await waitFor(() =>
      expect(screen.getByRole("button", { name: /^save$/i })).not.toBeDisabled(),
    );
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() => expect(createPolicy).toHaveBeenCalled());

    const arg = createPolicy.mock.calls[0][0] as {
      resourceMode: string;
      trinouser: { values: string[] };
      items: { users: string[]; accesses: string[] }[];
    };
    expect(arg.resourceMode).toBe("impersonation");
    expect(arg.trinouser.values).toContain("bob");
    expect(arg.items[0].accesses).toEqual(["impersonate"]);
    expect(updatePolicy).not.toHaveBeenCalled();
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
});
