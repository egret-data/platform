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
