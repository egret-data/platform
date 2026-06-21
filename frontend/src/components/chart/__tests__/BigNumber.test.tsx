import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { BigNumber } from "../BigNumber";

describe("BigNumber", () => {
  it("renders the formatted value and the metric caption", () => {
    render(<BigNumber columns={[{ name: "rev", type: "double" }]} rows={[[1234.5]]} metric="rev" format={{ prefix: "$", decimals: 0 }} />);
    expect(screen.getByText("$1,235")).toBeInTheDocument();
    expect(screen.getByText("rev")).toBeInTheDocument();
  });
  it("shows an em dash when there is no row", () => {
    render(<BigNumber columns={[{ name: "rev", type: "double" }]} rows={[]} metric="rev" />);
    expect(screen.getByText("—")).toBeInTheDocument();
  });
});
