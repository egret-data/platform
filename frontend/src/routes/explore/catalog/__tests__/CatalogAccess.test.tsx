import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const tokensMock = vi.fn();
const mintMutateAsync = vi.fn();
const updateMutateAsync = vi.fn();
const rotateMutateAsync = vi.fn();
const revokeMutate = vi.fn();
const mintReset = vi.fn();
const updateReset = vi.fn();
const rotateReset = vi.fn();
const useCatalogTokensMock = vi.fn();
const useMintTokenMock = vi.fn();
const useUpdateTokenMock = vi.fn();
const useRotateTokenMock = vi.fn();
const useRevokeTokenMock = vi.fn();

vi.mock("@/api/catalog", () => ({
  useCatalogTokens: (ws: string, wid: string) => {
    useCatalogTokensMock(ws, wid);
    return tokensMock();
  },
  useMintToken: (ws: string, wid: string) => {
    useMintTokenMock(ws, wid);
    return { mutateAsync: mintMutateAsync, reset: mintReset, isPending: false };
  },
  useUpdateToken: (ws: string, wid: string) => {
    useUpdateTokenMock(ws, wid);
    return { mutateAsync: updateMutateAsync, reset: updateReset, isPending: false };
  },
  useRotateToken: (ws: string, wid: string) => {
    useRotateTokenMock(ws, wid);
    return { mutateAsync: rotateMutateAsync, reset: rotateReset, isPending: false };
  },
  useRevokeToken: (ws: string, wid: string) => {
    useRevokeTokenMock(ws, wid);
    return { mutate: revokeMutate };
  },
}));

import CatalogAccess from "../CatalogAccess";

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <CatalogAccess ws="eng" wid="wh-1" />
    </QueryClientProvider>,
  );
}

const tokenRows = [
  {
    id: "t1",
    label: "dbt-ci",
    permissions: ["catalog:read", "catalog:write"],
    namespace_scope: [],
    table_scope: [],
    created_at: "2026-01-01T00:00:00Z",
  },
  {
    id: "t2",
    label: "marts-reader",
    permissions: ["catalog:read"],
    namespace_scope: ["marts"],
    table_scope: ["marts.daily"],
    created_at: "2026-01-02T00:00:00Z",
  },
];

beforeEach(() => {
  tokensMock.mockReset();
  mintMutateAsync.mockReset();
  updateMutateAsync.mockReset();
  rotateMutateAsync.mockReset();
  revokeMutate.mockReset();
  mintReset.mockReset();
  updateReset.mockReset();
  rotateReset.mockReset();
  useCatalogTokensMock.mockReset();
  useMintTokenMock.mockReset();
  useUpdateTokenMock.mockReset();
  useRotateTokenMock.mockReset();
  useRevokeTokenMock.mockReset();
  tokensMock.mockReturnValue({ data: tokenRows, isLoading: false, isError: false });
});

