import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

/**
 * `GrantDialog` reads the generated perm client directly. `PrincipalPicker`
 * and `ScopePicker` are mocked to dumb controlled stand-ins — their own
 * behavior (kind offering, the group warning, row-filter narrowing) is
 * covered by their own test files; this file is about the dialog's wiring:
 * full-body PATCH, preview-before-save, and verbatim refusals.
 */
vi.mock("@/api/generated/perm/perm");
vi.mock("@/components/perm/PrincipalPicker", () => ({
  PrincipalPicker: ({
    value,
    onChange,
  }: {
    value: unknown;
    onChange: (v: unknown) => void;
  }) => (
    <div>
      <div data-testid="principals-value">{JSON.stringify(value)}</div>
      <button
        type="button"
        onClick={() =>
          onChange({ include: [{ kind: "user", ref: "alice" }], exclude: [] })
        }
      >
        set-principals
      </button>
      <button type="button" onClick={() => onChange({ include: [], exclude: [] })}>
        clear-principals
      </button>
    </div>
  ),
}));
vi.mock("@/components/perm/ScopePicker", () => ({
  ScopePicker: ({ value, onChange }: { value: unknown; onChange: (v: unknown) => void }) => (
    <div>
      <div data-testid="scope-value">{JSON.stringify(value)}</div>
      <button type="button" onClick={() => onChange({ catalog_id: "cat-1" })}>
        set-scope
      </button>
    </div>
  ),
}));

import * as permApi from "@/api/generated/perm/perm";
import { ApiError } from "@/api/client";
import type { Grant } from "@/api/generated/model";
import { GrantDialog } from "@/routes/governance/GrantDialog";

const EXISTING: Grant = {
  id: "g1",
  effect: "deny",
  actions: ["read", "write"],
  scope: { catalog_id: "cat-1", schema: "sales", table: "orders" },
  principals: {
    include: [{ kind: "role", ref: "analyst" }],
    exclude: [{ kind: "group", ref: "contractors" }],
  },
  origin: { type: "explicit" },
};

const OK_PREVIEW = {
  status: 200,
  data: {
    affected: [{ id: 1, name: "trino-prod" }],
    compile: { services: [1], created: 0, updated: 1, deleted: 0, unchanged: 3, skipped: [] },
  },
};

let previewMutateAsync: ReturnType<typeof vi.fn>;
let createMutateAsync: ReturnType<typeof vi.fn>;
let patchMutateAsync: ReturnType<typeof vi.fn>;

function renderDialog(props: Partial<React.ComponentProps<typeof GrantDialog>> = {}) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onOpenChange = vi.fn();
  const utils = render(
    <QueryClientProvider client={qc}>
      <GrantDialog
        open
        onOpenChange={onOpenChange}
        catalogName={(id) => (id === "cat-1" ? "wh" : id)}
        {...props}
      />
    </QueryClientProvider>,
  );
  return { ...utils, onOpenChange };
}

beforeEach(() => {
  vi.clearAllMocks();
  previewMutateAsync = vi.fn().mockResolvedValue(OK_PREVIEW);
  createMutateAsync = vi.fn().mockResolvedValue({ status: 201, data: undefined });
  patchMutateAsync = vi.fn().mockResolvedValue({ status: 200, data: {} });
  vi.mocked(permApi.usePermPreview).mockReturnValue({
    mutateAsync: previewMutateAsync,
    isPending: false,
  } as unknown as ReturnType<typeof permApi.usePermPreview>);
  vi.mocked(permApi.usePermCreateGrant).mockReturnValue({
    mutateAsync: createMutateAsync,
    isPending: false,
  } as unknown as ReturnType<typeof permApi.usePermCreateGrant>);
  vi.mocked(permApi.usePermPatchGrant).mockReturnValue({
    mutateAsync: patchMutateAsync,
    isPending: false,
  } as unknown as ReturnType<typeof permApi.usePermPatchGrant>);
});

