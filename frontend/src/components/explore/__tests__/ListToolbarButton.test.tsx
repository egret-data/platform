import type { ReactElement } from "react";
import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Star } from "lucide-react";
import { ListToolbarButton } from "@/components/explore/ListToolbarButton";
import { TooltipProvider } from "@/components/ui/tooltip";

// `Tooltip` (Radix) throws without an ancestor `TooltipProvider`. In production
// that provider is mounted once in `AppShell`; every other test in this tree that
// renders a `Tooltip`-based component supplies its own (see Charts.test.tsx,
// Dashboards.test.tsx, Alerts.test.tsx, Datasets.trash.test.tsx), so this follows
// the same convention rather than reaching into the component to work around it.
function renderButton(ui: ReactElement) {
  return render(<TooltipProvider>{ui}</TooltipProvider>);
}

describe("ListToolbarButton", () => {
  it("exposes its label to assistive tech, since the button shows only an icon", () => {
    renderButton(<ListToolbarButton icon={Star} label="Bookmarks" onClick={() => {}} />);
    // The visible content is an icon. Without an accessible name the control is
    // unreachable by anything but sight, which is why `label` is required rather
    // than optional.
    expect(screen.getByRole("button", { name: "Bookmarks" })).toBeInTheDocument();
  });

  it("reads as pressed when active, so the toolbar shows which filters are on", () => {
    const { rerender } = renderButton(<ListToolbarButton icon={Star} label="Bookmarks" onClick={() => {}} />);
    expect(screen.getByRole("button", { name: "Bookmarks" })).toHaveAttribute("aria-pressed", "false");
    rerender(
      <TooltipProvider>
        <ListToolbarButton icon={Star} label="Bookmarks" active onClick={() => {}} />
      </TooltipProvider>,
    );
    expect(screen.getByRole("button", { name: "Bookmarks" })).toHaveAttribute("aria-pressed", "true");
  });

  it("calls onClick", async () => {
    const onClick = vi.fn();
    renderButton(<ListToolbarButton icon={Star} label="Trash" onClick={onClick} />);
    await userEvent.click(screen.getByRole("button", { name: "Trash" }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
