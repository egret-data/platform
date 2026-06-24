import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ObjectDetail } from "../ObjectDetail";

const cols = [{ name: "id", type: "int" }, { name: "plan", type: "text" }];
const rows = [[1, "Basic"], [2, "Premium"]];

describe("ObjectDetail", () => {
  it("renders the first record's field/value pairs", () => {
    render(<ObjectDetail columns={cols} rows={rows} />);
    expect(screen.getByText("id")).toBeTruthy();
    expect(screen.getByText("plan")).toBeTruthy();
    expect(screen.getByText("Basic")).toBeTruthy();
    expect(screen.getByText("1 / 2")).toBeTruthy();
  });

  it("pages to the next record and clamps the buttons", () => {
    render(<ObjectDetail columns={cols} rows={rows} />);
    const prev = screen.getByLabelText("previous record") as HTMLButtonElement;
    const next = screen.getByLabelText("next record") as HTMLButtonElement;
    expect(prev.disabled).toBe(true);
    fireEvent.click(next);
    expect(screen.getByText("Premium")).toBeTruthy();
    expect(screen.getByText("2 / 2")).toBeTruthy();
    expect((screen.getByLabelText("next record") as HTMLButtonElement).disabled).toBe(true);
  });

  it("hides the nav bar for a single record", () => {
    render(<ObjectDetail columns={cols} rows={[[1, "Basic"]]} />);
    expect(screen.queryByLabelText("next record")).toBeNull();
  });

  it("shows 'No data' for empty rows", () => {
    render(<ObjectDetail columns={cols} rows={[]} />);
    expect(screen.getByText("No data")).toBeTruthy();
  });
});
