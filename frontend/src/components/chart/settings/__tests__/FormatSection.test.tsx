import { useState } from "react";
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FormatSection } from "../FormatSection";
import type { ColumnFormat } from "../../format-types";

// Mirrors how a real caller wires FormatSection: `value` is state fed back from `onChange`, so
// typing genuinely round-trips through the controlled `value={fmt.column_title ?? ""}` input —
// unlike a static `value={{}}` + `onChange={vi.fn()}` render, which never re-renders the input and
// so could not reproduce the keystroke-eating regression this file guards against.
function StatefulFormatSection({ columns }: { columns: { name: string; type: string }[] }) {
  const [value, setValue] = useState<Record<string, ColumnFormat>>({});
  return <FormatSection columns={columns} value={value} onChange={setValue} />;
}

describe("FormatSection", () => {
  it("expands a numeric column on click and writes a format when a setting is edited", () => {
    const onChange = vi.fn();
    render(
      <FormatSection
        columns={[{ name: "revenue", type: "double" }]}
        value={{}}
        onChange={onChange}
      />,
    );
    // collapsed by default
    expect(screen.queryByLabelText("Number style")).not.toBeInTheDocument();
    // click the column row to expand its settings
    fireEvent.click(screen.getByRole("button", { name: /revenue/ }));
    expect(screen.getByLabelText("Number style")).toBeInTheDocument();
    // editing a native input writes that column's format
    fireEvent.change(screen.getByLabelText("Prefix"), { target: { value: "$" } });
    expect(onChange).toHaveBeenCalledWith({ revenue: { prefix: "$" } });
  });

  it("offers Display-as (not numeric controls) for a text column", () => {
    render(
      <FormatSection
        columns={[{ name: "name", type: "varchar" }]}
        value={{}}
        onChange={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /name/ }));
    expect(screen.queryByLabelText("Number style")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Display as")).toBeInTheDocument();
  });

  it("shows an empty Column title input when nothing is stored", () => {
    render(
      <FormatSection
        columns={[{ name: "total_revenue", type: "double" }]}
        value={{}}
        onChange={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /total_revenue/ }));
    expect(screen.getByLabelText("Column title")).toHaveValue("");
  });

  it("emits column_title when typing a new name", () => {
    const onChange = vi.fn();
    render(
      <FormatSection
        columns={[{ name: "total_revenue", type: "double" }]}
        value={{}}
        onChange={onChange}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /total_revenue/ }));
    fireEvent.change(screen.getByLabelText("Column title"), { target: { value: "Revenue" } });
    expect(onChange).toHaveBeenCalledWith({ total_revenue: { column_title: "Revenue" } });
  });

  it("emits column_title: undefined (not empty string) when the title is cleared", () => {
    const onChange = vi.fn();
    render(
      <FormatSection
        columns={[{ name: "total_revenue", type: "double" }]}
        value={{ total_revenue: { column_title: "Revenue" } }}
        onChange={onChange}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /total_revenue/ }));
    fireEvent.change(screen.getByLabelText("Column title"), { target: { value: "" } });
    expect(onChange).toHaveBeenCalledWith({ total_revenue: { column_title: undefined } });
  });

  it("offers a Column title input for a text column too, not only numeric", () => {
    render(
      <FormatSection
        columns={[{ name: "name", type: "varchar" }]}
        value={{}}
        onChange={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /name/ }));
    expect(screen.getByLabelText("Column title")).toBeInTheDocument();
  });

  it("shows a renamed column's title in its collapsed row without changing other rows", () => {
    render(
      <FormatSection
        columns={[
          { name: "total_revenue", type: "double" },
          { name: "region", type: "varchar" },
        ]}
        value={{ total_revenue: { column_title: "Revenue" } }}
        onChange={() => {}}
      />,
    );
    // renamed column's collapsed row shows the new title plus the real name
    const revenueButton = screen.getByRole("button", { name: /Revenue/ });
    expect(revenueButton).toHaveTextContent("Revenue");
    expect(revenueButton).toHaveTextContent("total_revenue");

    // the other column's collapsed row is untouched
    const regionButton = screen.getByRole("button", { name: "region Text" });
    expect(regionButton).not.toHaveTextContent("Revenue");

    // now open the renamed column's panel (making it the "open" column) and confirm
    // the OTHER (closed) row still shows its own name, not the open column's title
    fireEvent.click(revenueButton);
    expect(screen.getByRole("button", { name: "region Text" })).not.toHaveTextContent("Revenue");
  });

  it("treats a whitespace-only title as unset, not as a real (blank-rendering) title", () => {
    const onChange = vi.fn();
    render(
      <FormatSection
        columns={[{ name: "total_revenue", type: "double" }]}
        value={{}}
        onChange={onChange}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /total_revenue/ }));
    fireEvent.change(screen.getByLabelText("Column title"), { target: { value: "   " } });
    expect(onChange).toHaveBeenCalledWith({ total_revenue: { column_title: undefined } });
  });

  // Regression guard for a rejected fix: trimming column_title on every keystroke (rather than
  // only deciding blank-vs-not once, on the raw value) would eat the space in "Rev Total" as the
  // user typed it, because the input is controlled by the stored value and would re-render
  // without the trailing space before the next character lands. Driven via userEvent.type
  // (character by character, like SeriesLineStyleSection's tests do) through a stateful wrapper
  // so the value genuinely round-trips — fireEvent.change sets the whole string in one shot and,
  // with a static value prop, would not exercise the controlled-input feedback loop at all.
  it("keeps an inner space when typing a multi-word title", async () => {
    const user = userEvent.setup();
    render(<StatefulFormatSection columns={[{ name: "total_revenue", type: "double" }]} />);
    fireEvent.click(screen.getByRole("button", { name: /total_revenue/ }));
    await user.type(screen.getByLabelText("Column title"), "Rev Total");
    expect(screen.getByLabelText("Column title")).toHaveValue("Rev Total");
  });
});

