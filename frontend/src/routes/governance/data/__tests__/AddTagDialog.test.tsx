import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Me } from "@/api/hooks";
import { AuthContext } from "@/auth/useAuth";
import { AddTagDialog, type TagTarget } from "@/routes/governance/data/AddTagDialog";
import { serve, ok, fail } from "@/routes/governance/engines/__tests__/engineHarness";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

afterEach(() => {
  vi.unstubAllGlobals();
  toast.success.mockClear();
});

const PII = { id: "t-pii", key: "pii", name: "PII", description: "Personal data", color: "#e11d48", assignment_count: 0, created_at: "", required_roles: [], required_groups: [] };
const HASH = { id: "r1", name: "Hash PII", kind: { type: "mask", mask_type: "MASK_HASH" }, selector: { type: "tag", key: "pii" }, exempt: { include: [], exclude: [] }, tag_name: "PII" };
const TABLE: TagTarget = { catalog: "ice", catalogId: "c-ice", warehouseId: "wh-1", schema: "sales", table: "orders", assigned: [], columns: ["id", "email"] };
const me = (permissions: string[]): Me => ({ user_id: "u", email: "u@x", platform_roles: [], permissions });
const CONFLICT = {
  error: 'tag "PII" is attached…',
  code: "table_tag_mask_conflict",
  tag: "PII",
  tagId: "t-pii",
  warehouseId: null,
  schema: "sales",
  table: "orders",
  mask: "Hash PII",
  maskOrigin: "rule",
  moreTables: false,
};

function open(target: TagTarget, as: Me) {
  const onOpenChange = vi.fn();
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <AuthContext.Provider value={{ me: as, isLoading: false }}>
        <MemoryRouter>
          <AddTagDialog target={target} onOpenChange={onOpenChange} />
        </MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  return onOpenChange;
}

async function pick(name: string) {
  await userEvent.click(await screen.findByRole("combobox", { name: "Classification" }));
  await userEvent.click(await screen.findByRole("option", { name: new RegExp(name) }));
}

