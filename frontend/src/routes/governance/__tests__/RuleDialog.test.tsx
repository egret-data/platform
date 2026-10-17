import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, test, vi } from "vitest";

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
vi.mock("@/components/perm/ScopePicker", () => ({
  ScopePicker: ({
    value,
    onChange,
    rowFilterMode,
  }: {
    value: unknown;
    onChange: (v: unknown) => void;
    rowFilterMode?: boolean;
  }) => (
    <div data-testid="scope-picker" data-row-filter-mode={String(!!rowFilterMode)}>
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
import { ApiError } from "@/api/client";
import type { GovernanceCatalog, Rule } from "@/api/generated/model";
import { RuleDialog } from "@/routes/governance/RuleDialog";

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
let createMutateAsync: ReturnType<typeof vi.fn>;
let patchMutateAsync: ReturnType<typeof vi.fn>;

function renderDialog(props: Partial<React.ComponentProps<typeof RuleDialog>> = {}) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const onOpenChange = vi.fn();
  const utils = render(
    <QueryClientProvider client={qc}>
      <RuleDialog open onOpenChange={onOpenChange} {...props} />
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
  vi.mocked(tagsApi.useTags).mockReturnValue({ data: TAGS } as unknown as ReturnType<typeof tagsApi.useTags>);
  previewMutateAsync = vi.fn().mockResolvedValue(OK_PREVIEW_ONE);
  createMutateAsync = vi.fn().mockResolvedValue({ status: 201, data: undefined });
  patchMutateAsync = vi.fn().mockResolvedValue({ status: 200, data: {} });
  vi.mocked(permApi.usePermPreview).mockReturnValue({
    mutateAsync: previewMutateAsync,
    isPending: false,
  } as unknown as ReturnType<typeof permApi.usePermPreview>);
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
    renderDialog();

    await user.type(screen.getByLabelText("Name"), "mask-pii");
    await user.type(screen.getByPlaceholderText("Search tags by name…"), "PII");
    const option = await screen.findByRole("button", { name: /PII Data/ });
    expect(option).toHaveTextContent("stored as pii_v2");
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

    expect(screen.queryByLabelText("Custom expression")).not.toBeInTheDocument();

    await user.click(screen.getByRole("combobox", { name: /mask type/i }));
    await user.click(await screen.findByRole("option", { name: "Custom" }));
    expect(screen.getByLabelText("Custom expression")).toBeInTheDocument();

    await user.click(screen.getByRole("combobox", { name: /mask type/i }));
    await user.click(await screen.findByRole("option", { name: "Hash" }));
    expect(screen.queryByLabelText("Custom expression")).not.toBeInTheDocument();
  });
});

describe("RuleDialog — a row filter cannot be scoped to a column (D6)", () => {
  test("row_filter + path passes rowFilterMode to ScopePicker, and a forced column still blocks Save", async () => {
    const user = userEvent.setup();
    renderDialog();

    await user.type(screen.getByLabelText("Name"), "filter-region");
    await user.click(screen.getByRole("combobox", { name: "Kind" }));
    await user.click(await screen.findByRole("option", { name: "Filter rows" }));
    await user.type(screen.getByLabelText("Filter expression"), "region = 'us'");
    await user.click(screen.getByRole("combobox", { name: "Selects by" }));
    await user.click(await screen.findByRole("option", { name: "Path" }));

    expect(screen.getByTestId("scope-picker")).toHaveAttribute("data-row-filter-mode", "true");

    await user.click(screen.getByRole("button", { name: "set-scope-with-column" }));
    expect(
      screen.getByText(/A path-selected row filter needs exactly a catalog, schema and table/),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Preview" })).toBeDisabled();

    await user.click(screen.getByRole("button", { name: "set-scope-complete" }));
    expect(screen.getByRole("button", { name: "Preview" })).toBeEnabled();
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

    await user.type(screen.getByLabelText("Name"), "filter-region");
    await user.click(screen.getByRole("combobox", { name: "Selects by" }));
    await user.click(await screen.findByRole("option", { name: "Path" }));
    await user.click(screen.getByRole("button", { name: "set-scope-with-column" }));
    expect(screen.getByTestId("scope-value")).toHaveTextContent('"column":"ssn"');

    await user.click(screen.getByRole("combobox", { name: "Kind" }));
    await user.click(await screen.findByRole("option", { name: "Filter rows" }));
    await user.type(screen.getByLabelText("Filter expression"), "region = 'us'");

    // The column is gone from the form's own state, so the form is completable.
    expect(screen.getByTestId("scope-value")).not.toHaveTextContent('"column"');
    expect(
      screen.queryByText(/A path-selected row filter needs exactly a catalog, schema and table/),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Preview" })).toBeEnabled();
  });

  /** The reverse switch must NOT invent or keep anything: going back to `mask`
   *  leaves the column cleared rather than resurrecting it. */
  test("switching back to mask does not resurrect the dropped column", async () => {
    const user = userEvent.setup();
    renderDialog();

    await user.click(screen.getByRole("combobox", { name: "Selects by" }));
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
    renderDialog();

    await user.type(screen.getByLabelText("Name"), "mask-pii");
    await user.type(screen.getByPlaceholderText("Search tags by name…"), "PII");
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
    renderDialog({ existing: EXISTING });

    expect(screen.getByDisplayValue("mask-ssn")).toBeInTheDocument();
    expect(screen.getByText("PII Data")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Preview" }));
    const saveBtn = await screen.findByRole("button", { name: "Save changes" });
    await waitFor(() => expect(saveBtn).toBeEnabled());
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
    renderDialog({ existing: EXISTING });

    await user.click(screen.getByRole("button", { name: "Preview" }));
    const saveBtn = await screen.findByRole("button", { name: "Save changes" });
    await waitFor(() => expect(saveBtn).toBeEnabled());
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
    renderDialog({ existing: EXISTING });

    await user.click(screen.getByRole("button", { name: "Preview" }));

    expect(await screen.findByText(REFUSAL)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();
  });

  const SENTENCE =
    'two mask policies would be served at policyPriority 0 that may both decide one request: rule "mask-ssn" and rule "null ssn".';

  test("the 409 sentence shows beside the form and nothing the user entered is cleared", async () => {
    const user = userEvent.setup();
    patchMutateAsync.mockRejectedValueOnce(new ApiError(409, SENTENCE));
    renderDialog({ existing: EXISTING });

    await user.click(screen.getByRole("button", { name: "Preview" }));
    const saveBtn = await screen.findByRole("button", { name: "Save changes" });
    await waitFor(() => expect(saveBtn).toBeEnabled());
    await user.click(saveBtn);

    expect(await screen.findByText(SENTENCE)).toBeInTheDocument();
    expect(screen.getByDisplayValue("mask-ssn")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save changes" })).toBeEnabled();
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
    renderDialog();
    await user.type(screen.getByLabelText("Name"), "mask-pii-columns");
    await user.type(screen.getByPlaceholderText("Search tags by name…"), "PII");
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

    await user.click(screen.getByRole("combobox", { name: "Selects by" }));
    await user.click(await screen.findByRole("option", { name: "Path" }));
    await user.click(screen.getByRole("button", { name: "set-scope-complete" }));

    expect(screen.getByText(/Selects wh\.sales\.orders/)).toBeInTheDocument();
    expect(screen.queryByText(/Selects cat-1/)).not.toBeInTheDocument();
  });
});
