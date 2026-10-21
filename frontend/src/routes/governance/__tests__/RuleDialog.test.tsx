import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, onTestFinished, test, vi } from "vitest";

/**
 * `ScopePicker` is mocked to a dumb stand-in that reports the `rowFilterMode`
 * prop it was given, plus buttons to simulate a complete path scope and a
 * (would-be-refused) one with a column — its own column-hiding behavior is
 * covered by `ScopePicker.test.tsx`. The tag selector is NOT mocked: it is
 * the thing Task 4 is about, and `useTags` is mocked instead so it has real
 * data to browse.
 */
vi.mock("@/api/generated/perm/perm");
// The dialog reads the Governance catalog list itself (review M-6; `policy:read`, not `catalog_registry:read`).
vi.mock("@/api/generated/control-plane-governance/control-plane-governance");
vi.mock("@/api/tags");
vi.mock("@/api/generated/control-plane-catalog/control-plane-catalog");
vi.mock("@/components/perm/ScopePicker", () => ({
  ScopePicker: ({
    value,
    onChange,
    rowFilterMode,
    catalogRequired,
  }: {
    value: unknown;
    onChange: (v: unknown) => void;
    rowFilterMode?: boolean;
    catalogRequired?: boolean;
  }) => (
    <div
      data-testid="scope-picker"
      data-row-filter-mode={String(!!rowFilterMode)}
      data-catalog-required={String(!!catalogRequired)}
    >
      <div data-testid="scope-value">{JSON.stringify(value)}</div>
      <button
        type="button"
        onClick={() => onChange({ catalog_id: "cat-1", schema: "sales", table: "orders" })}
      >
        set-scope-complete
      </button>
      <button
        type="button"
        onClick={() =>
          onChange({ catalog_id: "cat-1", schema: "sales", table: "orders", column: "ssn" })
        }
      >
        set-scope-with-column
      </button>
    </div>
  ),
}));

import * as permApi from "@/api/generated/perm/perm";
import * as governance from "@/api/generated/control-plane-governance/control-plane-governance";
import * as tagsApi from "@/api/tags";
import * as catalogApi from "@/api/generated/control-plane-catalog/control-plane-catalog";
import { MemoryRouter } from "react-router-dom";
import { useState } from "react";
import { ApiError } from "@/api/client";
import type { GovernanceCatalog, Rule } from "@/api/generated/model";
import type { Me } from "@/api/hooks";
import { AuthContext } from "@/auth/useAuth";
import { RuleDialog } from "@/routes/governance/RuleDialog";
import { clearAllRuleDrafts, loadRuleDraft, ruleDraftKey, type RuleEntry } from "@/routes/governance/ruleDraft";

const ME: Me = { user_id: "u1", email: "", platform_roles: ["platform_admin"], permissions: [] };
const CLASSIFICATION_ENTRY: RuleEntry = { kind: "classification", tagKey: "pii_v2", tagName: "PII Data" };
const TABLE_ENTRY: RuleEntry = { kind: "table", catalogId: "cat-1", schema: "sales", table: "orders" };

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

const TAGS = [
  {
    id: "t1",
    key: "pii_v2",
    name: "PII Data",
    description: "",
    color: "#fff",
    assignment_count: 0,
    created_at: "",
    required_roles: [],
    required_groups: [],
  },
];

const OK_PREVIEW_ONE = {
  status: 200,
  data: {
    affected: [{ id: 1, name: "trino-prod" }],
    compile: { services: [1], created: 1, updated: 0, deleted: 0, unchanged: 0, skipped: [] },
  },
};
const OK_PREVIEW_MANY = {
  status: 200,
  data: {
    affected: [
      { id: 1, name: "trino-prod" },
      { id: 2, name: "trino-staging" },
      { id: 3, name: "trino-eu" },
    ],
    compile: { services: [1, 2, 3], created: 0, updated: 3, deleted: 0, unchanged: 0, skipped: [] },
  },
};

let previewMutateAsync: ReturnType<typeof vi.fn>;

/**
 * The unassign hook as the Remove confirm reads it: `unassign` is the call,
 * and a rejection lands in `error`/`isError` the way the mutation's state would.
 */
function mockUnassign(unassign: (v: unknown) => unknown) {
  vi.mocked(catalogApi.useControlPlaneCatalogUnassignResourceTag).mockImplementation((() => {
    const [error, setError] = useState<unknown>(null);
    return {
      mutateAsync: async (v: unknown) => {
        try {
          return await unassign(v);
        } catch (e) {
          setError(e);
          throw e;
        }
      },
      isPending: false,
      isError: error !== null,
      error,
      reset: () => setError(null),
    };
  }) as never);
}
let createMutateAsync: ReturnType<typeof vi.fn>;
let patchMutateAsync: ReturnType<typeof vi.fn>;

function renderDialog(props: Partial<React.ComponentProps<typeof RuleDialog>> = {}, me: Me | null = ME) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onOpenChange = vi.fn();
  const tree = (p: Partial<React.ComponentProps<typeof RuleDialog>>) => (
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <MemoryRouter>
          <RuleDialog open onOpenChange={onOpenChange} entry={CLASSIFICATION_ENTRY} {...p} />
        </MemoryRouter>
      </QueryClientProvider>
    </AuthContext.Provider>
  );
  const utils = render(tree(props));
  /** Re-renders the same open dialog with new props, as a parent's refetch would. */
  const rerenderWith = (p: Partial<React.ComponentProps<typeof RuleDialog>>) => utils.rerender(tree(p));
  return { ...utils, onOpenChange, rerenderWith };
}

/** A table entry switched to Applies to = Classification: the tag picker starts empty. */
async function openWithNoClassification(user: ReturnType<typeof userEvent.setup>) {
  renderDialog({ entry: TABLE_ENTRY });
  await user.click(screen.getByRole("combobox", { name: "Applies to" }));
  await user.click(await screen.findByRole("option", { name: "Classification" }));
}

beforeEach(() => {
  vi.clearAllMocks();
  // The draft store is module-level by design (it outlives the dialog); no test inherits another's draft.
  clearAllRuleDrafts();
  vi.mocked(governance.useControlPlaneGovernanceListCatalogs).mockReturnValue({
    data: { status: 200, data: [CATALOG] },
    isLoading: false,
    isError: false,
  } as unknown as ReturnType<typeof governance.useControlPlaneGovernanceListCatalogs>);
  vi.mocked(tagsApi.useTags).mockReturnValue({ data: TAGS } as unknown as ReturnType<typeof tagsApi.useTags>);
  previewMutateAsync = vi.fn().mockResolvedValue(OK_PREVIEW_ONE);
  createMutateAsync = vi.fn().mockResolvedValue({ status: 201, data: undefined });
  patchMutateAsync = vi.fn().mockResolvedValue({ status: 200, data: {} });
  vi.mocked(permApi.usePermPreview).mockReturnValue({
    mutateAsync: previewMutateAsync,
    isPending: false,
  } as unknown as ReturnType<typeof permApi.usePermPreview>);
  mockUnassign(vi.fn());
  // The Remove confirm names the rules on the classification (Task 6's dialog).
  vi.mocked(permApi.usePermListRules).mockReturnValue({
    data: { status: 200, data: [] },
    isError: false,
  } as unknown as ReturnType<typeof permApi.usePermListRules>);
  vi.mocked(permApi.usePermCreateRule).mockReturnValue({
    mutateAsync: createMutateAsync,
    isPending: false,
  } as unknown as ReturnType<typeof permApi.usePermCreateRule>);
  vi.mocked(permApi.usePermPatchRule).mockReturnValue({
    mutateAsync: patchMutateAsync,
    isPending: false,
  } as unknown as ReturnType<typeof permApi.usePermPatchRule>);
});

