import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DashboardEditToolbar } from "../DashboardEditToolbar";

function setup() {
  const props = {
    onAddChart: vi.fn(), onAddText: vi.fn(), onAddHeading: vi.fn(),
    onAddLink: vi.fn(), onAddIframe: vi.fn(), onAddSection: vi.fn(), onAddFilter: vi.fn(),
    // W3 added the width switch to this toolbar after the plan was written; the menu it feeds
    // is a plain Popover with no hooks, so it needs no Router or QueryClient here.
    width: "fixed" as const, onChangeWidth: vi.fn(),
  };
  render(<DashboardEditToolbar {...props} />);
  return props;
}

// userEvent, NOT fireEvent.click: a Radix DropdownMenuTrigger opens on `pointerdown`, and a bare
// `fireEvent.click` dispatches only `click` — the menu never mounts and every menuitem query
// fails with "unable to find role menuitem". Verified by running it that way first.
// `DashboardActionsMenu.view.test.tsx` reaches for userEvent for the same reason.
describe("DashboardEditToolbar link/iframe menu", () => {
  it("uses Metabase's label for the trigger", () => {
    // Verbatim from AddLinkOrEmbedButton.tsx:43-46, because this toolbar is measured against it.
    setup();
    expect(screen.getByRole("button", { name: "add a link or iframe" })).toBeInTheDocument();
  });

  it("offers Link and Iframe, in that order", async () => {
    setup();
    await userEvent.click(screen.getByRole("button", { name: "add a link or iframe" }));
    const items = screen.getAllByRole("menuitem").map((el) => el.textContent);
    expect(items).toStrictEqual(["Link", "Iframe"]);
  });

  it("adds a link from the menu, not from a bare button", async () => {
    // The regression this guards: turning the button into a menu without moving its handler
    // leaves a menu whose Link item does nothing.
    const props = setup();
    await userEvent.click(screen.getByRole("button", { name: "add a link or iframe" }));
    await userEvent.click(screen.getByRole("menuitem", { name: "Link" }));
    expect(props.onAddLink).toHaveBeenCalledTimes(1);
    expect(props.onAddIframe).not.toHaveBeenCalled();
  });

  it("adds an iframe from the menu", async () => {
    const props = setup();
    await userEvent.click(screen.getByRole("button", { name: "add a link or iframe" }));
    await userEvent.click(screen.getByRole("menuitem", { name: "Iframe" }));
    expect(props.onAddIframe).toHaveBeenCalledTimes(1);
    expect(props.onAddLink).not.toHaveBeenCalled();
  });
});
