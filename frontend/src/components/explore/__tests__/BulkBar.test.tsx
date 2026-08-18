import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BulkBar, BulkToggle } from "@/components/explore/BulkBar";
import { TooltipProvider } from "@/components/ui/tooltip";

// `BulkToggle` now delegates to `ListToolbarButton`, which renders a Radix
// `Tooltip`. That throws without an ancestor `TooltipProvider` — in production
// the provider is mounted once in `AppShell`, and every other test in this tree
// that renders a `Tooltip`-based component supplies its own (see
// ListToolbarButton.test.tsx, Charts.test.tsx), so this follows the same
// convention.
describe("BulkToggle", () => {
  it("is an icon button carrying an accessible name, matching the rest of the toolbar", async () => {
    const onToggle = vi.fn();
    render(
      <TooltipProvider>
        <BulkToggle active={false} onToggle={onToggle} />
      </TooltipProvider>,
    );
    const btn = screen.getByRole("button", { name: "Bulk select" });
    expect(btn).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(btn);
    expect(onToggle).toHaveBeenCalledTimes(1);
  });

  it("reads as pressed when bulk mode is on", () => {
    render(
      <TooltipProvider>
        <BulkToggle active onToggle={() => {}} />
      </TooltipProvider>,
    );
    expect(screen.getByRole("button", { name: "Bulk select" })).toHaveAttribute("aria-pressed", "true");
  });
});

describe("BulkBar", () => {
  it("renders nothing at all when no rows are selected", () => {
    const { container } = render(
      <BulkBar count={0}>
        <button type="button">Move</button>
      </BulkBar>,
    );
    // Not "renders an empty bar": an empty bar still occupies vertical space and
    // shifts the table down the moment bulk mode is entered.
    expect(container).toBeEmptyDOMElement();
  });

  it("renders its actions and the selected count once rows are selected", () => {
    render(
      <BulkBar count={2}>
        <button type="button">Move</button>
      </BulkBar>,
    );
    expect(screen.getByRole("button", { name: "Move" })).toBeInTheDocument();
    expect(screen.getByText(/2 selected/i)).toBeInTheDocument();
  });
});
