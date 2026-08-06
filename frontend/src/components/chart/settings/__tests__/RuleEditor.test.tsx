import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { RuleEditor } from "../RuleEditor";
import type { FormattingOperator } from "../../format-types";

// The column picker is a plain Popover + tick-list, so no data-fetching provider is needed.
const renderEditor = render;

describe("RuleEditor", () => {
  it("builds a single-color rule and saves it", () => {
    const onSave = vi.fn();
    renderEditor(
      <RuleEditor
        columns={[{ name: "revenue", type: "double" }, { name: "region", type: "varchar" }]}
        rule={{ columns: ["revenue"], type: "single", operator: ">", value: "100", color: "#ff0000" }}
        isPivot={false}
        onSave={onSave}
        onCancel={() => {}}
      />,
    );
    fireEvent.click(screen.getByText("Save rule"));
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ type: "single", operator: ">", value: "100", columns: ["revenue"] }));
  });
});

describe("RuleEditor column picker", () => {
  // It used to be one checkbox per column, so the list grew with the width of the result and
  // pushed the operator, value and colour off the panel on a wide table.
  it("collapses the column list behind a trigger that states the choice", () => {
    renderEditor(
      <RuleEditor
        columns={[{ name: "revenue", type: "double" }, { name: "region", type: "varchar" }]}
        rule={{ columns: ["revenue"], type: "single", operator: ">", value: "1", color: "#ff0000" }}
        isPivot={false}
        onSave={vi.fn()}
        onCancel={() => {}}
      />,
    );
    // closed: the trigger names the chosen column, and no column is a standing row
    expect(screen.getByRole("button", { name: "Columns" })).toHaveTextContent("revenue");
    expect(screen.queryByText("region")).not.toBeInTheDocument();
  });
});

describe("RuleEditor column picker — opened", () => {
  const columns = [
    { name: "revenue", type: "double" },
    { name: "orders", type: "bigint" },
    { name: "region", type: "varchar" },
  ];

  it("lists every column with its type once opened, and toggles a choice", async () => {
    const onSave = vi.fn();
    render(
      <RuleEditor
        columns={columns}
        rule={{ columns: ["revenue"], type: "single", operator: ">", value: "1", color: "#ff0000" }}
        isPivot={false}
        onSave={onSave}
        onCancel={() => {}}
      />,
    );
    await userEvent.click(screen.getByRole("button", { name: "Columns" }));
    expect(screen.getByText("orders")).toBeInTheDocument();
    expect(screen.getAllByText("bigint").length).toBeGreaterThan(0);

    await userEvent.click(screen.getByText("orders"));
    fireEvent.click(screen.getByText("Save rule"));
    expect(onSave).toHaveBeenCalledWith(
      expect.objectContaining({ columns: ["revenue", "orders"] }),
    );
  });

  it("summarises rather than listing when several columns are chosen", () => {
    render(
      <RuleEditor
        columns={columns}
        rule={{ columns: ["revenue", "orders", "region"], type: "single", operator: ">", value: "1", color: "#ff0000" }}
        isPivot={false}
        onSave={vi.fn()}
        onCancel={() => {}}
      />,
    );
    expect(screen.getByRole("button", { name: "Columns" })).toHaveTextContent("3 columns");
  });
});

/**
 * A COLOUR RANGE COULD NOT BE GIVEN A RANGE.
 *
 * `conditional.ts` has always honoured all four of `min_type`/`max_type` ("custom" and "all") and
 * `min_value`/`max_value` — a custom endpoint pins the gradient so two cards can be compared, and
 * "all" takes one extent across every column the rule targets instead of re-basing per column. The
 * editor hard-coded `{ min_type: null, max_type: null }` on creation and offered no control for any
 * of them, so the only reachable behaviour was the default.
 *
 * THE OPTION SET IS METABASE'S, read off the live reference at :3033 rather than guessed. With ONE
 * column selected it offers "Smallest value in this column" and "Custom value"; the moment a second
 * column is added the first becomes "…in each column" and a third option, "…in all of these
 * columns", appears. That conditional third option IS the rule: across one column "all" and "each"
 * compute the identical extent, so offering it would be a choice with no consequence.
 *
 * egret renders the choice as a `Select` where Metabase uses a radio group — the deliberate
 * deviation, because every other control in THIS editor (rule type, operator) is a Select and a
 * six-line radio group with labels this long reads as a different kind of control in a panel this
 * narrow. The labels, the values written and the one-versus-many rule are Metabase's exactly.
 */
