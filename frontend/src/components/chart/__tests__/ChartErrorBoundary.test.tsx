import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { ChartErrorBoundary } from "../ChartErrorBoundary";

function Boom(): never {
  throw new Error("kaboom");
}

describe("ChartErrorBoundary", () => {
  it("renders children normally when nothing throws", () => {
    render(<ChartErrorBoundary><div>ok content</div></ChartErrorBoundary>);
    expect(screen.getByText("ok content")).toBeTruthy();
  });

  it("catches a child render error and shows a contained fallback (does not re-throw)", () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(() => render(<ChartErrorBoundary><Boom /></ChartErrorBoundary>)).not.toThrow();
    expect(screen.getByText(/Couldn't render this chart/)).toBeTruthy();
    expect(screen.getByText(/kaboom/)).toBeTruthy();
    spy.mockRestore();
  });

  it("recovers (clears the error) when resetKey changes and the child no longer throws", () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    const { rerender } = render(<ChartErrorBoundary resetKey="a"><Boom /></ChartErrorBoundary>);
    expect(screen.getByText(/Couldn't render this chart/)).toBeTruthy();
    rerender(<ChartErrorBoundary resetKey="b"><div>recovered</div></ChartErrorBoundary>);
    expect(screen.getByText("recovered")).toBeTruthy();
    expect(screen.queryByText(/Couldn't render this chart/)).toBeNull();
    spy.mockRestore();
  });
});