describe("FormatSection — settings that had no control", () => {
  // `scale` and `number_separators` were both read by `formatValue` and offered nowhere, so they
  // could only be set by hand-editing stored JSON — implemented and unreachable.
  it("offers Multiply by a number and writes it through", async () => {
    const onChange = vi.fn();
    render(<FormatSection columns={[{ name: "rev", type: "bigint" }]} value={{}} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /rev/ }));
    fireEvent.change(screen.getByLabelText("Multiply by a number"), { target: { value: "100" } });
    expect(onChange).toHaveBeenCalledWith({ rev: { scale: 100 } });
  });

  it("clearing Multiply by a number removes it rather than storing 0", () => {
    const onChange = vi.fn();
    render(<FormatSection columns={[{ name: "rev", type: "bigint" }]} value={{ rev: { scale: 100 } }} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /rev/ }));
    fireEvent.change(screen.getByLabelText("Multiply by a number"), { target: { value: "" } });
    expect(onChange).toHaveBeenCalledWith({ rev: { scale: undefined } });
  });

  it("offers a separator style", () => {
    render(<FormatSection columns={[{ name: "rev", type: "bigint" }]} value={{}} onChange={vi.fn()} />);
    fireEvent.click(screen.getByRole("button", { name: /rev/ }));
    expect(screen.getByLabelText("Separator style")).toBeInTheDocument();
  });
});

/**
 * DECIMAL PLACES CANNOT BE ALLOWED TO STORE A VALUE THAT TAKES THE CHART DOWN.
 *
 * `formatValue` hands this straight to `Intl.NumberFormat` as both `minimumFractionDigits` and
 * `maximumFractionDigits`, which throw a `RangeError` outside 0..100 — and the throw fires during
 * the OPTION BUILD (`baseOption` formats the widest axis tick to size the gutter), so the card is
 * replaced by `ChartErrorBoundary`'s message rather than showing a wrong number. See
 * `echarts/__tests__/decimalsOutOfRange.echarts.test.ts` for the measured table and the stack.
 *
 * `min={0}` on the input does NOT prevent this. PR #398 established in a real Chrome probe that
 * `min`/`max`/`step` only decide `validity.*` flags at form submission and block no keystroke, so
 * the handler has to do the clamping — which is exactly what the pie's own Decimal places
 * (`PieDisplaySection`) has always done, and this box is the one that never copied it.
 *
 * Driven with `fireEvent.change` rather than `userEvent.type`: what is under test is what the
 * handler stores for a given input value, and `type="number"` inputs report an intermediate `-`
 * as `""`, which would make a character-by-character typing test assert about the browser's
 * sanitiser instead of about this handler.
 */
describe("FormatSection — Decimal places cannot store a value Intl rejects", () => {
  const openDecimals = (value: Record<string, ColumnFormat> = {}, onChange = vi.fn()) => {
    render(<FormatSection columns={[{ name: "rev", type: "double" }]} value={value} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /rev/ }));
    return { input: screen.getByLabelText("Decimal places"), onChange };
  };

  it("clamps a negative to 0 rather than storing the RangeError", () => {
    const { input, onChange } = openDecimals();
    fireEvent.change(input, { target: { value: "-1" } });
    expect(onChange).toHaveBeenCalledWith({ rev: { decimals: 0 } });
  });

  it("clamps above the maximum the box offers", () => {
    const { input, onChange } = openDecimals();
    fireEvent.change(input, { target: { value: "101" } });
    expect(onChange).toHaveBeenCalledWith({ rev: { decimals: 10 } });
  });

  it("truncates a fractional count instead of storing it", () => {
    const { input, onChange } = openDecimals();
    fireEvent.change(input, { target: { value: "2.7" } });
    expect(onChange).toHaveBeenCalledWith({ rev: { decimals: 2 } });
  });

  it.each([["3", 3], ["0", 0], ["10", 10]])("still stores a legal %s untouched", (typed, stored) => {
    const { input, onChange } = openDecimals();
    fireEvent.change(input, { target: { value: typed } });
    expect(onChange).toHaveBeenCalledWith({ rev: { decimals: stored } });
  });

  // Clearing the box is the only way back to "Auto" — a clamp that read `Number("")` as 0 would
  // take that away and pin the column to whole numbers instead. Rendered with a stored value so
  // the input is genuinely non-empty first: `fireEvent.change` to the value already on screen
  // dispatches nothing at all, and the assertion would pass without the handler running.
  it("still clears back to Auto", () => {
    const { input, onChange } = openDecimals({ rev: { decimals: 3 } });
    expect(input).toHaveValue(3);
    fireEvent.change(input, { target: { value: "" } });
    expect(onChange).toHaveBeenCalledWith({ rev: { decimals: undefined } });
  });

  // The bounds the browser DOES enforce — the spinner arrows and the mouse wheel step within
  // them — declared on the element so the control agrees with its own handler.
  it("declares the bounds it clamps to", () => {
    const { input } = openDecimals();
    expect(input).toHaveAttribute("min", "0");
    expect(input).toHaveAttribute("max", "10");
    expect(input).toHaveAttribute("step", "1");
  });
});

