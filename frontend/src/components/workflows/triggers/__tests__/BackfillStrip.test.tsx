import { describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Table, TableBody } from "@/components/ui/table";
import { BackfillCell, BackfillStrip } from "@/components/workflows/triggers/BackfillStrip";
import { backfill, scheduleEntry } from "./triggerFixtures";

function renderStrip(over: Parameters<typeof scheduleEntry>[0] = {}, props: Partial<Parameters<typeof BackfillStrip>[0]> = {}) {
  const row = { ...scheduleEntry({ backfill: backfill() }), ...over, flow_uuid: "flow-1" };
  const handlers = { onPause: vi.fn(), onDelete: vi.fn() };
  render(
    <Table>
      <TableBody>
        <BackfillStrip row={row} colSpan={10} name="daily" manage busy={false} {...handlers} {...props} />
      </TableBody>
    </Table>,
  );
  return handlers;
}

describe("BackfillStrip (W7f §4.1)", () => {
  it("shows the window, the progress as a bar with its value, and the ticks", () => {
    renderStrip();
    expect(screen.getByRole("progressbar", { name: "Backfill progress of daily" })).toHaveAttribute("aria-valuenow", "30");
    expect(screen.getByText("30% · 3 of 10 ticks")).toBeInTheDocument();
  });

  it("says Paused in words, not by colour alone, and offers Resume", async () => {
    const handlers = renderStrip({ backfill: backfill({ paused: true }) });
    expect(screen.getByText("Paused")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Resume" }));
    expect(handlers.onPause).toHaveBeenCalledWith(false);
  });

  it("Pause sends the opposite of the state the strip shows, and nothing else", async () => {
    const handlers = renderStrip();
    await userEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(handlers.onPause).toHaveBeenCalledWith(true);
    expect(handlers.onDelete).not.toHaveBeenCalled();
  });

  it("asks before Delete, names the ticks that will not run, and Cancel sends nothing", async () => {
    const handlers = renderStrip();
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    const dialog = screen.getByRole("alertdialog");
    expect(within(dialog).getByText(/Its remaining 7 ticks will not run/)).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(handlers.onDelete).not.toHaveBeenCalled();
  });

  it("Delete Confirm sends the delete once", async () => {
    const handlers = renderStrip();
    await userEvent.click(screen.getByRole("button", { name: "Delete" }));
    await userEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Delete backfill" }));
    expect(handlers.onDelete).toHaveBeenCalledTimes(1);
  });

  it("says Frozen while the trigger is off, and keeps Pause and Delete", () => {
    renderStrip({ runtime_disabled: true });
    expect(screen.getByText("Frozen while the trigger is off")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Pause" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Delete" })).toBeEnabled();
  });

  it("without workflow:manage, every action is off with its reason", () => {
    renderStrip({}, { manage: false });
    expect(screen.getByRole("button", { name: "Pause" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Delete" })).toBeDisabled();
    expect(screen.getAllByText("You need workflow:manage").length).toBeGreaterThan(0);
  });
});

describe("BackfillCell (W7f §4.1)", () => {
  it("is empty for a trigger that is not a schedule", () => {
    const { container } = render(<BackfillCell row={{ ...scheduleEntry(), kind: "webhook", flow_uuid: "f" }} manage />);
    expect(container).toBeEmptyDOMElement();
  });

  it("says Running for a running backfill and Paused for a paused one", () => {
    const { rerender } = render(<BackfillCell row={{ ...scheduleEntry({ backfill: backfill() }), flow_uuid: "f" }} manage />);
    expect(screen.getByText("Running")).toBeInTheDocument();
    rerender(<BackfillCell row={{ ...scheduleEntry({ backfill: backfill({ paused: true }) }), flow_uuid: "f" }} manage />);
    expect(screen.getByText("Paused")).toBeInTheDocument();
  });

  it("disables Backfill with the server's refusal in words", () => {
    render(
      <BackfillCell
        row={{ ...scheduleEntry({ can_backfill: false, backfill_refusal: "off_queue" }, { off_queue: true, last_error: "bad cron" }), flow_uuid: "f" }}
        manage
        onBackfill={() => undefined}
      />,
    );
    expect(screen.getByRole("button", { name: "Backfill" })).toBeDisabled();
    expect(screen.getByText("Off the queue: bad cron")).toBeInTheDocument();
  });

  it("shows a dash when no create form is wired here", () => {
    render(<BackfillCell row={{ ...scheduleEntry(), flow_uuid: "f" }} manage />);
    expect(screen.getByText("—")).toBeInTheDocument();
  });
});
