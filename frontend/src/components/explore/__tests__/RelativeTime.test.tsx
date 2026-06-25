import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { fullTimestamp } from "@/lib/relativeTime";

test("shows relative text with a full-timestamp tooltip", () => {
  const iso = new Date(Date.now() - 5 * 60_000).toISOString();
  render(<RelativeTime iso={iso} />);
  const el = screen.getByText(/ago$/);
  expect(el).toHaveAttribute("title", fullTimestamp(iso));
});
