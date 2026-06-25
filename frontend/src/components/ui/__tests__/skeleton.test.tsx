import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { Skeleton } from "@/components/ui/skeleton";

test("renders an animated placeholder with passed classes", () => {
  render(<Skeleton data-testid="sk" className="h-4 w-10" />);
  const el = screen.getByTestId("sk");
  expect(el.className).toContain("animate-pulse");
  expect(el.className).toContain("h-4");
});