describe("CatalogAccess", () => {
  it("scopes its hooks to the warehouse id", () => {
    renderAt();
    expect(useCatalogTokensMock).toHaveBeenCalledWith("eng", "wh-1");
    expect(useRevokeTokenMock).toHaveBeenCalledWith("eng", "wh-1");
    expect(useUpdateTokenMock).toHaveBeenCalledWith("eng", "wh-1");
    expect(useRotateTokenMock).toHaveBeenCalledWith("eng", "wh-1");
  });

  it("renders the token list with permission badges, namespace + table scope", () => {
    renderAt();

    // First token: both read + write badges.
    const row1 = screen.getByText("dbt-ci").closest("tr")!;
    expect(within(row1).getByText("read")).toBeInTheDocument();
    expect(within(row1).getByText("write")).toBeInTheDocument();
    // Empty namespace_scope renders "*", empty table_scope renders "All tables".
    expect(within(row1).getByText("*")).toBeInTheDocument();
    expect(within(row1).getByText("All tables")).toBeInTheDocument();

    // Second token: read only, scoped to "marts" / "marts.daily".
    const row2 = screen.getByText("marts-reader").closest("tr")!;
    expect(within(row2).getByText("read")).toBeInTheDocument();
    expect(within(row2).queryByText("write")).not.toBeInTheDocument();
    expect(within(row2).getByText("marts")).toBeInTheDocument();
    expect(within(row2).getByText("marts.daily")).toBeInTheDocument();
  });

  it("mints a token: parses table_scope and reveals the raw secret once", async () => {
    mintMutateAsync.mockResolvedValue({
      token: tokenRows[0],
      raw: "egret_cat_SECRET_RAW_VALUE",
    });
    const user = userEvent.setup();
    renderAt();

    // Raw is not present before submit.
    expect(screen.queryByText("egret_cat_SECRET_RAW_VALUE")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /mint token/i }));

    // The mint hook is scoped to the warehouse id.
    expect(useMintTokenMock).toHaveBeenCalledWith("eng", "wh-1");

    // Read is on by default. Set label + table scope (newline + comma separated).
    await user.type(screen.getByPlaceholderText("e.g. dbt-ci"), "ci-token");
    await user.type(
      screen.getByPlaceholderText(/leave blank for all tables/i),
      "sales.orders, marts.daily",
    );

    // Submit the dialog form.
    const submit = screen.getByRole("button", { name: /^mint token$/i });
    await user.click(submit);

    expect(mintMutateAsync).toHaveBeenCalledTimes(1);
    const req = mintMutateAsync.mock.calls[0][0];
    expect(req.permissions).toEqual(["read"]);
    expect(req.label).toBe("ci-token");
    expect(req.namespace_scope).toEqual([]);
    expect(req.table_scope).toEqual(["sales.orders", "marts.daily"]);

    // Reveal phase shows the raw secret exactly once.
    const reveals = await screen.findAllByText("egret_cat_SECRET_RAW_VALUE");
    expect(reveals).toHaveLength(1);
  });

  it("rejects an invalid single-segment table scope before minting", async () => {
    const user = userEvent.setup();
    renderAt();

    await user.click(screen.getByRole("button", { name: /mint token/i }));
    await user.type(screen.getByPlaceholderText("e.g. dbt-ci"), "ci-token");
    await user.type(screen.getByPlaceholderText(/leave blank for all tables/i), "orders");

    await user.click(screen.getByRole("button", { name: /^mint token$/i }));

    // Client-side rejection: mint hook is never called, error is surfaced.
    expect(mintMutateAsync).not.toHaveBeenCalled();
    expect(screen.getByText(/Invalid table scope/i)).toBeInTheDocument();
  });

  it("edits a token: pre-fills from the row and submits via useUpdateToken", async () => {
    updateMutateAsync.mockResolvedValue(tokenRows[1]);
    const user = userEvent.setup();
    renderAt();

    const row2 = screen.getByText("marts-reader").closest("tr")!;
    await user.click(within(row2).getByRole("button", { name: /edit/i }));

    // Pre-filled from the row.
    const labelInput = screen.getByPlaceholderText("e.g. dbt-ci") as HTMLInputElement;
    expect(labelInput.value).toBe("marts-reader");
    const tablesInput = screen.getByPlaceholderText(
      /leave blank for all tables/i,
    ) as HTMLTextAreaElement;
    expect(tablesInput.value).toBe("marts.daily");

    // Change the label and submit.
    await user.clear(labelInput);
    await user.type(labelInput, "marts-reader-2");
    await user.click(screen.getByRole("button", { name: /save changes/i }));

    expect(updateMutateAsync).toHaveBeenCalledTimes(1);
    const arg = updateMutateAsync.mock.calls[0][0];
    expect(arg.id).toBe("t2");
    expect(arg.body.label).toBe("marts-reader-2");
    expect(arg.body.permissions).toEqual(["read"]);
    expect(arg.body.namespace_scope).toEqual(["marts"]);
    expect(arg.body.table_scope).toEqual(["marts.daily"]);
  });

  it("rotates a token: calls useRotateToken and reveals the new raw secret", async () => {
    rotateMutateAsync.mockResolvedValue({
      token: tokenRows[0],
      raw: "egret_cat_ROTATED_SECRET",
    });
    const user = userEvent.setup();
    renderAt();

    const row1 = screen.getByText("dbt-ci").closest("tr")!;
    await user.click(within(row1).getByRole("button", { name: /rotate/i }));

    // Confirm phase → confirm the rotation.
    await user.click(screen.getByRole("button", { name: /^rotate token$/i }));

    expect(rotateMutateAsync).toHaveBeenCalledWith("t1");

    // The new raw secret is revealed (reuse of the mint reveal UI).
    const reveals = await screen.findAllByText("egret_cat_ROTATED_SECRET");
    expect(reveals).toHaveLength(1);
  });

  it("revokes a token after confirmation", async () => {
    const user = userEvent.setup();
    renderAt();

    const row1 = screen.getByText("dbt-ci").closest("tr")!;
    await user.click(within(row1).getByRole("button", { name: /revoke/i }));

    // ConfirmDeleteDialog gates its action button behind typing the confirm
    // word — here "revoke" (our confirmText). The action button is always
    // literally labelled "Delete", so match that exactly.
    const confirmDialog = screen.getByRole("alertdialog");
    await user.type(within(confirmDialog).getByPlaceholderText("revoke"), "revoke");
    await user.click(within(confirmDialog).getByRole("button", { name: "Delete" }));

    expect(revokeMutate).toHaveBeenCalledWith("t1");
  });

  it("shows a workspace-admin notice on 403 and hides the table", () => {
    tokensMock.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: { status: 403 },
    });
    renderAt();

    expect(
      screen.getByText("Requires workspace-admin to manage catalog access."),
    ).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });
});
