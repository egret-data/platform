import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { QueryStatusBadge } from "@/components/sqllab/QueryStatusBadge";

test("renders the status label in sentence case", () => {
  render(<QueryStatusBadge status="finished" />);
  expect(screen.getByText("Finished")).toBeInTheDocument();
});

test("uses the success tone for finished and destructive for failed", () => {
  const { rerender } = render(<QueryStatusBadge status="finished" />);
  expect(screen.getByText("Finished")).toHaveAttribute("data-status-tone", "success");
  rerender(<QueryStatusBadge status="failed" />);
  expect(screen.getByText("Failed")).toHaveAttribute("data-status-tone", "destructive");
});
