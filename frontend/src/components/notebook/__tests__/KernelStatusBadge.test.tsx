import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { KernelStatusBadge } from "../KernelStatusBadge";

describe("KernelStatusBadge", () => {
  it("labels each status with an accessible text (not color-only)", () => {
    const { rerender } = render(<KernelStatusBadge status="connecting" />);
    expect(screen.getByText("Connecting…")).toBeInTheDocument();
    rerender(<KernelStatusBadge status="error" />);
    expect(screen.getByText("Error")).toBeInTheDocument();
    rerender(<KernelStatusBadge status="idle" />);
    expect(screen.getByText("Ready")).toBeInTheDocument();
  });

  it("exposes a polite live region for screen readers", () => {
    render(<KernelStatusBadge status="busy" />);
    expect(screen.getByText("Running…").closest("[aria-live]")).toHaveAttribute("aria-live", "polite");
  });
});