describe("RuleDialog — the tag selector carries the key, not the name", () => {
  test("selecting a tag by its NAME stores the model's KEY", async () => {
    const user = userEvent.setup();
    await openWithNoClassification(user);

    await user.type(screen.getByLabelText(/^Name/), "mask-pii");
    await user.type(screen.getByPlaceholderText("Search classifications by name…"), "PII");
    const option = await screen.findByRole("button", { name: /PII Data/ });
    expect(option).toHaveTextContent("Key pii_v2");
    expect(option).not.toHaveTextContent("stored as");
    await user.click(option);

    // The chosen tag now reads by name, with its stored key visible beside it —
    // proving the picker itself never displays the raw key as the selection.
    expect(screen.getByText("PII Data")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Preview" }));
    await waitFor(() => expect(previewMutateAsync).toHaveBeenCalled());
    const sent = previewMutateAsync.mock.calls[0][0];
    expect(sent.data.rule.selector).toEqual({ type: "tag", key: "pii_v2" });
  });
});

describe("RuleDialog — mask_type is a fixed list, CUSTOM reveals the expression", () => {
  test("CUSTOM shows the expression field; MASK_HASH does not", async () => {
    const user = userEvent.setup();
    renderDialog();

    expect(screen.queryByLabelText(/^Custom expression/)).not.toBeInTheDocument();

    await user.click(screen.getByRole("combobox", { name: /mask type/i }));
    await user.click(await screen.findByRole("option", { name: "Custom" }));
    expect(screen.getByLabelText(/^Custom expression/)).toBeInTheDocument();
    expect(screen.getByLabelText(/^Custom expression/)).toHaveAttribute("aria-required", "true");
    expect(screen.getByText("Custom expression").closest("label")).toHaveTextContent("Custom expression (required)");

    await user.click(screen.getByRole("combobox", { name: /mask type/i }));
    await user.click(await screen.findByRole("option", { name: "Hash" }));
    expect(screen.queryByLabelText(/^Custom expression/)).not.toBeInTheDocument();
  });
});

describe("RuleDialog — a row filter cannot be scoped to a column (D6)", () => {
  test("row_filter + path passes rowFilterMode to ScopePicker, and a forced column still blocks Save", async () => {
    const user = userEvent.setup();
    renderDialog();

    await user.type(screen.getByLabelText(/^Name/), "filter-region");
    await user.click(screen.getByRole("combobox", { name: "Kind" }));
    await user.click(await screen.findByRole("option", { name: "Filter rows" }));
    await user.type(screen.getByLabelText(/^Filter expression/), "region = 'us'");
    await user.click(screen.getByRole("combobox", { name: "Applies to" }));
    await user.click(await screen.findByRole("option", { name: "Path" }));

    expect(screen.getByTestId("scope-picker")).toHaveAttribute("data-row-filter-mode", "true");

    await user.click(screen.getByRole("button", { name: "set-scope-with-column" }));
    expect(
      screen.getByText(/A path-selected row filter needs exactly a catalog, schema and table/),
    ).toBeInTheDocument();
    const preview = screen.getByRole("button", { name: "Preview" });
    expect(preview).toHaveAttribute("aria-disabled", "true");
    expect(preview).toHaveAccessibleDescription(/A path-selected row filter needs exactly a catalog, schema and table/);

    await user.click(screen.getByRole("button", { name: "set-scope-complete" }));
    expect(screen.getByRole("button", { name: "Preview" })).toHaveAttribute("aria-disabled", "false");
  });

  /**
   * **THE DISCRIMINATING CASE (review M-4).** A column typed while the kind was
   * `mask` survives the switch to `row_filter`, where `ScopePicker` renders no
   * Column field at all (fact 4/5: Ranger's `rowFilterDef` has no column
   * level). Save was then blocked by a reason naming a field the operator can
   * neither see nor clear — an unwinnable form. The order here is the operator's
   * order: pick the column FIRST, switch Kind SECOND.
   */
  test("switching Kind to row_filter drops a column the new form cannot show", async () => {
    const user = userEvent.setup();
    renderDialog();

    await user.type(screen.getByLabelText(/^Name/), "filter-region");
    await user.click(screen.getByRole("combobox", { name: "Applies to" }));
    await user.click(await screen.findByRole("option", { name: "Path" }));
    await user.click(screen.getByRole("button", { name: "set-scope-with-column" }));
    expect(screen.getByTestId("scope-value")).toHaveTextContent('"column":"ssn"');

    await user.click(screen.getByRole("combobox", { name: "Kind" }));
    await user.click(await screen.findByRole("option", { name: "Filter rows" }));
    await user.type(screen.getByLabelText(/^Filter expression/), "region = 'us'");

    // The column is gone from the form's own state, so the form is completable.
    expect(screen.getByTestId("scope-value")).not.toHaveTextContent('"column"');
    expect(
      screen.queryByText(/A path-selected row filter needs exactly a catalog, schema and table/),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Preview" })).toHaveAttribute("aria-disabled", "false");
  });

  /** The reverse switch must NOT invent or keep anything: going back to `mask`
   *  leaves the column cleared rather than resurrecting it. */
  test("switching back to mask does not resurrect the dropped column", async () => {
    const user = userEvent.setup();
    renderDialog();

    await user.click(screen.getByRole("combobox", { name: "Applies to" }));
    await user.click(await screen.findByRole("option", { name: "Path" }));
    await user.click(screen.getByRole("button", { name: "set-scope-with-column" }));
    await user.click(screen.getByRole("combobox", { name: "Kind" }));
    await user.click(await screen.findByRole("option", { name: "Filter rows" }));
    await user.click(screen.getByRole("combobox", { name: "Kind" }));
    await user.click(await screen.findByRole("option", { name: "Mask a column" }));

    expect(screen.getByTestId("scope-value")).not.toHaveTextContent('"column"');
  });
});

describe("RuleDialog — the preview of a tag rule can fan out to every profile (D2)", () => {
  test("shows more than one profile when several would recompile", async () => {
    const user = userEvent.setup();
    previewMutateAsync.mockResolvedValueOnce(OK_PREVIEW_MANY);
    await openWithNoClassification(user);

    await user.type(screen.getByLabelText(/^Name/), "mask-pii");
    await user.type(screen.getByPlaceholderText("Search classifications by name…"), "PII");
    await user.click(await screen.findByRole("button", { name: /PII Data/ }));
    await user.click(screen.getByRole("button", { name: "Preview" }));

    expect(
      await screen.findByText(/Would recompile trino-prod, trino-staging, trino-eu/),
    ).toBeInTheDocument();
  });
});

describe("RuleDialog — full-body PATCH and verbatim refusals", () => {
  const EXISTING: Rule = {
    id: "r1",
    name: "mask-ssn",
    kind: { type: "mask", mask_type: "MASK_HASH" },
    selector: { type: "tag", key: "pii_v2" },
    exempt: { include: [{ kind: "user", ref: "alice" }], exclude: [] },
  };

  test("editing seeds every field and resends it unchanged", async () => {
    const user = userEvent.setup();
    renderDialog({ entry: { kind: "edit", rule: EXISTING } });

    expect(screen.getByDisplayValue("mask-ssn")).toBeInTheDocument();
    expect(screen.getByText("PII Data")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Preview" }));
    const saveBtn = await screen.findByRole("button", { name: "Save changes" });
    await waitFor(() => expect(saveBtn).toHaveAttribute("aria-disabled", "false"));
    await user.click(saveBtn);

    await waitFor(() => expect(patchMutateAsync).toHaveBeenCalled());
    expect(patchMutateAsync).toHaveBeenCalledWith({
      id: "r1",
      data: {
        name: "mask-ssn",
        kind: { type: "mask", mask_type: "MASK_HASH", value_expr: undefined },
        selector: { type: "tag", key: "pii_v2" },
        exempt: EXISTING.exempt,
      },
    });
  });

  test("a refusal from save is shown verbatim", async () => {
    const user = userEvent.setup();
    patchMutateAsync.mockRejectedValueOnce(
      new ApiError(409, "rule 'mask-ssn' would push profile 'trino-eu' to 2001 policies, over its 2000 ceiling"),
    );
    renderDialog({ entry: { kind: "edit", rule: EXISTING } });

    await user.click(screen.getByRole("button", { name: "Preview" }));
    const saveBtn = await screen.findByRole("button", { name: "Save changes" });
    await waitFor(() => expect(saveBtn).toHaveAttribute("aria-disabled", "false"));
    await user.click(saveBtn);

    expect(
      await screen.findByText(
        "rule 'mask-ssn' would push profile 'trino-eu' to 2001 policies, over its 2000 ceiling",
      ),
    ).toBeInTheDocument();
  });

  /**
   * J-F3: an exemption naming nobody egret knows is refused by the PREVIEW
   * (it runs the same `write_rule`), so that is where the author first sees it.
   * The save path is covered above; this is the preview's, which GrantDialog
   * already pins and this dialog did not.
   */
  test("a preview refusal naming an unknown exemption is shown verbatim, and blocks Save", async () => {
    const user = userEvent.setup();
    const REFUSAL =
      'bad request: these principal refs match nobody egret knows: user "ghost" (exempt.include[0]): no user has this username';
    previewMutateAsync.mockRejectedValueOnce(new ApiError(400, REFUSAL));
    renderDialog({ entry: { kind: "edit", rule: EXISTING } });

    await user.click(screen.getByRole("button", { name: "Preview" }));

    expect(await screen.findByText(REFUSAL)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save changes" })).toHaveAttribute("aria-disabled", "true");
  });

  const SENTENCE =
    'two mask policies would be served at policyPriority 0 that may both decide one request: rule "mask-ssn" and rule "null ssn".';

  test("the 409 sentence shows beside the form and nothing the user entered is cleared", async () => {
    const user = userEvent.setup();
    patchMutateAsync.mockRejectedValueOnce(new ApiError(409, SENTENCE));
    renderDialog({ entry: { kind: "edit", rule: EXISTING } });

    await user.click(screen.getByRole("button", { name: "Preview" }));
    const saveBtn = await screen.findByRole("button", { name: "Save changes" });
    await waitFor(() => expect(saveBtn).toHaveAttribute("aria-disabled", "false"));
    await user.click(saveBtn);

    expect(await screen.findByText(SENTENCE)).toBeInTheDocument();
    expect(screen.getByDisplayValue("mask-ssn")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save changes" })).toHaveAttribute("aria-disabled", "false");
  });
});

describe("RuleDialog — the fan-out's skip list (SP4b-2 finding 4)", () => {
  /**
   * **THE DISCRIMINATING CASE.** `SkipReport` carries no profile field, so one
   * tag rule skipped in two profiles comes back as two BYTE-IDENTICAL entries.
   * That is not an edge case: a tag selector is catalog-unscoped, so it is the
   * normal shape of the very fan-out this panel exists to show, and the
   * browser run hit it on the first tag rule it created —
   * `Encountered two children with the same key, rule-9c4c1e6f-…`, at error
   * level, with React then rendering one line where the preview reported two.
   *
   * Asserted two ways, because either alone is weak: React only WARNS about a
   * duplicate key (so the console is the only signal it happened), while the
   * rendered count is the consequence an operator actually sees.
   */
  test("renders both identical skip lines and logs no duplicate-key error", async () => {
    const skip = {
      source_kind: "rule",
      id: "9c4c1e6f-88bb-4baa-a12d-0b7093e2821d",
      reason: "profile has no usable `tagService`, so tag rule cannot compile here",
    };
    previewMutateAsync.mockResolvedValue({
      status: 200,
      data: {
        affected: [
          { id: 1, name: "trino-alpha" },
          { id: 2, name: "trino-beta" },
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
    await openWithNoClassification(user);
    await user.type(screen.getByLabelText(/^Name/), "mask-pii-columns");
    await user.type(screen.getByPlaceholderText("Search classifications by name…"), "PII");
    await user.click(await screen.findByRole("button", { name: /PII Data/ }));
    await user.click(screen.getByRole("button", { name: "Preview" }));

    await waitFor(() => expect(previewMutateAsync).toHaveBeenCalled());
    const lines = await screen.findAllByText(/cannot compile here/);
    expect(lines).toHaveLength(2);
    expect(
      errors.filter((a) => String(a[0]).includes("same key")),
      `React reported a duplicate key: ${JSON.stringify(errors)}`,
    ).toHaveLength(0);

    spy.mockRestore();
  });
});

describe("RuleDialog — it resolves the catalog's name itself (review M-6)", () => {
  /** The path-selector summary must name the catalog, for the same reason
   *  `GrantDialog`'s preview line must: the name comes from the Governance
   *  catalog list, so its resolution needs an assertion. */
  test("the path selector's summary names the catalog, not its uuid", async () => {
    const user = userEvent.setup();
    renderDialog();

    await user.click(screen.getByRole("combobox", { name: "Applies to" }));
    await user.click(await screen.findByRole("option", { name: "Path" }));
    await user.click(screen.getByRole("button", { name: "set-scope-complete" }));

    expect(screen.getByText(/Applies to wh\.sales\.orders/)).toBeInTheDocument();
    expect(screen.queryByText(/Applies to cat-1/)).not.toBeInTheDocument();
  });
});

describe("RuleDialog — a table entry seeds a path mask on the table", () => {
  test("opens on Path with the scope filled in, and is sent as given", async () => {
    const user = userEvent.setup();
    renderDialog({ entry: TABLE_ENTRY });

    expect(screen.getByRole("combobox", { name: "Applies to" })).toHaveTextContent("Path");
    expect(JSON.parse(screen.getByTestId("scope-value").textContent ?? "")).toEqual({
      catalog_id: "cat-1",
      schema: "sales",
      table: "orders",
    });
    expect(screen.getByText(/Applies to wh\.sales\.orders/)).toBeInTheDocument();
    // Catalog is the one level a path selector cannot do without.
    expect(screen.getByTestId("scope-picker")).toHaveAttribute("data-catalog-required", "true");

    await user.type(screen.getByLabelText(/^Name/), "mask-orders");
    await user.click(screen.getByRole("button", { name: "Preview" }));
    await waitFor(() => expect(previewMutateAsync).toHaveBeenCalled());
    expect(previewMutateAsync.mock.calls[0][0].data.rule.selector).toEqual({
      type: "path",
      catalog_id: "cat-1",
      schema: "sales",
      table: "orders",
    });
  });

  /** Row filter stays one click away, and the table's scope completes it (spec C8.3). */
  test("switching to Filter rows keeps the table's scope, which completes the filter", async () => {
    const user = userEvent.setup();
    renderDialog({ entry: TABLE_ENTRY });

    await user.type(screen.getByLabelText(/^Name/), "filter-orders");
    await user.click(screen.getByRole("combobox", { name: "Kind" }));
    await user.click(await screen.findByRole("option", { name: "Filter rows" }));
    await user.type(screen.getByLabelText(/^Filter expression/), "region = 'us'");

    expect(screen.getByTestId("scope-picker")).toHaveAttribute("data-row-filter-mode", "true");
    expect(screen.getByRole("button", { name: "Preview" })).toHaveAttribute("aria-disabled", "false");
    expect(screen.queryByText("Leave Column empty to mask every column of the table.")).not.toBeInTheDocument();
  });
});

describe("RuleDialog — Create always says why it is not available (C8.2)", () => {
  test("Create stays focusable and is described by the reason in the footer", async () => {
    const user = userEvent.setup();
    renderDialog({ entry: TABLE_ENTRY });
    const create = screen.getByRole("button", { name: "Create" });
    expect(create).toHaveAttribute("aria-disabled", "true");
    expect(create).not.toBeDisabled();
    expect(create).toHaveAccessibleDescription("Name the rule.");
    await user.type(screen.getByLabelText(/^Name/), "mask email");
    expect(create).toHaveAccessibleDescription("Preview the rule to enable Create.");
    await user.click(create);
    expect(createMutateAsync).not.toHaveBeenCalled();
    // The reason is text in the footer, beside the button, not only a description.
    const footer = screen.getByRole("group", { name: "Rule actions" });
    expect(within(footer).getByText("Preview the rule to enable Create.")).toBeInTheDocument();
  });

  test("Preview follows the same pattern: focusable, with the reason tied, and a click does nothing", async () => {
    const user = userEvent.setup();
    renderDialog({ entry: TABLE_ENTRY });
    const preview = screen.getByRole("button", { name: "Preview" });
    expect(preview).toHaveAttribute("aria-disabled", "true");
    expect(preview).not.toBeDisabled();
    expect(preview).toHaveAccessibleDescription("Name the rule.");
    await user.click(preview);
    expect(previewMutateAsync).not.toHaveBeenCalled();
  });

  test("after a preview, Create is available and carries no reason", async () => {
    const user = userEvent.setup();
    const { onOpenChange } = renderDialog({ entry: CLASSIFICATION_ENTRY });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    const create = screen.getByRole("button", { name: "Create" });
    await waitFor(() => expect(create).toHaveAttribute("aria-disabled", "false"));
    expect(create).not.toHaveAccessibleDescription();
    await user.click(create);
    await waitFor(() => expect(createMutateAsync).toHaveBeenCalled());
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  test("required labels say so", () => {
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    expect(screen.getByText("Name").closest("label")).toHaveTextContent("Name (required)");
    expect(screen.getByLabelText(/^Name/)).toHaveAttribute("aria-required", "true");
  });

  test("the classification picker and the filter expression are marked required", async () => {
    const user = userEvent.setup();
    await openWithNoClassification(user);
    expect(screen.getByText("Classification", { selector: "label" })).toHaveTextContent("Classification (required)");
    expect(screen.getByPlaceholderText("Search classifications by name…")).toHaveAttribute("aria-required", "true");

    await user.click(screen.getByRole("combobox", { name: "Kind" }));
    await user.click(await screen.findByRole("option", { name: "Filter rows" }));
    expect(screen.getByText("Filter expression").closest("label")).toHaveTextContent("Filter expression (required)");
    expect(screen.getByLabelText(/^Filter expression/)).toHaveAttribute("aria-required", "true");
  });

  test("a path mask says that an empty column masks every column", () => {
    renderDialog({ entry: TABLE_ENTRY });
    expect(screen.getByText("Leave Column empty to mask every column of the table.")).toBeInTheDocument();
  });
});

describe("RuleDialog — defaults per entry point (C8.3)", () => {
  test("from a classification: Mask, Redact, Applies to = that classification, named after it", () => {
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    expect(screen.getByLabelText(/^Name/)).toHaveValue("Mask PII Data");
    expect(screen.getByRole("combobox", { name: "Kind" })).toHaveTextContent("Mask a column");
    expect(screen.getByRole("combobox", { name: /mask type/i })).toHaveTextContent("Redact");
    expect(screen.getByRole("combobox", { name: "Applies to" })).toHaveTextContent("Classification");
    expect(screen.getByText("PII Data")).toBeInTheDocument();
  });

  test("from a table: Mask on the table's path, not a row filter", () => {
    renderDialog({ entry: TABLE_ENTRY });
    expect(screen.getByRole("combobox", { name: "Kind" })).toHaveTextContent("Mask a column");
    expect(JSON.parse(screen.getByTestId("scope-value").textContent ?? "{}")).toEqual({ catalog_id: "cat-1", schema: "sales", table: "orders" });
  });

  test("from a column with a classification: Applies to = that classification by default, the column's path one click away", async () => {
    const user = userEvent.setup();
    renderDialog({
      entry: {
        kind: "column",
        catalogId: "cat-1",
        schema: "sales",
        table: "orders",
        column: "email",
        classifications: [{ key: "pii_v2", name: "PII Data" }],
      },
    });
    expect(screen.getByRole("combobox", { name: "Kind" })).toHaveTextContent("Mask a column");
    expect(screen.getByRole("combobox", { name: "Applies to" })).toHaveTextContent("Classification");
    expect(screen.getByLabelText(/^Name/)).toHaveValue("Mask PII Data");
    expect(screen.getByText("PII Data")).toBeInTheDocument();

    // Preview needs nothing else: the default is a complete rule.
    await user.click(screen.getByRole("button", { name: "Preview" }));
    await waitFor(() => expect(previewMutateAsync).toHaveBeenCalled());
    expect(previewMutateAsync.mock.calls[0][0].data.rule.selector).toEqual({ type: "tag", key: "pii_v2" });

    // The column alone is one click away, with its path filled in.
    await user.click(screen.getByRole("button", { name: "Or apply it to column email only" }));
    expect(screen.getByRole("combobox", { name: "Applies to" })).toHaveTextContent("Path");
    expect(JSON.parse(screen.getByTestId("scope-value").textContent ?? "{}")).toEqual({
      catalog_id: "cat-1",
      schema: "sales",
      table: "orders",
      column: "email",
    });
    // And back.
    await user.click(screen.getByRole("button", { name: "Or apply it to PII Data" }));
    expect(screen.getByRole("combobox", { name: "Applies to" })).toHaveTextContent("Classification");
  });

  test("from a column with no classification: a mask on that column's path", () => {
    renderDialog({
      entry: { kind: "column", catalogId: "cat-1", schema: "sales", table: "orders", column: "email", classifications: [] },
    });
    expect(screen.getByRole("combobox", { name: "Applies to" })).toHaveTextContent("Path");
    expect(JSON.parse(screen.getByTestId("scope-value").textContent ?? "{}")).toEqual({
      catalog_id: "cat-1",
      schema: "sales",
      table: "orders",
      column: "email",
    });
  });

  test("from a rule page: the rule's own kind and selector", () => {
    renderDialog({
      entry: {
        kind: "edit",
        rule: {
          id: "r9",
          name: "filter-us",
          kind: { type: "row_filter", filter_expr: "region = 'us'" },
          selector: { type: "path", catalog_id: "cat-1", schema: "sales", table: "orders" },
          exempt: { include: [], exclude: [] },
        },
      },
    });
    expect(screen.getByRole("combobox", { name: "Kind" })).toHaveTextContent("Filter rows");
    expect(screen.getByLabelText(/^Filter expression/)).toHaveValue("region = 'us'");
    expect(screen.getByRole("combobox", { name: "Applies to" })).toHaveTextContent("Path");
  });
});

describe("RuleDialog — the draft survives Cancel (C8.4)", () => {
  test("Cancel on a dirty form asks first; Keep editing keeps everything; reopening restores it", async () => {
    const user = userEvent.setup();
    const entry = { kind: "table", catalogId: "cat-1", schema: "sales", table: "orders" } as const;
    const { onOpenChange, unmount } = renderDialog({ entry });
    await user.type(screen.getByLabelText(/^Name/), "mask email");
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    const ask = await screen.findByRole("alertdialog", { name: "Discard this rule?" });
    await user.click(within(ask).getByRole("button", { name: "Keep editing" }));
    expect(screen.getByLabelText(/^Name/)).toHaveValue("mask email");
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
    unmount(); // the user navigates away (a detour) with the dialog open
    renderDialog({ entry });
    expect(screen.getByLabelText(/^Name/)).toHaveValue("mask email");
    expect(screen.getByText(/Restored your unsaved rule/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Start over" }));
    expect(screen.getByLabelText(/^Name/)).toHaveValue("");
    expect(screen.queryByText(/Restored your unsaved rule/)).not.toBeInTheDocument();
  });

  test("Escape on a dirty form asks too, and Discard closes and forgets the draft", async () => {
    const user = userEvent.setup();
    const { onOpenChange, unmount } = renderDialog({ entry: TABLE_ENTRY });
    await user.type(screen.getByLabelText(/^Name/), "mask email");
    await user.keyboard("{Escape}");
    const ask = await screen.findByRole("alertdialog", { name: "Discard this rule?" });
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
    await user.click(within(ask).getByRole("button", { name: "Discard" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
    unmount();
    renderDialog({ entry: TABLE_ENTRY });
    expect(screen.getByLabelText(/^Name/)).toHaveValue("");
    expect(screen.queryByText(/Restored your unsaved rule/)).not.toBeInTheDocument();
  });

  test("a clean form closes on Cancel without asking", async () => {
    const user = userEvent.setup();
    const { onOpenChange } = renderDialog({ entry: CLASSIFICATION_ENTRY });
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  test("a draft belongs to its entry point: another table opens clean", async () => {
    const user = userEvent.setup();
    const { unmount } = renderDialog({ entry: TABLE_ENTRY });
    await user.type(screen.getByLabelText(/^Name/), "mask email");
    unmount();
    renderDialog({ entry: { kind: "table", catalogId: "cat-1", schema: "sales", table: "customers" } });
    expect(screen.getByLabelText(/^Name/)).toHaveValue("");
  });

  test("a successful Create forgets the draft", async () => {
    const user = userEvent.setup();
    const { onOpenChange, unmount } = renderDialog({ entry: TABLE_ENTRY });
    await user.type(screen.getByLabelText(/^Name/), "mask email");
    await user.click(screen.getByRole("button", { name: "Preview" }));
    const create = screen.getByRole("button", { name: "Create" });
    await waitFor(() => expect(create).toHaveAttribute("aria-disabled", "false"));
    await user.click(create);
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    unmount();
    renderDialog({ entry: TABLE_ENTRY });
    expect(screen.getByLabelText(/^Name/)).toHaveValue("");
  });
});

describe("RuleDialog — errors stay with the form (C8.5)", () => {
  test("a preview error renders in the footer, takes focus, and clears no field", async () => {
    const user = userEvent.setup();
    previewMutateAsync.mockRejectedValueOnce(new ApiError(400, "bad request: nope"));
    renderDialog({ entry: { kind: "classification", tagKey: "pii_v2", tagName: "PII Data" } });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    const footer = screen.getByRole("group", { name: "Rule actions" });
    const err = await within(footer).findByRole("alert");
    expect(err).toHaveTextContent("nope");
    expect(err.contains(document.activeElement) || err === document.activeElement).toBe(true);
    expect(screen.getByLabelText(/^Name/)).toHaveValue("Mask PII Data");
  });

  test("a save error renders in the footer too, and takes focus", async () => {
    const user = userEvent.setup();
    createMutateAsync.mockRejectedValueOnce(new ApiError(409, "rule name already used"));
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    const create = screen.getByRole("button", { name: "Create" });
    await waitFor(() => expect(create).toHaveAttribute("aria-disabled", "false"));
    await user.click(create);
    const footer = screen.getByRole("group", { name: "Rule actions" });
    const err = await within(footer).findByRole("alert");
    expect(err).toHaveTextContent("rule name already used");
    expect(err.contains(document.activeElement) || err === document.activeElement).toBe(true);
    expect(screen.getByLabelText(/^Name/)).toHaveValue("Mask PII Data");
  });
});

describe("RuleDialog — closing and drafts, round 1 review", () => {
  test("the close button on a dirty form asks first", async () => {
    const user = userEvent.setup();
    const { onOpenChange } = renderDialog({ entry: TABLE_ENTRY });
    await user.type(screen.getByLabelText(/^Name/), "mask email");
    await user.click(screen.getByRole("button", { name: "Close" }));
    expect(await screen.findByRole("alertdialog", { name: "Discard this rule?" })).toBeInTheDocument();
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });

  test("a click outside a dirty form asks first", async () => {
    const user = userEvent.setup();
    const { onOpenChange } = renderDialog({ entry: TABLE_ENTRY });
    await user.type(screen.getByLabelText(/^Name/), "mask email");
    // A modal dialog sets `pointer-events: none` on the page behind it, so user-event refuses
    // to click there; the press Radix listens for is dispatched directly (jsdom's pointer event
    // has no pointerType, so Radix waits for the click that follows, as on touch).
    fireEvent.pointerDown(document.body);
    fireEvent.click(document.body);
    expect(await screen.findByRole("alertdialog", { name: "Discard this rule?" })).toBeInTheDocument();
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });

  test("a draft belongs to its user: another account opens the same entry clean", async () => {
    const user = userEvent.setup();
    const { unmount } = renderDialog({ entry: TABLE_ENTRY });
    await user.type(screen.getByLabelText(/^Name/), "mask email");
    unmount();
    renderDialog({ entry: TABLE_ENTRY }, { ...ME, user_id: "u2" });
    expect(screen.getByLabelText(/^Name/)).toHaveValue("");
    expect(screen.queryByText(/Restored your unsaved rule/)).not.toBeInTheDocument();
  });

  test("with nobody signed in, nothing is kept", async () => {
    const user = userEvent.setup();
    const { unmount } = renderDialog({ entry: TABLE_ENTRY }, null);
    await user.type(screen.getByLabelText(/^Name/), "mask email");
    unmount();
    renderDialog({ entry: TABLE_ENTRY }, null);
    expect(screen.getByLabelText(/^Name/)).toHaveValue("");
  });

  test("the reason line is a polite live region", () => {
    renderDialog({ entry: TABLE_ENTRY });
    expect(screen.getByText("Name the rule.")).toHaveAttribute("aria-live", "polite");
  });

  test("focus lands on the error's title, and a refusal of an earlier version says so once the form changes", async () => {
    const user = userEvent.setup();
    previewMutateAsync.mockRejectedValueOnce(new ApiError(400, "bad request: nope"));
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    const title = await screen.findByRole("heading", { name: "The preview was refused." });
    expect(title).toHaveFocus();

    await user.type(screen.getByLabelText(/^Name/), "!");
    expect(screen.getByRole("heading", { name: "The preview of an earlier version was refused." })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create" })).toHaveAccessibleDescription("Preview the rule to enable Create.");
  });
});

describe("RuleDialog — an edit draft never overwrites a newer rule (round 1 review)", () => {
  const RULE: Rule = {
    id: "r1",
    name: "mask-ssn",
    kind: { type: "mask", mask_type: "MASK_HASH" },
    selector: { type: "tag", key: "pii_v2" },
    exempt: { include: [], exclude: [] },
  };
  const CHANGED: Rule = { ...RULE, exempt: { include: [{ kind: "user", ref: "bob" }], exclude: [] } };

  test("a draft of a rule changed since is dropped: the server's values show, with a notice", async () => {
    const user = userEvent.setup();
    const { unmount } = renderDialog({ entry: { kind: "edit", rule: RULE } });
    await user.type(screen.getByLabelText(/^Name/), "-v2");
    unmount();

    renderDialog({ entry: { kind: "edit", rule: CHANGED } });
    expect(screen.getByLabelText(/^Name/)).toHaveValue("mask-ssn");
    expect(screen.getByText("This rule changed since your draft; the draft was discarded.")).toBeInTheDocument();
    expect(screen.queryByText(/Restored your unsaved rule/)).not.toBeInTheDocument();
    expect(loadRuleDraft(ruleDraftKey("u1", { kind: "edit", rule: CHANGED }))).toBeUndefined();
  });

  test("a draft of an unchanged rule is restored", async () => {
    const user = userEvent.setup();
    const { unmount } = renderDialog({ entry: { kind: "edit", rule: RULE } });
    await user.type(screen.getByLabelText(/^Name/), "-v2");
    unmount();

    renderDialog({ entry: { kind: "edit", rule: { ...RULE } } });
    expect(screen.getByLabelText(/^Name/)).toHaveValue("mask-ssn-v2");
    expect(screen.getByText(/Restored your unsaved rule/)).toBeInTheDocument();
  });

  test("a refetch with changed data under an untouched form creates no draft, and the form follows it", () => {
    const { rerenderWith, unmount, onOpenChange } = renderDialog({ entry: { kind: "edit", rule: RULE } });
    rerenderWith({ entry: { kind: "edit", rule: CHANGED } });

    expect(screen.getByText("bob")).toBeInTheDocument();
    expect(loadRuleDraft(ruleDraftKey("u1", { kind: "edit", rule: CHANGED }))).toBeUndefined();
    unmount();
    expect(onOpenChange).not.toHaveBeenCalled();
    renderDialog({ entry: { kind: "edit", rule: CHANGED } });
    expect(screen.queryByText(/Restored your unsaved rule/)).not.toBeInTheDocument();
  });

  test("a refetch under an edited form keeps the edits, and their draft is still checked against the version they began from", async () => {
    const user = userEvent.setup();
    const { rerenderWith, unmount } = renderDialog({ entry: { kind: "edit", rule: RULE } });
    await user.type(screen.getByLabelText(/^Name/), "-v2");
    rerenderWith({ entry: { kind: "edit", rule: CHANGED } });
    expect(screen.getByLabelText(/^Name/)).toHaveValue("mask-ssn-v2");
    unmount();

    renderDialog({ entry: { kind: "edit", rule: CHANGED } });
    expect(screen.getByText("This rule changed since your draft; the draft was discarded.")).toBeInTheDocument();
  });
});

const EFFECT = { kind: "mask", catalogs: [{ catalogId: "cat-1", catalogName: "wh", engines: [], notEnforced: "no_engine" }], tableCount: 1, columnCount: 1, enforcedBy: 0, exemptMembers: 2, exemptEveryone: false, alreadyMasked: [], truncated: false };

describe("RuleDialog — the preview speaks in effect (C7)", () => {
  test("leads with the effect, warns when no engine enforces it, keeps profiles under details", async () => {
    const user = userEvent.setup();
    previewMutateAsync.mockResolvedValueOnce({ status: 200, data: { ...OK_PREVIEW_ONE.data, effect: EFFECT } });
    renderDialog({ entry: { kind: "classification", tagKey: "pii_v2", tagName: "PII Data" } });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByText("Masks 1 column on 1 table · 2 people exempt")).toBeInTheDocument();
    const warn = screen.getByRole("status");
    expect(warn).toHaveTextContent("Not enforced yet. wh has no engine attached");
    expect(screen.getByRole("button", { name: "Create" })).toHaveAccessibleDescription(/Not enforced yet/);
    // A warning, not a refusal: Create stays available.
    expect(screen.getByRole("button", { name: "Create" })).toHaveAttribute("aria-disabled", "false");
    const details = screen.getByText("What egret will recompile").closest("details");
    expect(details).not.toHaveAttribute("open");
    expect(details).toHaveTextContent("trino-prod");
  });

  test("an enforced rule shows no warning and no description on Create", async () => {
    const user = userEvent.setup();
    previewMutateAsync.mockResolvedValueOnce({
      status: 200,
      data: { ...OK_PREVIEW_ONE.data, effect: { ...EFFECT, enforcedBy: 1, catalogs: [] } },
    });
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByText("Masks 1 column on 1 table · enforced by 1 engine · 2 people exempt")).toBeInTheDocument();
    expect(screen.queryByText(/Not enforced yet/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create" })).not.toHaveAccessibleDescription();
  });
});

describe("RuleDialog — a second mask on a column is never silent (C6)", () => {
  test("names the mask already there, gives the per-group result, and asks for a tick", async () => {
    const user = userEvent.setup();
    const already = [{ ruleId: "r9", ruleName: "Hash personal data", maskType: "MASK_HASH", coordinate: "wh.analytics.customers.email", exempt: { include: [{ kind: "group", ref: "data-stewards" }], exclude: [] } }];
    previewMutateAsync.mockResolvedValueOnce({ status: 200, data: { ...OK_PREVIEW_ONE.data, effect: { ...EFFECT, enforcedBy: 1, catalogs: [], alreadyMasked: already } } });
    renderDialog({ entry: { kind: "classification", tagKey: "pii_v2", tagName: "PII Data" } });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByText(/wh\.analytics\.customers\.email is already masked by/)).toBeInTheDocument();
    expect(screen.getByText(/wh\.analytics\.customers\.email is already masked by/)).toHaveTextContent(
      "wh.analytics.customers.email is already masked by Hash personal data (Hash, except group data-stewards).",
    );
    expect(screen.getByRole("link", { name: "Hash personal data" })).toHaveAttribute("href", "/governance/rules/r9");
    expect(screen.getByRole("list", { name: "Result by exemption as written" })).toHaveTextContent("group data-stewards: undecided");
    // Groups are expanded only when compiled: the lines compare names as written, and say so.
    expect(screen.getByText("A person exempted under more than one name may see more than their line says.")).toBeInTheDocument();
    const create = screen.getByRole("button", { name: "Create" });
    expect(create).toHaveAttribute("aria-disabled", "true");
    expect(create).toHaveAccessibleDescription("Tick 'Create anyway' to create a second mask on this column.");
    await user.click(screen.getByRole("checkbox", { name: /Create anyway/ }));
    expect(screen.getByRole("checkbox", { name: /Create anyway/ })).toHaveAccessibleName(
      "Create anyway: I understand wh.analytics.customers.email will have two masks",
    );
    expect(create).not.toHaveAttribute("aria-disabled", "true");
  });

  test("a cut list says there may be more", async () => {
    const user = userEvent.setup();
    const already = [{ ruleId: "r9", ruleName: "Hash personal data", maskType: "MASK_HASH", coordinate: "wh.a.c.email", exempt: { include: [], exclude: [] } }];
    previewMutateAsync.mockResolvedValueOnce({
      status: 200,
      data: { ...OK_PREVIEW_ONE.data, effect: { ...EFFECT, enforcedBy: 1, catalogs: [], alreadyMasked: already, truncated: true } },
    });
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByText(/stopped at its limit, and more masks may reach these columns/)).toBeInTheDocument();
  });

  test("a cut check with no mask listed says so without a list or a tick", async () => {
    const user = userEvent.setup();
    previewMutateAsync.mockResolvedValueOnce({
      status: 200,
      data: { ...OK_PREVIEW_ONE.data, effect: { ...EFFECT, enforcedBy: 1, catalogs: [], truncated: true } },
    });
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByText("Other masks may reach these columns: the check stopped at its limit.")).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create" })).toHaveAttribute("aria-disabled", "false");
  });

  const mask = (ruleId: string, ruleName: string, coordinate: string) => ({
    ruleId, ruleName, maskType: "MASK_HASH", coordinate, exempt: { include: [], exclude: [] },
  });

  test("a tick holds for one preview: previewing again asks again", async () => {
    const user = userEvent.setup();
    const effect = { ...EFFECT, enforcedBy: 1, catalogs: [], alreadyMasked: [mask("r9", "Hash personal data", "wh.a.c.email")] };
    previewMutateAsync.mockResolvedValue({ status: 200, data: { ...OK_PREVIEW_ONE.data, effect } });
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    await user.click(await screen.findByRole("checkbox", { name: /Create anyway/ }));
    const create = screen.getByRole("button", { name: "Create" });
    expect(create).toHaveAttribute("aria-disabled", "false");
    await user.click(screen.getByRole("button", { name: "Preview" }));
    await waitFor(() => expect(previewMutateAsync).toHaveBeenCalledTimes(2));
    expect(await screen.findByRole("checkbox", { name: /Create anyway/ })).not.toBeChecked();
    expect(create).toHaveAttribute("aria-disabled", "true");
  });

  test("three masks on one column, and masks on several columns", async () => {
    const user = userEvent.setup();
    previewMutateAsync
      .mockResolvedValueOnce({
        status: 200,
        data: { ...OK_PREVIEW_ONE.data, effect: { ...EFFECT, enforcedBy: 1, catalogs: [], alreadyMasked: [mask("a", "Hash A", "wh.s.t.email"), mask("b", "Hash B", "wh.s.t.email")] } },
      })
      .mockResolvedValueOnce({
        status: 200,
        data: { ...OK_PREVIEW_ONE.data, effect: { ...EFFECT, enforcedBy: 1, catalogs: [], alreadyMasked: [mask("a", "Hash A", "wh.s.t.email"), mask("b", "Hash B", "wh.s.t.*")] } },
      });
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByText(/wh\.s\.t\.email is already masked by/)).toHaveTextContent(
      "wh.s.t.email is already masked by Hash A (Hash) and Hash B (Hash).",
    );
    expect(screen.getByRole("checkbox", { name: "Create anyway: I understand wh.s.t.email will have 3 masks" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByRole("list", { name: "Result by exemption as written on every column of wh.s.t" })).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "Result by exemption as written on wh.s.t.email" })).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Create anyway: I understand these columns will have more than one mask" })).toBeInTheDocument();
  });

  test("editing says Save anyway", async () => {
    const user = userEvent.setup();
    previewMutateAsync.mockResolvedValueOnce({
      status: 200,
      data: { ...OK_PREVIEW_ONE.data, effect: { ...EFFECT, enforcedBy: 1, catalogs: [], alreadyMasked: [mask("r9", "Hash personal data", "wh.a.c.email")] } },
    });
    renderDialog({
      entry: { kind: "edit", rule: { id: "r1", name: "m", kind: { type: "mask", mask_type: "MASK" }, selector: { type: "tag", key: "pii_v2" }, exempt: { include: [], exclude: [] } } },
    });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByRole("checkbox", { name: "Save anyway: I understand wh.a.c.email will have two masks" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save changes" })).toHaveAccessibleDescription(
      "Tick 'Save anyway' to save a second mask on this column.",
    );
  });

  test("a row filter's preview never asks for the mask tick", async () => {
    const user = userEvent.setup();
    previewMutateAsync.mockResolvedValueOnce({
      status: 200,
      data: { ...OK_PREVIEW_ONE.data, effect: { ...EFFECT, kind: "row_filter", enforcedBy: 1, catalogs: [], alreadyMasked: [mask("r9", "Hash", "wh.a.c.email")] } },
    });
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    await screen.findByText(/Filters rows of/);
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create" })).toHaveAttribute("aria-disabled", "false");
  });

  test("a 409 rule_overlap names both rules and offers no tick", async () => {
    const user = userEvent.setup();
    previewMutateAsync.mockRejectedValueOnce(new ApiError(409, "recompiling profile …", "rule_overlap", {
      error: "recompiling profile …", code: "rule_overlap", rules: [{ id: "a", name: "Hash A" }, { id: "b", name: "Null B" }], coordinate: "wh.s.t.email", kind: "mask",
    }));
    renderDialog({ entry: { kind: "classification", tagKey: "pii_v2", tagName: "PII Data" } });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByText(/would make two masks decide wh\.s\.t\.email differently/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Hash A" })).toHaveAttribute("href", "/governance/rules/a");
    expect(screen.getByRole("link", { name: "Null B" })).toHaveAttribute("href", "/governance/rules/b");
    expect(screen.getByText("Details").closest("details")).toHaveTextContent("recompiling profile …");
    expect(screen.queryByRole("checkbox", { name: /Create anyway/ })).not.toBeInTheDocument();
  });
});

describe("RuleDialog — the table-level classification conflict, said short (C5)", () => {
  const CONFLICT = () =>
    new ApiError(400, "tag \"PII Data\" is attached to the whole table…", "table_tag_mask_conflict", {
      error: "tag \"PII Data\" is attached to the whole table…", code: "table_tag_mask_conflict", tag: "PII Data", tagId: "t1",
      warehouseId: "wh-1", schema: "analytics", table: "orders", mask: "Mask PII Data", maskOrigin: "rule", moreTables: false,
    });

  test("one sentence, Remove from table, then a fresh preview", async () => {
    const user = userEvent.setup();
    const unassign = vi.fn().mockResolvedValue({ status: 200, data: { ok: true } });
    mockUnassign(unassign);
    previewMutateAsync
      .mockRejectedValueOnce(CONFLICT())
      .mockResolvedValueOnce({ status: 200, data: { ...OK_PREVIEW_ONE.data, effect: { ...EFFECT, enforcedBy: 2, catalogs: [] } } });
    renderDialog({ entry: { kind: "classification", tagKey: "pii_v2", tagName: "PII Data" } });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByText("PII Data is on the whole table analytics.orders, so masking by PII Data would make that table unreadable.")).toBeInTheDocument();
    expect(screen.getByText("Details").closest("details")).not.toHaveAttribute("open");
    expect(screen.queryByText(/Other tables carry it too/)).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Remove from table" }));
    // A destructive action confirms first (FE-ACTION-1).
    const ask = await screen.findByRole("alertdialog", { name: "Remove PII Data from analytics.orders?" });
    expect(unassign).not.toHaveBeenCalled();
    await user.click(within(ask).getByRole("button", { name: "Remove" }));
    expect(unassign).toHaveBeenCalledWith({ warehouseId: "wh-1", data: { schema: "analytics", table: "orders", column: "", tag_id: "t1" } });
    const fresh = await screen.findByText("Masks 1 column on 1 table · enforced by 2 engines · 2 people exempt");
    // The button that was pressed is gone; focus moves to the fresh result.
    await waitFor(() => expect(fresh.parentElement).toHaveFocus());
    expect(previewMutateAsync).toHaveBeenCalledTimes(2);
    expect(tagsApi.invalidateTagAssignment).toHaveBeenCalledWith(expect.anything(), "t1", "wh-1");
  });

  test("other tables are announced, and without catalog_registry:update Remove is replaced by the reason", async () => {
    const user = userEvent.setup();
    const err = CONFLICT();
    (err.body as { moreTables: boolean }).moreTables = true;
    previewMutateAsync.mockRejectedValueOnce(err);
    renderDialog({ entry: CLASSIFICATION_ENTRY }, { ...ME, platform_roles: [], permissions: ["policy:create"] });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByText("Other tables carry it too; each will be shown after this one.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove from table" })).not.toBeInTheDocument();
    expect(screen.getByText("Removing needs the catalog_registry:update permission.")).toBeInTheDocument();
  });

  test("Apply by path instead switches Applies to = Path with an empty scope", async () => {
    const user = userEvent.setup();
    previewMutateAsync.mockRejectedValueOnce(CONFLICT());
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    await user.click(await screen.findByRole("button", { name: "Apply by path instead" }));
    expect(screen.getByRole("combobox", { name: "Applies to" })).toHaveTextContent("Path");
    expect(JSON.parse(screen.getByTestId("scope-value").textContent ?? "{}")).toEqual({ catalog_id: "" });
  });

  test("Cancel in the confirm sends nothing and returns focus to Remove from table", async () => {
    const user = userEvent.setup();
    const unassign = vi.fn();
    mockUnassign(unassign);
    previewMutateAsync.mockRejectedValueOnce(CONFLICT());
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    await user.click(await screen.findByRole("button", { name: "Remove from table" }));
    const ask = await screen.findByRole("alertdialog", { name: "Remove PII Data from analytics.orders?" });
    await user.click(within(ask).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument());
    expect(unassign).not.toHaveBeenCalled();
    expect(previewMutateAsync).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.getByRole("button", { name: "Remove from table" })).toHaveFocus());
  });

  test("a refused Remove says why in the confirm, keeps the conflict, and previews nothing", async () => {
    const user = userEvent.setup();
    const unassign = vi.fn().mockRejectedValue(new ApiError(403, "forbidden"));
    mockUnassign(unassign);
    previewMutateAsync.mockRejectedValueOnce(CONFLICT());
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    await user.click(await screen.findByRole("button", { name: "Remove from table" }));
    const ask = await screen.findByRole("alertdialog", { name: "Remove PII Data from analytics.orders?" });
    await user.click(within(ask).getByRole("button", { name: "Remove" }));
    expect(await within(ask).findByText("This account cannot do that: it needs the catalog_registry:update permission.")).toBeInTheDocument();
    expect(previewMutateAsync).toHaveBeenCalledTimes(1);
    expect(tagsApi.invalidateTagAssignment).not.toHaveBeenCalled();
    await user.click(within(ask).getByRole("button", { name: "Cancel" }));
    expect(screen.getByText(/is on the whole table analytics\.orders/)).toBeInTheDocument();
  });

  test("a conflict refused at save keeps its actions until the form changes", async () => {
    const user = userEvent.setup();
    createMutateAsync.mockRejectedValueOnce(CONFLICT());
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    const create = screen.getByRole("button", { name: "Create" });
    await waitFor(() => expect(create).toHaveAttribute("aria-disabled", "false"));
    await user.click(create);
    expect(await screen.findByRole("button", { name: "Remove from table" })).toBeInTheDocument();
    await user.type(screen.getByLabelText(/^Name/), "!");
    expect(screen.getByText(/is on the whole table analytics\.orders/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove from table" })).not.toBeInTheDocument();
  });

  test("once the form changes, the earlier refusal is explained but offers nothing to act on", async () => {
    const user = userEvent.setup();
    previewMutateAsync.mockRejectedValueOnce(CONFLICT());
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByRole("button", { name: "Remove from table" })).toBeInTheDocument();
    await user.type(screen.getByLabelText(/^Name/), "!");
    expect(screen.getByText(/is on the whole table analytics\.orders/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove from table" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Apply by path instead" })).not.toBeInTheDocument();
  });

  test("an attach-path conflict (no warehouse) offers no Remove", async () => {
    const user = userEvent.setup();
    const err = CONFLICT();
    (err.body as { warehouseId: string | null }).warehouseId = null;
    previewMutateAsync.mockRejectedValueOnce(err);
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    expect(await screen.findByText(/is on the whole table analytics\.orders/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove from table" })).not.toBeInTheDocument();
  });
});

describe("RuleDialog — live walkthrough fixes (Task 13)", () => {
  test("after Preview, focus goes to the result, which is scrolled into view", async () => {
    const user = userEvent.setup();
    const scrolled = vi.fn();
    const saved = HTMLElement.prototype.scrollIntoView;
    HTMLElement.prototype.scrollIntoView = scrolled;
    onTestFinished(() => {
      HTMLElement.prototype.scrollIntoView = saved;
    });
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    await user.click(screen.getByRole("button", { name: "Preview" }));
    await waitFor(() => expect(document.activeElement).toHaveTextContent("What egret will recompile"));
    expect(scrolled).toHaveBeenCalled();
  });

  test("the classification-wide sentence shows only for a rule on a classification", async () => {
    const user = userEvent.setup();
    renderDialog({ entry: CLASSIFICATION_ENTRY });
    expect(screen.getByText(/applies in every catalog where the classification is attached/)).toBeInTheDocument();
    await user.click(screen.getByRole("combobox", { name: "Applies to" }));
    await user.click(await screen.findByRole("option", { name: "Path" }));
    expect(screen.queryByText(/every catalog where the classification|catalog-unscoped/)).not.toBeInTheDocument();
    expect(screen.getByText("Preview shows what this rule reaches and who enforces it, before you create it.")).toBeInTheDocument();
  });

  test("after Discard, focus returns to the control that opened the dialog", async () => {
    const user = userEvent.setup();
    function Opener() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button type="button" onClick={() => setOpen(true)}>
            open rule
          </button>
          <RuleDialog open={open} onOpenChange={setOpen} entry={TABLE_ENTRY} />
        </>
      );
    }
    const qc = new QueryClient();
    render(
      <AuthContext.Provider value={{ me: ME, isLoading: false }}>
        <QueryClientProvider client={qc}>
          <MemoryRouter>
            <Opener />
          </MemoryRouter>
        </QueryClientProvider>
      </AuthContext.Provider>,
    );
    await user.click(screen.getByRole("button", { name: "open rule" }));
    await user.type(await screen.findByLabelText(/^Name/), "x");
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Discard" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await waitFor(() => expect(screen.getByRole("button", { name: "open rule" })).toHaveFocus());
  });
});
