import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const tokensMock = vi.fn();
const mintMutateAsync = vi.fn();
const revokeMutate = vi.fn();
const mintReset = vi.fn();
const useCatalogTokensMock = vi.fn();
const useMintTokenMock = vi.fn();
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
    created_at: "2026-01-01T00:00:00Z",
  },
  {
    id: "t2",
    label: "marts-reader",
    permissions: ["catalog:read"],
    namespace_scope: ["marts"],
    created_at: "2026-01-02T00:00:00Z",
  },
];

beforeEach(() => {
  tokensMock.mockReset();
  mintMutateAsync.mockReset();
  revokeMutate.mockReset();
  mintReset.mockReset();
  useCatalogTokensMock.mockReset();
  useMintTokenMock.mockReset();
  useRevokeTokenMock.mockReset();
  tokensMock.mockReturnValue({ data: tokenRows, isLoading: false, isError: false });
});

describe("CatalogAccess", () => {
  it("scopes its hooks to the warehouse id", () => {
    renderAt();
    expect(useCatalogTokensMock).toHaveBeenCalledWith("eng", "wh-1");
    expect(useRevokeTokenMock).toHaveBeenCalledWith("eng", "wh-1");
  });

  it("renders the token list with permission badges and namespace scope", () => {
    renderAt();

    // First token: both read + write badges.
    const row1 = screen.getByText("dbt-ci").closest("tr")!;
    expect(within(row1).getByText("read")).toBeInTheDocument();
    expect(within(row1).getByText("write")).toBeInTheDocument();
    // Empty namespace_scope renders "*".
    expect(within(row1).getByText("*")).toBeInTheDocument();

    // Second token: read only, scoped to "marts".
    const row2 = screen.getByText("marts-reader").closest("tr")!;
    expect(within(row2).getByText("read")).toBeInTheDocument();
    expect(within(row2).queryByText("write")).not.toBeInTheDocument();
    expect(within(row2).getByText("marts")).toBeInTheDocument();
  });

  it("mints a token: read-only, then reveals the raw secret once", async () => {
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

    // Read is on by default. Set label.
    await user.type(screen.getByPlaceholderText("e.g. dbt-ci"), "ci-token");

    // Submit the dialog form.
    const submit = screen.getByRole("button", { name: /^mint token$/i });
    await user.click(submit);

    expect(mintMutateAsync).toHaveBeenCalledTimes(1);
    const req = mintMutateAsync.mock.calls[0][0];
    expect(req.permissions).toEqual(["read"]);
    expect(req.label).toBe("ci-token");
    expect(req.namespace_scope).toEqual([]);

    // Reveal phase shows the raw secret exactly once.
    const reveals = await screen.findAllByText("egret_cat_SECRET_RAW_VALUE");
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
