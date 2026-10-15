import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { LabelsEditor } from "@/components/workflows/triggers/LabelsEditor";

describe("LabelsEditor (W7f §4.3)", () => {
  it("adds an empty row, and removes one by its name", async () => {
    const onChange = vi.fn();
    render(<LabelsEditor rows={[{ key: "team", value: "ops" }]} onChange={onChange} idPrefix="t" />);
    await userEvent.click(screen.getByRole("button", { name: "Add label" }));
    expect(onChange).toHaveBeenLastCalledWith([
      { key: "team", value: "ops" },
      { key: "", value: "" },
    ]);
    await userEvent.click(screen.getByRole("button", { name: "Remove label 1" }));
    expect(onChange).toHaveBeenLastCalledWith([]);
  });

  it("shows each row's error beside it, and ties its inputs to it", () => {
    render(
      <LabelsEditor
        rows={[
          { key: "system.x", value: "1" },
          { key: "team", value: "" },
        ]}
        onChange={() => undefined}
        idPrefix="t"
      />,
    );
    expect(screen.getByText("Keys starting with system. are reserved")).toBeInTheDocument();
    expect(screen.getByText("Fill both the key and the value")).toBeInTheDocument();
    expect(screen.getByLabelText("Key", { selector: "#t-label-key-0" })).toHaveAttribute("aria-invalid", "true");
  });
});