describe("Add classification on a whole table", () => {
  it("warns before submit when a mask uses the classification, and attaches to columns instead", async () => {
    const served = serve({ "/tags": ok([PII]), "/perm/rules": ok([HASH]), "PUT /warehouses/wh-1/resource-tags": ok({ ok: true }) });
    open(TABLE, me(["tag:read", "policy:read", "catalog_registry:update"]));
    await pick("PII");
    expect(await screen.findByText(/would make sales\.orders unreadable/)).toBeInTheDocument();
    const add = screen.getByRole("button", { name: "Add classification" });
    expect(add).toHaveAttribute("aria-disabled", "true");
    expect(add).toHaveAccessibleDescription(/Hash PII masks by PII/);
    // aria-disabled, not disabled: a click must still send nothing.
    await userEvent.click(add);
    expect(served.bodies["PUT /warehouses/wh-1/resource-tags"]).toBeUndefined();
    await userEvent.click(screen.getByRole("button", { name: "Attach to columns instead" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "email" }));
    await userEvent.click(screen.getByRole("button", { name: "Attach to 1 column" }));
    await waitFor(() =>
      expect(served.bodies["PUT /warehouses/wh-1/resource-tags"]).toEqual([{ schema: "sales", table: "orders", column: "email", tag_id: "t-pii" }]),
    );
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Classification attached to 1 column."));
  });

  it("sends one write per ticked column in order, stops at the first failure and keeps the ticks", async () => {
    const served = serve({ "/tags": ok([PII]), "/perm/rules": ok([HASH]), "PUT /warehouses/wh-1/resource-tags": fail(500, "internal error") });
    const onOpenChange = open(TABLE, me(["tag:read", "policy:read", "catalog_registry:update"]));
    await pick("PII");
    await userEvent.click(await screen.findByRole("button", { name: "Attach to columns instead" }));
    const submit = screen.getByRole("button", { name: "Attach to 0 columns" });
    expect(submit).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(screen.getByRole("checkbox", { name: "id" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "email" }));
    await userEvent.click(screen.getByRole("button", { name: "Attach to 2 columns" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("internal error");
    // Stopped after the first (`id`, in the table's order).
    expect(served.bodies["PUT /warehouses/wh-1/resource-tags"]).toEqual([{ schema: "sales", table: "orders", column: "id", tag_id: "t-pii" }]);
    expect(screen.getByRole("checkbox", { name: "id" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "email" })).toBeChecked();
    expect(onOpenChange).not.toHaveBeenCalled();
  });

  it("without policy:read, says what a table-level classification does and lets the server decide", async () => {
    serve({ "/tags": ok([PII]), "PUT /warehouses/wh-1/resource-tags": { status: 400, body: CONFLICT } });
    open(TABLE, me(["tag:read", "catalog_registry:update"]));
    await pick("PII");
    expect(screen.getByText(/A rule that masks by classification needs it on columns/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Add classification" }));
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(/Hash PII masks by PII, so PII on the whole table sales\.orders would make it unreadable/)).toBeInTheDocument();
    // The server's refusal offers the same way out.
    expect(screen.getByRole("button", { name: "Attach to columns instead" })).toBeInTheDocument();
  });

  it("says other tables carry it too when the server reports more", async () => {
    serve({ "/tags": ok([PII]), "PUT /warehouses/wh-1/resource-tags": { status: 400, body: { ...CONFLICT, moreTables: true } } });
    open(TABLE, me(["tag:read", "catalog_registry:update"]));
    await pick("PII");
    await userEvent.click(screen.getByRole("button", { name: "Add classification" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Other tables carry it too");
  });

  it("does not ask for the rules without policy:read", async () => {
    const served = serve({ "/tags": ok([PII]) });
    open(TABLE, me(["tag:read", "catalog_registry:update"]));
    await pick("PII");
    expect(served.asked.some((a) => a.includes("/perm/rules"))).toBe(false);
  });

  it("shows the note, not the warning, when no mask uses the classification", async () => {
    serve({ "/tags": ok([PII]), "/perm/rules": ok([{ ...HASH, kind: { type: "row_filter", filter_expr: "1=1" } }]) });
    open(TABLE, me(["tag:read", "policy:read", "catalog_registry:update"]));
    await pick("PII");
    expect(await screen.findByText(/A rule that masks by classification needs it on columns/)).toBeInTheDocument();
    expect(screen.queryByText(/unreadable/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add classification" })).not.toHaveAttribute("aria-disabled");
  });

  it("shows the swatch, key, description and clearance in each option", async () => {
    serve({ "/tags": ok([{ ...PII, required_roles: ["cleared"] }]), "/perm/rules": ok([]) });
    open({ ...TABLE, column: "email" }, me(["tag:read", "policy:read", "catalog_registry:update"]));
    await userEvent.click(await screen.findByRole("combobox", { name: "Classification" }));
    const option = await screen.findByRole("option", { name: /PII/ });
    expect(option).toHaveTextContent("pii");
    expect(option).toHaveTextContent("Personal data");
    expect(option).toHaveTextContent("Clearance");
  });

  it("counts the rules on each option with policy:read", async () => {
    serve({ "/tags": ok([PII]), "/perm/rules": ok([HASH]) });
    open({ ...TABLE, column: "email" }, me(["tag:read", "policy:read", "catalog_registry:update"]));
    await userEvent.click(await screen.findByRole("combobox", { name: "Classification" }));
    expect(await screen.findByRole("option", { name: /PII/ })).toHaveTextContent("1 rule");
  });

  it("keeps a failed write's server sentence beside the field", async () => {
    serve({ "/tags": ok([PII]), "/perm/rules": ok([]), "PUT /warehouses/wh-1/resource-tags": fail(500, "internal error") });
    open({ ...TABLE, column: "email" }, me(["tag:read", "policy:read", "catalog_registry:update"]));
    await pick("PII");
    await userEvent.click(screen.getByRole("button", { name: "Add classification" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("internal error");
  });
});
