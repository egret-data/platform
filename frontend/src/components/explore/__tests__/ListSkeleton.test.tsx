import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { ListSkeleton } from "@/components/explore/ListSkeleton";

test("renders the requested number of skeleton rows", () => {
  render(<ListSkeleton rows={3} cols={2} />);
  expect(screen.getByTestId("list-skeleton").children.length).toBe(3);
});
