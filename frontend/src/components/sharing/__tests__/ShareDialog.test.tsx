import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, test, vi } from "vitest";

// ---- Mocks for the generated shares hooks ----------------------------------
// useSharesGrantList is called twice by the dialog: once for the object's own
// (direct) grants and once for the folder's (inherited) grants — we key the
// stubbed result off the `kind` argument (`"folder"` = inherited).
type QResult = {
  data?: { data: unknown[] };
  isLoading: boolean;
  isError: boolean;
  error: unknown;
};

let directResult: QResult;
let inheritedResult: QResult;
const putMutate = vi.fn();
const delMutate = vi.fn();

vi.mock("@/api/generated/shares/shares", () => ({
  getSharesGrantListQueryKey: () => ["grants"],
  useSharesGrantList: (_ws: string, kind: string) =>
    kind === "folder" ? inheritedResult : directResult,
  useSharesGrantPut: () => ({ mutate: putMutate, isPending: false }),
  useSharesGrantDelete: () => ({ mutate: delMutate, isPending: false }),
}));

vi.mock("@/api/policies", () => ({
  listPrincipals: vi.fn(() => Promise.resolve([])),
}));

import { ShareDialog } from "@/components/sharing/ShareDialog";

const ok = (grants: unknown[]): QResult => ({
  data: { data: grants },
  isLoading: false,
  isError: false,
  error: null,
});

const empty = ok([]);

function renderDialog(props: Partial<React.ComponentProps<typeof ShareDialog>> = {}) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <ShareDialog ws="eng" kind="chart" id="c1" open onOpenChange={() => {}} {...props} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  directResult = empty;
  inheritedResult = empty;
  putMutate.mockReset();
  delMutate.mockReset();
});

test("lists direct grants with principal and role", () => {
  directResult = ok([
    { id: "g1", principal_type: "user", principal_id: "alice@egret.local", role: "ws_viewer" },
    { id: "g2", principal_type: "group", principal_id: "analysts", role: "ws_editor" },
  ]);
  renderDialog();
  expect(screen.getByText("alice@egret.local")).toBeInTheDocument();
  expect(screen.getByText("analysts")).toBeInTheDocument();
  expect(screen.getByLabelText("Role for alice@egret.local")).toBeInTheDocument();
});

test("renders read-only inherited section when folderId is set", () => {
  directResult = ok([
    { id: "g1", principal_type: "user", principal_id: "alice@egret.local", role: "ws_viewer" },
  ]);
  inheritedResult = ok([
    { id: "f1", principal_type: "group", principal_id: "team-eng", role: "ws_editor" },
  ]);
  renderDialog({ folderId: "fold1" });

  expect(screen.getByText("Inherited from folder")).toBeInTheDocument();
  expect(screen.getByText("team-eng")).toBeInTheDocument();
  // Inherited rows are read-only: no remove control.
  expect(screen.queryByLabelText("Remove team-eng")).toBeNull();
  // Direct rows keep their remove control.
  expect(screen.getByLabelText("Remove alice@egret.local")).toBeInTheDocument();
});

test("does not render an inherited section without folderId", () => {
  directResult = ok([]);
  renderDialog();
  expect(screen.queryByText("Inherited from folder")).toBeNull();
});

test("calls the PUT hook when adding a grant", async () => {
  renderDialog();
  await userEvent.type(screen.getByLabelText("User email"), "bob@egret.local");
  await userEvent.click(screen.getByRole("button", { name: "Add" }));

  expect(putMutate).toHaveBeenCalledTimes(1);
  expect(putMutate.mock.calls[0][0]).toEqual({
    workspace: "eng",
    kind: "chart",
    id: "c1",
    data: { principal_type: "user", principal_id: "bob@egret.local", role: "ws_viewer" },
  });
});

test("calls the DELETE hook when removing a direct grant", async () => {
  directResult = ok([
    { id: "g1", principal_type: "user", principal_id: "alice@egret.local", role: "ws_viewer" },
  ]);
  renderDialog();
  await userEvent.click(screen.getByLabelText("Remove alice@egret.local"));

  expect(delMutate).toHaveBeenCalledTimes(1);
  expect(delMutate.mock.calls[0][0]).toMatchObject({ grantId: "g1" });
});

test("shows a permission message (no crash) on a 403 from the list", () => {
  directResult = { data: undefined, isLoading: false, isError: true, error: { status: 403 } };
  renderDialog();
  expect(screen.getByRole("alert")).toHaveTextContent(/don.?t have permission to manage sharing/i);
  // The ACL is not leaked: no add controls rendered.
  expect(screen.queryByRole("button", { name: "Add" })).toBeNull();
});
