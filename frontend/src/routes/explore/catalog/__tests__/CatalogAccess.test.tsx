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
const useCatalogNamespacesMock = vi.fn();
const useNamespaceContentsMock = vi.fn();

vi.mock("@/api/catalog", () => ({
  nsDotted: (levels: string[]) => levels.join("."),
  useCatalogNamespaces: (ws: string, wid: string) => {
    useCatalogNamespacesMock(ws, wid);
    return { data: [{ levels: ["sale"] }, { levels: ["marketing"] }, { levels: ["marketing", "ads"] }] };
  },
  useNamespaceContents: (ws: string, wid: string, ns: string | null) => {
    useNamespaceContentsMock(ws, wid, ns);
    // Per-namespace table fixtures.
    const tables: Record<string, string[]> = {
      sale: ["orders", "returns"],
      marketing: ["campaigns", "leads"],
      "marketing.ads": ["clicks"],
    };
    return { data: { tables: ns ? (tables[ns] ?? []) : [], views: [] } };
  },
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
    scope: [{ namespace: "*", table: null, permissions: ["read", "write"] }],
    created_at: "2026-01-01T00:00:00Z",
  },
  {
    id: "t2",
    label: "marts-reader",
    scope: [
      { namespace: "sale", table: null, permissions: ["read"] },
      { namespace: "marketing", table: "campaigns", permissions: ["read", "write"] },
    ],
    created_at: "2026-01-02T00:00:00Z",
  },
];

/** Open a Radix Select (by accessible name) and click the option labelled `option`. */
async function selectOption(user: ReturnType<typeof userEvent.setup>, trigger: HTMLElement, option: string | RegExp) {
  await user.click(trigger);
  const listbox = await screen.findByRole("listbox");
  await user.click(within(listbox).getByRole("option", { name: option }));
}

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
  useCatalogNamespacesMock.mockReset();
  useNamespaceContentsMock.mockReset();
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

  it("renders the token list with a grant summary per token", () => {
    renderAt();

    // First token: a single all-namespaces read+write grant.
    const row1 = screen.getByText("dbt-ci").closest("tr")!;
    expect(within(row1).getByText("* (read, write)")).toBeInTheDocument();

    // Second token: two grants summarised independently.
    const row2 = screen.getByText("marts-reader").closest("tr")!;
    expect(within(row2).getByText("sale (read)")).toBeInTheDocument();
    expect(within(row2).getByText("marketing.campaigns (read, write)")).toBeInTheDocument();
  });

  it("adds and removes grant rows in the builder", async () => {
    const user = userEvent.setup();
    renderAt();

    await user.click(screen.getByRole("button", { name: /mint token/i }));

    // Mint starts with exactly one default grant row.
    expect(screen.getAllByTestId("grant-row")).toHaveLength(1);

    // Add a resource → two rows.
    await user.click(screen.getByRole("button", { name: /add resource/i }));
    expect(screen.getAllByTestId("grant-row")).toHaveLength(2);

    // Remove the second row → back to one.
    const rows = screen.getAllByTestId("grant-row");
    await user.click(within(rows[1]).getByRole("button", { name: /remove resource/i }));
    expect(screen.getAllByTestId("grant-row")).toHaveLength(1);
  });

  it("populates the table select from the chosen namespace", async () => {
    const user = userEvent.setup();
    renderAt();

    await user.click(screen.getByRole("button", { name: /mint token/i }));

    const row = screen.getByTestId("grant-row");
    // Choose a concrete namespace.
    await selectOption(user, within(row).getByRole("combobox", { name: /namespace/i }), "marketing");
    expect(useNamespaceContentsMock).toHaveBeenCalledWith("eng", "wh-1", "marketing");

    // Its tables are now offered in the table select.
    await user.click(within(row).getByRole("combobox", { name: /table/i }));
    const listbox = await screen.findByRole("listbox");
    expect(within(listbox).getByRole("option", { name: "campaigns" })).toBeInTheDocument();
    expect(within(listbox).getByRole("option", { name: "leads" })).toBeInTheDocument();
  });

  it("disables the table picker when the namespace is '*'", async () => {
    const user = userEvent.setup();
    renderAt();

    await user.click(screen.getByRole("button", { name: /mint token/i }));
    const row = screen.getByTestId("grant-row");

    await selectOption(user, within(row).getByRole("combobox", { name: /namespace/i }), /all namespaces/i);

    // The table picker is disabled and forced to "All tables".
    expect(within(row).getByRole("combobox", { name: /table/i })).toBeDisabled();
  });

  it("guards against an empty scope (no permissions selected)", async () => {
    const user = userEvent.setup();
    renderAt();

    await user.click(screen.getByRole("button", { name: /mint token/i }));
    await user.type(screen.getByPlaceholderText("e.g. dbt-ci"), "ci-token");

    // Default row has read on — toggle it off so the only grant has no perm.
    const row = screen.getByTestId("grant-row");
    await user.click(within(row).getByRole("button", { name: /^read$/i }));

    await user.click(screen.getByRole("button", { name: /^mint token$/i }));

    expect(mintMutateAsync).not.toHaveBeenCalled();
    expect(screen.getByText(/at least one permission/i)).toBeInTheDocument();
  });

  it("mints a token: submits the parsed scope and reveals the raw secret once", async () => {
    mintMutateAsync.mockResolvedValue({ token: tokenRows[0], raw: "egret_cat_SECRET_RAW_VALUE" });
    const user = userEvent.setup();
    renderAt();

    expect(screen.queryByText("egret_cat_SECRET_RAW_VALUE")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /mint token/i }));
    expect(useMintTokenMock).toHaveBeenCalledWith("eng", "wh-1");

    await user.type(screen.getByPlaceholderText("e.g. dbt-ci"), "ci-token");

    const row = screen.getByTestId("grant-row");
    await selectOption(user, within(row).getByRole("combobox", { name: /namespace/i }), "sale");
    await selectOption(user, within(row).getByRole("combobox", { name: /table/i }), "orders");
    // Add write so the grant is read+write.
    await user.click(within(row).getByRole("button", { name: /^write$/i }));

    await user.click(screen.getByRole("button", { name: /^mint token$/i }));

    expect(mintMutateAsync).toHaveBeenCalledTimes(1);
    const req = mintMutateAsync.mock.calls[0][0];
    expect(req.label).toBe("ci-token");
    expect(req.scope).toEqual([
      { namespace: "sale", table: "orders", permissions: ["read", "write"] },
    ]);

    const reveals = await screen.findAllByText("egret_cat_SECRET_RAW_VALUE");
    expect(reveals).toHaveLength(1);
  });

  it("edits a token: pre-fills the builder from token.scope and submits", async () => {
    updateMutateAsync.mockResolvedValue(tokenRows[1]);
    const user = userEvent.setup();
    renderAt();

    const row2 = screen.getByText("marts-reader").closest("tr")!;
    await user.click(within(row2).getByRole("button", { name: /edit/i }));

    // Pre-filled label + two grant rows.
    const labelInput = screen.getByPlaceholderText("e.g. dbt-ci") as HTMLInputElement;
    expect(labelInput.value).toBe("marts-reader");
    expect(screen.getAllByTestId("grant-row")).toHaveLength(2);

    await user.clear(labelInput);
    await user.type(labelInput, "marts-reader-2");
    await user.click(screen.getByRole("button", { name: /save changes/i }));

    expect(updateMutateAsync).toHaveBeenCalledTimes(1);
    const arg = updateMutateAsync.mock.calls[0][0];
    expect(arg.id).toBe("t2");
    expect(arg.body.label).toBe("marts-reader-2");
    expect(arg.body.scope).toEqual([
      { namespace: "sale", table: null, permissions: ["read"] },
      { namespace: "marketing", table: "campaigns", permissions: ["read", "write"] },
    ]);
  });

  it("surfaces a server 400 when minting", async () => {
    mintMutateAsync.mockRejectedValue(new Error("scope must have at least one grant"));
    const user = userEvent.setup();
    renderAt();

    await user.click(screen.getByRole("button", { name: /mint token/i }));
    await user.type(screen.getByPlaceholderText("e.g. dbt-ci"), "ci-token");
    await user.click(screen.getByRole("button", { name: /^mint token$/i }));

    expect(await screen.findByText(/at least one grant/i)).toBeInTheDocument();
  });

  it("rotates a token: calls useRotateToken and reveals the new raw secret", async () => {
    rotateMutateAsync.mockResolvedValue({ token: tokenRows[0], raw: "egret_cat_ROTATED_SECRET" });
    const user = userEvent.setup();
    renderAt();

    const row1 = screen.getByText("dbt-ci").closest("tr")!;
    await user.click(within(row1).getByRole("button", { name: /rotate/i }));
    await user.click(screen.getByRole("button", { name: /^rotate token$/i }));

    expect(rotateMutateAsync).toHaveBeenCalledWith("t1");
    const reveals = await screen.findAllByText("egret_cat_ROTATED_SECRET");
    expect(reveals).toHaveLength(1);
  });

  it("revokes a token after confirmation", async () => {
    const user = userEvent.setup();
    renderAt();

    const row1 = screen.getByText("dbt-ci").closest("tr")!;
    await user.click(within(row1).getByRole("button", { name: /revoke/i }));

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
