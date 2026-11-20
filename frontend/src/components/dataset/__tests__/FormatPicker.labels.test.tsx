import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FormatPicker } from "../FormatPicker";

/** KI-274: Decimals, Prefix and Suffix were `Label`s with no `htmlFor`, so they named nothing. */
describe("FormatPicker labels", () => {
  it("ties Decimals, Prefix and Suffix to their inputs on a number column", async () => {
    render(<FormatPicker value="" onChange={() => {}} type="bigint" />);
    await userEvent.setup().click(screen.getByRole("button", { name: "Format" }));
    expect(screen.getByLabelText("Decimals")).toHaveAttribute("type", "number");
    expect(screen.getByLabelText("Prefix")).toBeInTheDocument();
    expect(screen.getByLabelText("Suffix")).toBeInTheDocument();
  });
  it("ties Prefix and Suffix to their inputs on a text column", async () => {
    render(<FormatPicker value="" onChange={() => {}} type="varchar" />);
    await userEvent.setup().click(screen.getByRole("button", { name: "Format" }));
    expect(screen.getByLabelText("Prefix")).toBeInTheDocument();
    expect(screen.getByLabelText("Suffix")).toBeInTheDocument();
  });
});
