import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FormatSection } from "../FormatSection";

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

  it("offers no numeric controls for a text column", () => {
    render(
      <FormatSection
        columns={[{ name: "name", type: "varchar" }]}
        value={{}}
        onChange={() => {}}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /name/ }));
    expect(screen.queryByLabelText("Number style")).not.toBeInTheDocument();
    expect(screen.getByText(/no format options/i)).toBeInTheDocument();
  });
});
