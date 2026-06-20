import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { TableColumnsSection } from "../settings/TableColumnsSection";

const COLS = [{ name: "a", type: "varchar" }, { name: "b", type: "double" }];

describe("TableColumnsSection", () => {
  it("renders a checked checkbox per visible column", () => {
    render(<TableColumnsSection columns={COLS} value={{}} onChange={() => {}} />);
    expect(screen.getByLabelText("a")).toBeChecked();
    expect(screen.getByLabelText("b")).toBeChecked();
  });

  it("emits hiddenColumns when a column is unchecked", () => {
    const onChange = vi.fn();
    render(<TableColumnsSection columns={COLS} value={{}} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("a"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ hiddenColumns: ["a"] }));
  });

  it("emits pinnedLeft from the freeze input", () => {
    const onChange = vi.fn();
    render(<TableColumnsSection columns={COLS} value={{}} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("Freeze first columns"), { target: { value: "1" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ pinnedLeft: 1 }));
  });

  it("emits pagination enabled with default page size", () => {
    const onChange = vi.fn();
    render(<TableColumnsSection columns={COLS} value={{}} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Paginate"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ pagination: { enabled: true, pageSize: 25 } }));
  });
});
