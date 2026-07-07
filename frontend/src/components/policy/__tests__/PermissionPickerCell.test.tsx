import { render, screen, fireEvent } from "@testing-library/react";
import { describe, it, expect, vi } from "vitest";
import { PermissionPickerCell } from "../PermissionPickerCell";

describe("PermissionPickerCell", () => {
  it("renders the empty placeholder when there are no permissions", () => {
    render(<PermissionPickerCell value={[]} options={["select", "insert"]} onChange={vi.fn()} />);
    expect(screen.getByText("Add permissions")).toBeInTheDocument();
  });

  it("commits the buffered draft on confirm", () => {
    const onChange = vi.fn();
    render(<PermissionPickerCell value={[]} options={["select", "insert"]} onChange={onChange} />);

    fireEvent.click(screen.getByText("Add permissions"));
    fireEvent.click(screen.getByLabelText("select"));
    fireEvent.click(screen.getByLabelText("insert"));
    fireEvent.click(screen.getByLabelText("Confirm"));

    expect(onChange).toHaveBeenCalledWith(["select", "insert"]);
  });

  it("discards the buffered draft on cancel", () => {
    const onChange = vi.fn();
    render(<PermissionPickerCell value={["select"]} options={["select", "insert"]} onChange={onChange} />);

    fireEvent.click(screen.getByText("select"));
    fireEvent.click(screen.getByLabelText("insert"));
    fireEvent.click(screen.getByLabelText("Discard"));

    expect(onChange).not.toHaveBeenCalled();
  });
});
