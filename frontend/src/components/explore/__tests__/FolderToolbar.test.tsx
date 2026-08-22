import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
// `ListToolbarButton` is a Radix `Tooltip`, which throws without a provider
// above it. Production mounts one once in `AppShell`.
import { TooltipProvider } from "@/components/ui/tooltip";
import { FolderToolbar } from "@/components/explore/FolderToolbar";

describe("FolderToolbar", () => {
  it("offers a New folder control with an accessible name", async () => {
    const onNewFolder = vi.fn();
    render(
      <TooltipProvider>
        <FolderToolbar onNewFolder={onNewFolder} />
      </TooltipProvider>,
    );
    await userEvent.click(screen.getByRole("button", { name: "New folder" }));
    expect(onNewFolder).toHaveBeenCalledTimes(1);
  });

  // The button's only visible content is an icon, so the accessible name is the
  // whole of its label. A control that merely EXISTS is not the same as one a
  // screen-reader user can find, and the two are indistinguishable in a test
  // that queries by test id.
  it("is an idle control, not a pressed filter", () => {
    render(
      <TooltipProvider>
        <FolderToolbar onNewFolder={vi.fn()} />
      </TooltipProvider>,
    );
    // `aria-pressed` must be ABSENT, not `"false"`: `"false"` still announces
    // "toggle button, not pressed" to a screen reader for a control that has
    // no engaged state at all.
    expect(screen.getByRole("button", { name: "New folder" })).not.toHaveAttribute("aria-pressed");
  });
});
