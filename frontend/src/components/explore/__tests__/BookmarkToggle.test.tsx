import type { ReactNode } from "react";
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
// `ListToolbarButton` renders a Radix `Tooltip`, which throws without a
// provider above it. Production mounts one once in `AppShell`; every test in
// this tree supplies its own (see `ListToolbarButton.test.tsx`).
import { TooltipProvider } from "@/components/ui/tooltip";
import { BookmarkToggle, BookmarkStar } from "@/components/explore/BookmarkToggle";

const wrap = (ui: ReactNode) => render(<TooltipProvider>{ui}</TooltipProvider>);

describe("BookmarkToggle", () => {
  it("reads as pressed when the bookmarked-only filter is on", () => {
    const { rerender } = wrap(<BookmarkToggle active={false} onToggle={() => {}} />);
    expect(screen.getByRole("button", { name: "Bookmarks" })).toHaveAttribute("aria-pressed", "false");
    rerender(
      <TooltipProvider>
        <BookmarkToggle active onToggle={() => {}} />
      </TooltipProvider>,
    );
    expect(screen.getByRole("button", { name: "Bookmarks" })).toHaveAttribute("aria-pressed", "true");
  });

  it("calls onToggle, since the page — not the button — owns the filter state", async () => {
    const onToggle = vi.fn();
    wrap(<BookmarkToggle active={false} onToggle={onToggle} />);
    await userEvent.click(screen.getByRole("button", { name: "Bookmarks" }));
    expect(onToggle).toHaveBeenCalledTimes(1);
  });
});

describe("BookmarkStar", () => {
  it("names the row it belongs to, so 30 stars are not 30 identical controls", async () => {
    const onToggle = vi.fn();
    wrap(<BookmarkStar bookmarked={false} name="orders by region" onToggle={onToggle} />);
    const btn = screen.getByRole("button", { name: /orders by region/i });
    expect(btn).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(btn);
    expect(onToggle).toHaveBeenCalledTimes(1);
  });

  it("says which way the click goes", () => {
    wrap(<BookmarkStar bookmarked name="orders" onToggle={() => {}} />);
    // A star that is already filled must offer to REMOVE, not to add again.
    expect(screen.getByRole("button", { name: /remove bookmark/i })).toBeInTheDocument();
  });

  it("gives two rows two different accessible names", () => {
    // A one-row fixture cannot tell "the label names THIS row" apart from "the
    // label is a constant" — the defect that survived Task 2's four green tests.
    wrap(
      <>
        <BookmarkStar bookmarked={false} name="orders" onToggle={() => {}} />
        <BookmarkStar bookmarked name="revenue" onToggle={() => {}} />
      </>,
    );
    expect(screen.getByRole("button", { name: "Bookmark orders" })).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByRole("button", { name: "Remove bookmark from revenue" })).toHaveAttribute("aria-pressed", "true");
  });
});
