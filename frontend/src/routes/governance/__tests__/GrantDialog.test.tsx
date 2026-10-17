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
// The dialog reads the Governance catalog list itself (review M-6) instead of
// taking a resolved-name function from the list page.
vi.mock("@/api/generated/control-plane-governance/control-plane-governance");
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
import * as governance from "@/api/generated/control-plane-governance/control-plane-governance";
import { ApiError } from "@/api/client";
import type { Grant, GovernanceCatalog } from "@/api/generated/model";
import { GrantDialog } from "@/routes/governance/GrantDialog";

const CATALOG: GovernanceCatalog = {
  id: "cat-1",
  name: "wh",
  source: "egret",
  connectorType: "iceberg",
  engines: [],
  warehouseId: "wh-1",
  warehouseName: "wh",
  workspaces: [],
};

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
      <GrantDialog open onOpenChange={onOpenChange} {...props} />
    </QueryClientProvider>,
  );
  return { ...utils, onOpenChange };
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(governance.useControlPlaneGovernanceListCatalogs).mockReturnValue({
    data: { status: 200, data: [CATALOG] },
    isLoading: false,
    isError: false,
  } as unknown as ReturnType<typeof governance.useControlPlaneGovernanceListCatalogs>);
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

describe("GrantDialog — initialScope (the object page's Grant access)", () => {
  const OBJECT_SCOPE = { catalog_id: "cat-1", schema: "sales", table: "orders" };

  test("a new grant starts on the scope it was opened for, and creates on it", async () => {
    const user = userEvent.setup();
    renderDialog({ initialScope: OBJECT_SCOPE });

    expect(screen.getByRole("heading", { name: "New grant" })).toBeInTheDocument();
    expect(screen.getByTestId("scope-value")).toHaveTextContent(JSON.stringify(OBJECT_SCOPE));
    expect(screen.getByText(/Preview: read on wh\.sales\.orders/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "set-principals" }));
    await user.click(screen.getByRole("button", { name: "Preview" }));
    const create = await screen.findByRole("button", { name: "Create" });
    await waitFor(() => expect(create).toBeEnabled());
    await user.click(create);

    await waitFor(() => expect(createMutateAsync).toHaveBeenCalled());
    expect(createMutateAsync).toHaveBeenCalledWith({
      data: {
        effect: "allow",
        actions: ["read"],
        scope: OBJECT_SCOPE,
        principals: { include: [{ kind: "user", ref: "alice" }], exclude: [] },
      },
    });
  });

  test("an existing grant keeps its own scope", () => {
    renderDialog({ existing: EXISTING, initialScope: { catalog_id: "cat-2" } });
    expect(screen.getByTestId("scope-value")).toHaveTextContent(JSON.stringify(EXISTING.scope));
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

describe("GrantDialog — two identical skip lines (SP4b-2 finding 4)", () => {
  /**
   * The same panel, keyed the same way, so it carries the same collision:
   * `SkipReport` has no profile field, so one grant skipped in two profiles is
   * two byte-identical entries and `${source_kind}-${id}` is not unique. Fixed
   * in both dialogs, so tested in both — a key fixed only where the browser
   * happened to trip over it is a fix waiting to be re-broken.
   */
  test("renders both and logs no duplicate-key error", async () => {
    const skip = {
      source_kind: "grant",
      id: "e615e686-215b-4513-85bd-15cbc7310cc0",
      reason: "profile cannot express this grant",
    };
    previewMutateAsync.mockResolvedValue({
      status: 200,
      data: {
        affected: [
          { id: 1, name: "trino-prod" },
          { id: 2, name: "trino-eu" },
        ],
        compile: {
          services: [1, 2],
          created: 0,
          updated: 0,
          deleted: 0,
          unchanged: 1,
          skipped: [skip, { ...skip }],
        },
      },
    });
    const errors: unknown[][] = [];
    const spy = vi.spyOn(console, "error").mockImplementation((...args) => {
      errors.push(args);
    });

    const user = userEvent.setup();
    renderDialog({ existing: EXISTING });
    await user.click(screen.getByRole("button", { name: "Preview" }));

    await waitFor(() => expect(previewMutateAsync).toHaveBeenCalled());
    expect(await screen.findAllByText(/cannot express this grant/)).toHaveLength(2);
    expect(
      errors.filter((a) => String(a[0]).includes("same key")),
      `React reported a duplicate key: ${JSON.stringify(errors)}`,
    ).toHaveLength(0);

    spy.mockRestore();
  });
});

describe("GrantDialog — it resolves the catalog's name itself (review M-6)", () => {
  /**
   * The dialog reads the Governance catalog list directly, instead of taking a
   * lookup function from the list page. Nothing asserted the resolved NAME, so
   * a mutation that returned the id survived — this is that missing assertion.
   * A writer must pick a catalog, and the preview line must say which.
   */
  test("the preview line names the catalog, not its uuid", async () => {
    renderDialog({ existing: EXISTING });

    expect(screen.getByText(/Preview: read and write on wh\.sales\.orders/)).toBeInTheDocument();
    // (The mocked `ScopePicker` stand-in prints the raw scope JSON, so `cat-1`
    // legitimately appears in the DOM — the negative direction is asserted by
    // the forbidden-list test below instead.)
  });

  /** With the catalog list forbidden the dialog falls back to the id rather than
   *  inventing a name — the same honesty the list page's sentence keeps. */
  test("falls back to the id when the catalog list is forbidden", async () => {
    vi.mocked(governance.useControlPlaneGovernanceListCatalogs).mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: new ApiError(403, "forbidden"),
    } as unknown as ReturnType<typeof governance.useControlPlaneGovernanceListCatalogs>);
    renderDialog({ existing: EXISTING });

    expect(screen.getByText(/Preview: read and write on cat-1\.sales\.orders/)).toBeInTheDocument();
  });
});

describe("GrantDialog — a preview that did not succeed says so (review M-7)", () => {
  /**
   * **THE DISCRIMINATING CASE.** The generated mutator resolves whatever
   * envelope came back, so a non-200 that does not throw left `preview` null
   * and no error: pressing Preview looked like nothing happened at all. It
   * failed CLOSED — Save stays blocked without a preview — but an operator
   * cannot tell "refused" from "button broken", and silence is the failure this
   * branch keeps fixing elsewhere.
   */
  test("a non-200 preview envelope reports the status instead of nothing", async () => {
    previewMutateAsync.mockResolvedValueOnce({ status: 500, data: { error: "boom" } });
    const user = userEvent.setup();
    renderDialog({ existing: EXISTING });

    await user.click(screen.getByRole("button", { name: "Preview" }));

    expect(await screen.findByText(/Preview failed \(HTTP 500\)/)).toBeInTheDocument();
    // …and it still fails closed.
    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();
    expect(screen.queryByText(/Would recompile/)).not.toBeInTheDocument();
  });

  /** A thrown ApiError keeps its own message — this must not have flattened
   *  every failure into the generic status line. */
  test("a thrown error still shows the server's message", async () => {
    previewMutateAsync.mockRejectedValueOnce(new ApiError(400, "scope names no catalog"));
    const user = userEvent.setup();
    renderDialog({ existing: EXISTING });

    await user.click(screen.getByRole("button", { name: "Preview" }));

    expect(await screen.findByText(/scope names no catalog/)).toBeInTheDocument();
    expect(screen.queryByText(/HTTP/)).not.toBeInTheDocument();
  });
});
