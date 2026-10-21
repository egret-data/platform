import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

/**
 * The picker reads the GENERATED Governance catalog list directly
 * (`GET /governance/catalogs`, `policy:read` or `tag:read`), so that is what
 * is mocked here. The registry list is mocked too, only to prove it is never
 * read: it needs `catalog_registry:read`, which a steward does not hold.
 */
vi.mock("@/api/generated/control-plane-governance/control-plane-governance");
vi.mock("@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry");

import * as governance from "@/api/generated/control-plane-governance/control-plane-governance";
import * as registry from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import type { GovernanceCatalog, Scope } from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { ScopePicker } from "@/components/perm/ScopePicker";

const CATALOGS: GovernanceCatalog[] = [
  {
    id: "11111111-1111-1111-1111-111111111111",
    name: "analytics",
    source: "egret",
    connectorType: "iceberg",
    engines: [],
    warehouseId: "wh-1",
    warehouseName: "analytics_wh",
    workspaces: [],
  },
];

function mockCatalogs(rows: GovernanceCatalog[] = CATALOGS) {
  vi.mocked(governance.useControlPlaneGovernanceListCatalogs).mockReturnValue({
    data: { status: 200, data: rows },
    isLoading: false,
    isError: false,
  } as unknown as ReturnType<typeof governance.useControlPlaneGovernanceListCatalogs>);
}

function renderPicker(props: Partial<React.ComponentProps<typeof ScopePicker>> = {}) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onChange = vi.fn();
  const value: Scope = props.value ?? { catalog_id: "" };
  const utils = render(
    <QueryClientProvider client={qc}>
      <ScopePicker value={value} onChange={onChange} {...props} />
    </QueryClientProvider>,
  );
  return { ...utils, onChange };
}

describe("ScopePicker", () => {
  beforeEach(() => {
    mockCatalogs();
  });

  test("lists catalogs from the Governance list, never the catalog registry", () => {
    renderPicker();
    expect(governance.useControlPlaneGovernanceListCatalogs).toHaveBeenCalled();
    expect(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).not.toHaveBeenCalled();
  });

  test("resolves catalog_id to the catalog's name, not the raw uuid", async () => {
    const user = userEvent.setup();
    renderPicker();

    await user.click(screen.getByRole("combobox", { name: /catalog/i }));
    expect(await screen.findByRole("option", { name: "analytics" })).toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: "11111111-1111-1111-1111-111111111111" }),
    ).not.toBeInTheDocument();
  });

  test("catalogRequired marks the Catalog field required, in its label and to assistive technology", () => {
    renderPicker({ catalogRequired: true });
    expect(screen.getByText("Catalog").closest("label")).toHaveTextContent("Catalog (required)");
    expect(screen.getByRole("combobox", { name: /^Catalog/ })).toHaveAttribute("aria-required", "true");
  });

  test("without catalogRequired the Catalog field reads as before", () => {
    renderPicker();
    expect(screen.getByText("Catalog").closest("label")).toHaveTextContent(/^Catalog$/);
    expect(screen.getByRole("combobox", { name: /^Catalog/ })).not.toHaveAttribute("aria-required");
  });

  test("row-filter mode renders no Column field at all", () => {
    renderPicker({ rowFilterMode: true });
    expect(screen.queryByLabelText("Column")).not.toBeInTheDocument();
  });

  test("outside row-filter mode, a Column field is present (disabled until a table is set)", () => {
    renderPicker({ rowFilterMode: false });
    expect(screen.getByLabelText("Column")).toBeDisabled();
  });

  test("row-filter mode: Apply is absent without a handler, and disabled until catalog+schema+table are all set", () => {
    const onApply = vi.fn();
    const { rerender } = renderPicker({
      rowFilterMode: true,
      value: { catalog_id: "11111111-1111-1111-1111-111111111111", schema: "sales" },
      onApply,
    });

    // Missing `table` — the control that would submit an incomplete scope is
    // disabled, not merely validated after a click.
    expect(screen.getByRole("button", { name: "Apply" })).toBeDisabled();

    rerender(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <ScopePicker
          value={{ catalog_id: "11111111-1111-1111-1111-111111111111", schema: "sales", table: "orders" }}
          onChange={vi.fn()}
          rowFilterMode
          onApply={onApply}
        />
      </QueryClientProvider>,
    );

    expect(screen.getByRole("button", { name: "Apply" })).toBeEnabled();
  });

  test("with no onApply prop, there is no Apply button at all", () => {
    renderPicker({
      value: { catalog_id: "11111111-1111-1111-1111-111111111111" },
    });
    expect(screen.queryByRole("button", { name: "Apply" })).not.toBeInTheDocument();
  });
});

describe("ScopePicker — a picker that cannot be operated says why (SP4b-2 finding 2)", () => {
  /**
   * **THE DISCRIMINATING CASE.** With `policy:read` alone the catalog list
   * comes back 403, so the dropdown holds zero options, everything below it
   * stays disabled and — before this — the page said nothing at all. The
   * browser run measured exactly that on `Explain access`: `optionCount: 0`,
   * `Explain disabled: true`, no message. The permission is NAMED, because
   * "empty" alone tells an administrator nothing about what to fix.
   */
  test("names the missing permission when the catalog list is forbidden", () => {
    vi.mocked(governance.useControlPlaneGovernanceListCatalogs).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: new ApiError(403, "forbidden"),
    } as unknown as ReturnType<typeof governance.useControlPlaneGovernanceListCatalogs>);
    renderPicker();

    const note = screen.getByRole("alert");
    expect(note).toHaveTextContent(/policy:read/);
    expect(note).toHaveTextContent(/tag:read/);
    expect(note).not.toHaveTextContent(/catalog_registry:read/);
    expect(screen.getByLabelText("Catalog")).toHaveAttribute(
      "aria-describedby",
      "scope-catalog-unavailable",
    );
  });

  /**
   * The other empty case must NOT blame the account: an installation with no
   * catalog registered is not a permission problem, and telling a platform
   * admin to ask for a permission they already hold is worse than silence.
   */
  test("an empty registry says there is nothing to name, not that a permission is missing", () => {
    mockCatalogs([]);
    renderPicker();

    const note = screen.getByRole("alert");
    expect(note).toHaveTextContent(/No catalogs are registered yet/);
    expect(note).not.toHaveTextContent(/policy:read/);
  });

  test("with catalogs to pick, no such note is rendered", () => {
    mockCatalogs();
    renderPicker();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});
