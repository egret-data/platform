import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FormatSection } from "../FormatSection";

describe("FormatSection", () => {
  it("shows numeric controls and writes a column format when a setting is edited", () => {
    const onChange = vi.fn();
    render(
      <FormatSection
        columns={[{ name: "revenue", type: "double" }]}
        value={{}}
        onChange={onChange}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "revenue" }));
    // numeric column → the Number style control is present
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
    expect(screen.queryByLabelText("Number style")).not.toBeInTheDocument();
    expect(screen.getByText(/no format options/i)).toBeInTheDocument();
  });
});
