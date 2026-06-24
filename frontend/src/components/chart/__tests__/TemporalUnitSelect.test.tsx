import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { TemporalUnitSelect } from "../TemporalUnitSelect";

describe("TemporalUnitSelect", () => {
  it("renders the current unit label", () => {
    render(<TemporalUnitSelect value="month" onChange={() => {}} />);
    expect(screen.getByText("Month")).toBeTruthy();
  });

  it("shows 'None' as the placeholder when no unit is set", () => {
    render(<TemporalUnitSelect value={undefined} onChange={() => {}} />);
    expect(screen.getByText("None")).toBeTruthy();
  });
});
