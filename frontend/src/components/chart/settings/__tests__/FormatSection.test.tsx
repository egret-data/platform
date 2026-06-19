import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FormatSection } from "../FormatSection";

describe("FormatSection", () => {
  it("writes a column format when the user picks a number style", () => {
    const onChange = vi.fn();
    render(
      <FormatSection
        columns={[{ name: "revenue", type: "double" }]}
        value={{}}
        onChange={onChange}
      />,
    );
    fireEvent.click(screen.getByText("revenue"));
    const styleSelect = screen.getByLabelText("Number style") as HTMLSelectElement;
    fireEvent.change(styleSelect, { target: { value: "currency" } });
    expect(onChange).toHaveBeenCalledWith({ revenue: { number_style: "currency" } });
  });
});