describe("GrantDialog — PATCH is a full-body replace (D1)", () => {
  test("editing an existing grant seeds every field, including the exclude and every action", () => {
    renderDialog({ existing: EXISTING });
    expect(screen.getByTestId("scope-value")).toHaveTextContent(JSON.stringify(EXISTING.scope));
    expect(screen.getByTestId("principals-value")).toHaveTextContent(
      JSON.stringify(EXISTING.principals),
    );
    expect(screen.getByRole("checkbox", { name: /Read/ })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /Write/ })).toBeChecked();
  });

  test("saving an untouched edit resends every field — nothing pre-filled is dropped", async () => {
    const user = userEvent.setup();
    renderDialog({ existing: EXISTING });

    await user.click(screen.getByRole("button", { name: "Preview" }));
    const saveBtn = await screen.findByRole("button", { name: "Save changes" });
    await waitFor(() => expect(saveBtn).toBeEnabled());
    await user.click(saveBtn);

    await waitFor(() => expect(patchMutateAsync).toHaveBeenCalled());
    expect(patchMutateAsync).toHaveBeenCalledWith({
      id: "g1",
      data: {
        effect: "deny",
        actions: ["read", "write"],
        scope: EXISTING.scope,
        principals: EXISTING.principals,
      },
    });
  });
});

describe("GrantDialog — the preview result is shown before the write (D2)", () => {
  test("Save is disabled until a matching preview has run, then shows the affected profiles", async () => {
    const user = userEvent.setup();
    renderDialog({ existing: EXISTING });

    const saveBtn = screen.getByRole("button", { name: "Save changes" });
    expect(saveBtn).toBeDisabled();
    expect(screen.queryByText(/trino-prod/)).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Preview" }));

    expect(await screen.findByText(/Would recompile trino-prod/)).toBeInTheDocument();
    expect(screen.getByText(/updated 1/)).toBeInTheDocument();
    await waitFor(() => expect(saveBtn).toBeEnabled());
  });

  test("changing the form after a preview marks it stale and re-blocks Save", async () => {
    const user = userEvent.setup();
    renderDialog({ existing: EXISTING });

    await user.click(screen.getByRole("button", { name: "Preview" }));
    const saveBtn = await screen.findByRole("button", { name: "Save changes" });
    await waitFor(() => expect(saveBtn).toBeEnabled());

    await user.click(screen.getByRole("button", { name: "set-scope" }));

    expect(saveBtn).toBeDisabled();
    expect(screen.getByText(/The form changed since that preview/)).toBeInTheDocument();
  });

  test("an empty include blocks Preview and Save before any 400 is possible", async () => {
    renderDialog(); // create mode: principals default to {include: [], exclude: []}
    expect(
      screen.getByText(/Include at least one principal/),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Preview" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Create" })).toBeDisabled();
  });
});

describe("GrantDialog — a refusal is surfaced verbatim (D8)", () => {
  test("the server's own message appears, not a generic failure", async () => {
    const user = userEvent.setup();
    patchMutateAsync.mockRejectedValueOnce(
      new ApiError(409, "rule 'r1' would push profile 'trino-prod' to 501 policies, over its 500 ceiling"),
    );
    renderDialog({ existing: EXISTING });

    await user.click(screen.getByRole("button", { name: "Preview" }));
    const saveBtn = await screen.findByRole("button", { name: "Save changes" });
    await waitFor(() => expect(saveBtn).toBeEnabled());
    await user.click(saveBtn);

    expect(
      await screen.findByText(
        "rule 'r1' would push profile 'trino-prod' to 501 policies, over its 500 ceiling",
      ),
    ).toBeInTheDocument();
  });

  test("a preview refusal is also shown verbatim, and blocks Save", async () => {
    const user = userEvent.setup();
    previewMutateAsync.mockRejectedValueOnce(new ApiError(400, "a preview must name exactly one of `grant` or `rule`"));
    renderDialog({ existing: EXISTING });

    await user.click(screen.getByRole("button", { name: "Preview" }));

    expect(
      await screen.findByText("a preview must name exactly one of `grant` or `rule`"),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();
  });
});