describe("RuleEditor — a colour range's endpoints", () => {
  const numeric = [
    { name: "revenue", type: "double" },
    { name: "orders", type: "bigint" },
  ];
  const rangeRule = (columns: string[]) => ({
    columns, type: "range" as const, colors: ["#ffffff", "#3b82f6"],
    min_type: null, max_type: null,
  });

  function open(columns: string[], onSave = vi.fn()) {
    render(
      <RuleEditor columns={numeric} rule={rangeRule(columns)} isPivot={false}
        onSave={onSave} onCancel={() => {}} />,
    );
    return onSave;
  }

  it("offers a start and an end for the range", () => {
    open(["revenue"]);
    expect(screen.getByLabelText("Start the range at")).toBeInTheDocument();
    expect(screen.getByLabelText("End the range at")).toBeInTheDocument();
  });

  it("does not offer 'all of these columns' while one column is selected", async () => {
    open(["revenue"]);
    await userEvent.click(screen.getByLabelText("Start the range at"));
    expect(await screen.findByRole("option", { name: "Smallest value in this column" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /all of these columns/ })).not.toBeInTheDocument();
  });

  it("offers 'all of these columns' once a second column is selected", async () => {
    open(["revenue", "orders"]);
    await userEvent.click(screen.getByLabelText("Start the range at"));
    expect(await screen.findByRole("option", { name: "Smallest value in each column" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Smallest value in all of these columns" })).toBeInTheDocument();
  });

  /**
   * A stored number outlives the type that stopped using it, so re-choosing Custom must offer the
   * number back rather than the generic default. Rules arrive here from saved params and from
   * Metabase-shaped JSON, both of which carry `min_value`/`max_value` INDEPENDENTLY of
   * `min_type`/`max_type` — Metabase's own new range rule is `{min_type: null, min_value: 0,
   * max_type: null, max_value: 100}`, measured — so "had a custom start of 42, switched away,
   * switched back" is a real path and it must not silently become 0.
   */
  it("re-choosing Custom offers back the number the rule already carried", async () => {
    render(
      <RuleEditor columns={numeric} isPivot={false} onSave={vi.fn()} onCancel={() => {}}
        rule={{ ...rangeRule(["revenue"]), min_value: 42 }} />,
    );
    await userEvent.click(screen.getByLabelText("Start the range at"));
    await userEvent.click(await screen.findByRole("option", { name: "Custom value" }));
    expect(screen.getByLabelText("Range start value")).toHaveValue(42);
  });

  it("writes min_type: all", async () => {
    const onSave = open(["revenue", "orders"]);
    await userEvent.click(screen.getByLabelText("Start the range at"));
    await userEvent.click(await screen.findByRole("option", { name: "Smallest value in all of these columns" }));
    fireEvent.click(screen.getByText("Save rule"));
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ min_type: "all" }));
  });

  it("a custom endpoint reveals a value box and saves both halves of the answer", async () => {
    const onSave = open(["revenue"]);
    expect(screen.queryByLabelText("Range start value")).not.toBeInTheDocument();
    await userEvent.click(screen.getByLabelText("End the range at"));
    await userEvent.click(await screen.findByRole("option", { name: "Custom value" }));
    const box = screen.getByLabelText("Range end value");
    await userEvent.clear(box);
    await userEvent.type(box, "250");
    fireEvent.click(screen.getByText("Save rule"));
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ max_type: "custom", max_value: 250 }));
  });

  /**
   * `toStrictEqual`, not `objectContaining`: the job of choosing "each column" again after a custom
   * value is to LEAVE NOTHING BEHIND, and both `toEqual` and `toHaveBeenCalledWith` skip a property
   * whose value is `undefined` — so a rule that still carried `max_value: 250` under
   * `max_type: null` would pass a looser assertion while `conditional.ts` ignored the stale number
   * only by accident of its `max_type === "custom"` guard.
   */
  it("going back to a non-custom endpoint drops the custom number", async () => {
    const onSave = open(["revenue"]);
    await userEvent.click(screen.getByLabelText("End the range at"));
    await userEvent.click(await screen.findByRole("option", { name: "Custom value" }));
    await userEvent.type(screen.getByLabelText("Range end value"), "250");
    await userEvent.click(screen.getByLabelText("End the range at"));
    await userEvent.click(await screen.findByRole("option", { name: "Largest value in this column" }));
    fireEvent.click(screen.getByText("Save rule"));
    expect(onSave.mock.calls.at(-1)![0]).toStrictEqual({
      columns: ["revenue"], type: "range", colors: ["#ffffff", "#3b82f6"],
      min_type: null, max_type: null,
    });
  });
});

