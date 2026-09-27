import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

/**
 * The picker reads the GENERATED registry client directly (no hand-written
 * wrapper — see `Catalogs.test.tsx`, the newest list page, for the same
 * pattern), so that is what is mocked here.
 */
vi.mock("@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry");

import * as registry from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import type { RegistryCatalogDto, Scope } from "@/api/generated/model";
import { ScopePicker } from "@/components/perm/ScopePicker";

const CATALOGS: RegistryCatalogDto[] = [
  {
    id: "11111111-1111-1111-1111-111111111111",
    name: "analytics",
    kind: "iceberg",
    config: {} as RegistryCatalogDto["config"],
    advanced: {} as RegistryCatalogDto["advanced"],
    warehouseId: "wh-1",
    engines: [],
    workspaceIds: [],
    createdAt: "2026-09-01T00:00:00Z",
    updatedAt: "2026-09-01T00:00:00Z",
  },
];

function mockCatalogs(rows: RegistryCatalogDto[] = CATALOGS) {
  vi.mocked(registry.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue({
    data: { status: 200, data: rows },
    isLoading: false,
    isError: false,
  } as unknown as ReturnType<typeof registry.useControlPlaneCatalogRegistryListRegistryCatalogs>);
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

  test("resolves catalog_id to the catalog's name, not the raw uuid", async () => {
    const user = userEvent.setup();
    renderPicker();

    await user.click(screen.getByRole("combobox", { name: /catalog/i }));
    expect(await screen.findByRole("option", { name: "analytics" })).toBeInTheDocument();
    expect(
      screen.queryByRole("option", { name: "11111111-1111-1111-1111-111111111111" }),
    ).not.toBeInTheDocument();
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