/**
 * THREE DATE/TIME FIELDS THE FORMATTER READ AND THE PANEL COULD NOT WRITE.
 *
 * `formatDate` consults `date_separator`, `time_enabled` and `time_style` — and `format.ts`'s
 * `CONSULTED_BY_FORMAT_DATE` marks all three `true`, which is as explicit as a codebase gets about
 * a setting mattering — while the temporal branch here offered only `date_style` and
 * `date_abbreviate`. A month bucket could be styled and a timestamp could not have its time shown
 * at all, which is the same shape of defect that had already cost `scale`, `number_separators` and
 * the pie's four settings.
 *
 * BOTH CONTROLS ARE CONDITIONAL, and that is this PR's own rule rather than the reference's. Live
 * Metabase at :3033 renders `date_separator` and `time_style` unconditionally — measured, with the
 * time set to Off and the radios still in the popover. egret does not, because `formatDate` reads
 * `time_style` ONLY inside `if (fmt.time_enabled)`, and applies `date_separator` only as
 * `dateFmt.replace(/\//g, …)` — so under "Off", and under a date style with no slash in it, each is
 * a control that saves a value and changes nothing. Offering that is precisely what Group A of this
 * PR removes elsewhere; it would be incoherent to add two more of them here for parity's sake. The
 * OPTION SETS and the labels are Metabase's exactly.
 */
describe("FormatSection — the temporal branch's missing controls", () => {
  const openDate = (value: Record<string, ColumnFormat> = {}, onChange = vi.fn()) => {
    render(<FormatSection columns={[{ name: "d", type: "timestamp" }]} value={value} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /d/ }));
    return onChange;
  };

  it("offers Show the time, and writes the bucket the formatter reads", async () => {
    const onChange = openDate();
    await userEvent.click(screen.getByLabelText("Show the time"));
    await userEvent.click(await screen.findByRole("option", { name: "HH:MM:SS" }));
    expect(onChange).toHaveBeenCalledWith({ d: { time_enabled: "seconds" } });
  });

  it("Off clears time_enabled rather than storing a string", async () => {
    const onChange = openDate({ d: { time_enabled: "minutes" } });
    await userEvent.click(screen.getByLabelText("Show the time"));
    await userEvent.click(await screen.findByRole("option", { name: "Off" }));
    expect(onChange.mock.calls.at(-1)![0]).toStrictEqual({ d: { time_enabled: null } });
  });

  it("hides Time style until the time is shown at all", async () => {
    openDate();
    expect(screen.queryByLabelText("Time style")).not.toBeInTheDocument();
  });

  it("offers Time style once the time is on, and writes the 24-hour clock", async () => {
    const onChange = openDate({ d: { time_enabled: "minutes" } });
    await userEvent.click(screen.getByLabelText("Time style"));
    await userEvent.click(await screen.findByRole("option", { name: "17:24 (24-hour clock)" }));
    expect(onChange).toHaveBeenCalledWith({ d: { time_enabled: "minutes", time_style: "HH:mm" } });
  });

  it("hides Date separator for a date style with no separator to replace", () => {
    openDate({ d: { date_style: "MMMM D, YYYY" } });
    expect(screen.queryByLabelText("Date separator")).not.toBeInTheDocument();
  });

  it("offers Date separator for a slashed date style, and writes it", async () => {
    const onChange = openDate({ d: { date_style: "M/D/YYYY" } });
    await userEvent.click(screen.getByLabelText("Date separator"));
    await userEvent.click(await screen.findByRole("option", { name: "2024-01-31" }));
    expect(onChange).toHaveBeenCalledWith({ d: { date_style: "M/D/YYYY", date_separator: "-" } });
  });

  // The default style is unslashed, so a stored separator on a style that cannot use it is
  // reachable only from an older chart — it must not resurrect the control.
  it("stays hidden for an unslashed style even when a separator is already stored", () => {
    openDate({ d: { date_style: "YYYY", date_separator: "." } });
    expect(screen.queryByLabelText("Date separator")).not.toBeInTheDocument();
  });
});