/**
 * `is-true` / `is-false` WERE IMPLEMENTED AND NO DROPDOWN COULD PRODUCE THEM.
 *
 * `conditional.ts`'s `matchOperator` handles both, and `NO_VALUE_OPS` here already knew to hide the
 * value box for them — but the operator list was a two-way choice between numeric and string, and
 * neither contained them. So the pair was reachable only by hand-editing a chart's stored params.
 *
 * IT IS AN ADDITION AND NOT A DELETION, and the deciding question was whether a boolean column can
 * reach `matchOperator` at all. It can: `ColumnMeta.type` is the ENGINE-reported type name, Trino
 * reports `boolean`, and `isNumericType`/`isTemporalType` both say no to it — so such a column is
 * offered to a rule today and lands on the STRING operator list, where the only way to ask "is this
 * true" is to type the word into a text box and rely on `matchOperator`'s `String(cell)` fallback.
 * Metabase names the same pair `BOOLEAN_OPERATOR_NAMES` for the same column class.
 *
 * (StarRocks is the asymmetry worth writing down: its connector reports the MySQL protocol type, so
 * a StarRocks `BOOLEAN` arrives as `MYSQL_TYPE_TINY` and `isNumericType` matches it on `tiny`. That
 * column gets the numeric operators, which is a protocol limitation recorded in `typegen.rs`, not
 * something this list can fix.)
 */
describe("RuleEditor — boolean columns", () => {
  const columns = [
    { name: "is_paid", type: "boolean" },
    { name: "revenue", type: "double" },
    { name: "region", type: "varchar" },
  ];
  const singleRule = (cols: string[], operator: FormattingOperator = ">") =>
    ({ columns: cols, type: "single" as const, operator, value: "", color: "#ef4444" });

  /**
   * NAMED BY LABEL, NOT BY TOKEN. These three assertions used to read `"is-true"`, `"is-false"` and
   * `">"` — the stored identifiers — because the dropdown printed them as its labels. They were
   * pinning the defect `RuleEditor — the operator dropdown reads as English` below closes, so when
   * that fix landed these failed and it was these that were wrong: an accessible name is what a
   * user hears, and no user should ever have heard "is-true". The tokens themselves are unchanged
   * and are still asserted where they belong, on what `onSave` stores.
   */
  it("offers is true / is false for a boolean column", async () => {
    render(<RuleEditor columns={columns} rule={singleRule(["is_paid"])} isPivot={false}
      onSave={vi.fn()} onCancel={() => {}} />);
    await userEvent.click(screen.getByLabelText("Operator"));
    expect(await screen.findByRole("option", { name: "is true" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "is false" })).toBeInTheDocument();
    // and not the ones that mean nothing about a boolean
    expect(screen.queryByRole("option", { name: "contains" })).not.toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "is greater than" })).not.toBeInTheDocument();
  });

  it("does not offer them for a text column", async () => {
    render(<RuleEditor columns={columns} rule={singleRule(["region"])} isPivot={false}
      onSave={vi.fn()} onCancel={() => {}} />);
    await userEvent.click(screen.getByLabelText("Operator"));
    expect(await screen.findByRole("option", { name: "contains" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "is true" })).not.toBeInTheDocument();
  });

  /**
   * FOUND WHILE WIRING THE ABOVE, and pre-existing. A new rule is minted with `operator: ">"`
   * (`ConditionalFormattingSection`), which is not in `STRING_OPS` — so choosing a text column left
   * the operator Select showing NOTHING (Radix renders no item for a value it has no option for)
   * and saving stored a `>` that `matchOperator` then evaluated as a string comparison. Adding a
   * boolean list makes a third way to land there, so the drift is resolved rather than widened: the
   * editor falls back to the first operator its current column selection actually offers.
   */
  it("an operator that the chosen columns cannot express falls back to one they can", () => {
    const onSave = vi.fn();
    render(<RuleEditor columns={columns} rule={singleRule(["region"], ">")} isPivot={false}
      onSave={onSave} onCancel={() => {}} />);
    // The trigger reads the LABEL and `onSave` writes the TOKEN — the two halves of the relabelling
    // below, asserted here on the same fallback they were already about.
    expect(screen.getByLabelText("Operator")).toHaveTextContent("is equal to");
    fireEvent.click(screen.getByText("Save rule"));
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ operator: "=" }));
  });
});

