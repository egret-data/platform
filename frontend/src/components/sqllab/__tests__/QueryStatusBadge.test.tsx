import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { QueryStatusBadge } from "@/components/sqllab/QueryStatusBadge";

test("renders the status label", () => {
  render(<QueryStatusBadge status="finished" />);
  expect(screen.getByText("finished")).toBeInTheDocument();
});

test("uses the success colour for finished and destructive for failed", () => {
  const { rerender } = render(<QueryStatusBadge status="finished" />);
  expect(screen.getByText("finished").className).toContain("bg-green-600");
  rerender(<QueryStatusBadge status="failed" />);
  expect(screen.getByText("failed").className).toContain("bg-destructive");
});