/**
 * THE OPERATOR DROPDOWN PRINTED ITS STORED TOKENS.
 *
 * `ops.map((op) => <SelectItem value={op}>{op}</SelectItem>)` used the identifier as the label, so
 * the panel read `does-not-contain` and `not-null` — kebab-case machine tokens shown to a user — and
 * `!=`, `>=` where Metabase reads "is not equal to" and "is greater than or equal to". Read off
 * both sides today: egret `= != < > <= >= contains does-not-contain starts-with ends-with is-null
 * not-null`; live Metabase at :3033 `is equal to / is not equal to / is less than / is greater than
 * / is less than or equal to / is greater than or equal to / contains / does not contain / starts
 * with / ends with / is null / is not null`.
 *
 * THE LABELS ARE `ALL_OPERATOR_NAMES` FROM
 * `visualizations/components/settings/ChartSettingsTableFormatting/get-operators-for-columns.ts`,
 * copied string for string rather than re-worded, including the one place the two families disagree
 * about the same token: `not-null` reads "is not null", not "not null".
 *
 * THE STORED VALUES DO NOT MOVE, and the assertions below say so on the same line as the label.
 * `conditional.ts`'s `matchOperator` switches on these exact tokens, `FormattingOperator` is their
 * type, and every saved chart's `conditional_formatting` already holds them — relabelling is a
 * one-file display change, while renaming would be a migration bought for nothing.
 */
describe("RuleEditor — the operator dropdown reads as English", () => {
  const numericRule = {
    columns: ["revenue"], type: "single", operator: ">", value: "1", color: "#ff0000",
  } as const;
  const openOperators = async (columns: { name: string; type: string }[], rule: unknown) => {
    render(
      <RuleEditor columns={columns} rule={rule as never} isPivot={false} onSave={vi.fn()} onCancel={() => {}} />,
    );
    await userEvent.click(screen.getByLabelText("Operator"));
  };

  it("labels the numeric operators the way Metabase does", async () => {
    await openOperators([{ name: "revenue", type: "double" }], numericRule);
    const labels = screen.getAllByRole("option").map((o) => o.textContent);
    expect(labels).toEqual([
      "is equal to", "is not equal to", "is less than", "is greater than",
      "is less than or equal to", "is greater than or equal to", "is null", "is not null",
    ]);
  });

  it("labels the string operators the way Metabase does", async () => {
    await openOperators([{ name: "region", type: "varchar" }], { ...numericRule, columns: ["region"] });
    const labels = screen.getAllByRole("option").map((o) => o.textContent);
    expect(labels).toEqual([
      "is equal to", "is not equal to", "contains", "does not contain",
      "starts with", "ends with", "is null", "is not null",
    ]);
  });

  it("labels the boolean pair the way Metabase does", async () => {
    await openOperators([{ name: "flag", type: "boolean" }], { ...numericRule, columns: ["flag"] });
    const labels = screen.getAllByRole("option").map((o) => o.textContent);
    expect(labels).toEqual(["is true", "is false", "is null", "is not null"]);
  });

  // The point of the whole change: the label moved and the stored token did not.
  it("still saves the kebab-case token the matcher switches on", async () => {
    const onSave = vi.fn();
    render(
      <RuleEditor
        columns={[{ name: "region", type: "varchar" }]}
        rule={{ columns: ["region"], type: "single", operator: "=", value: "x", color: "#ff0000" }}
        isPivot={false}
        onSave={onSave}
        onCancel={() => {}}
      />,
    );
    await userEvent.click(screen.getByLabelText("Operator"));
    await userEvent.click(await screen.findByRole("option", { name: "does not contain" }));
    await userEvent.click(screen.getByText("Save rule"));
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ operator: "does-not-contain" }));
  });

  // The trigger reads the label too — a Select showing "=" over options reading "is equal to" would
  // be the same defect one click later.
  it("shows the label rather than the token on the closed trigger", () => {
    render(
      <RuleEditor
        columns={[{ name: "region", type: "varchar" }]}
        rule={{ columns: ["region"], type: "single", operator: "does-not-contain", value: "x", color: "#ff0000" }}
        isPivot={false}
        onSave={vi.fn()}
        onCancel={() => {}}
      />,
    );
    expect(screen.getByLabelText("Operator")).toHaveTextContent("does not contain");
  });
});
