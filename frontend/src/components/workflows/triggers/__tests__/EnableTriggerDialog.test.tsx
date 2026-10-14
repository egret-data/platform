import { describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { EnableTriggerDialog } from "@/components/workflows/triggers/EnableTriggerDialog";
import { scheduleEntry } from "./triggerFixtures";

const overdue = (id: string, recover: string | null, missed = { count: 3, more: false }) =>
  scheduleEntry({ trigger_id: id, runtime_disabled: true }, { recover_missed_schedules: recover, missed_ticks: missed });

function renderDialog(props: Partial<Parameters<typeof EnableTriggerDialog>[0]> = {}) {
  const onCancel = vi.fn();
  const onEnable = vi.fn();
  render(
    <EnableTriggerDialog
      open
      count={1}
      bulk={false}
      schedules={[overdue("daily", null)]}
      busy={false}
      onCancel={onCancel}
      onEnable={onEnable}
      {...props}
    />,
  );
  return { onCancel, onEnable, dialog: screen.getByRole("dialog") };
}

describe("EnableTriggerDialog", () => {
  it.each([
    [null, "daily runs every missed tick, one per scheduler pass."],
    ["ALL", "daily runs every missed tick, one per scheduler pass."],
    ["LAST", "daily runs only the latest missed tick."],
    ["NONE", "daily skips them: this trigger's recoverMissedSchedules is NONE."],
  ])("describes Recover for recoverMissedSchedules %s", (raw, description) => {
    const { dialog } = renderDialog({ schedules: [overdue("daily", raw)] });
    expect(within(dialog).getByRole("heading", { name: "Enable trigger" })).toBeInTheDocument();
    const recover = within(dialog).getByRole("radio", { name: "Recover missed runs (3 missed)" });
    expect(recover).toHaveAccessibleDescription(description);
    const skip = within(dialog).getByRole("radio", { name: "Skip missed runs; start at the next tick" });
    expect(skip).toBeChecked();
    expect(recover).not.toBeChecked();
  });

  it("says 1000+ when the server counted more than it reports", () => {
    const { dialog } = renderDialog({ schedules: [overdue("daily", null, { count: 1000, more: true })] });
    expect(within(dialog).getByRole("radio", { name: "Recover missed runs (1000+ missed)" })).toBeInTheDocument();
  });

  // ui/label is `flex gap-2`: every child node of the label is its own flex
  // item and gets a gap, so "(", the count and "missed)" must be one run of
  // text inside one inline element. jsdom does no layout, so the gap itself
  // cannot be measured here; the test pins the structure that causes it
  // (the e2e measures the gap in a browser).
  it("labels Recover with a single text run, so the flex gap cannot split the count", () => {
    const { dialog } = renderDialog({ schedules: [overdue("daily", null, { count: 2, more: false })] });
    const recover = within(dialog).getByRole("radio", { name: "Recover missed runs (2 missed)" });
    const label = dialog.querySelector(`label[for="${recover.id}"]`);
    expect(label).not.toBeNull();
    const textBearing = Array.from(label!.childNodes).filter((n) => (n.textContent ?? "").trim() !== "");
    expect(textBearing).toHaveLength(1);
    expect(textBearing[0].nodeType).toBe(Node.ELEMENT_NODE);
    expect(textBearing[0].textContent).toBe("Recover missed runs (2 missed)");
  });

  it("sends each choice", async () => {
    const { dialog, onEnable } = renderDialog();
    await userEvent.click(within(dialog).getByRole("button", { name: "Enable" }));
    expect(onEnable).toHaveBeenLastCalledWith("skip");
    await userEvent.click(within(dialog).getByRole("radio", { name: /^Recover missed runs/ }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Enable" }));
    expect(onEnable).toHaveBeenLastCalledWith("recover");
  });

  it("Cancel sends nothing", async () => {
    const { dialog, onCancel, onEnable } = renderDialog();
    await userEvent.click(within(dialog).getByRole("radio", { name: /^Recover missed runs/ }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onEnable).not.toHaveBeenCalled();
  });

  it("in its bulk form, counts the triggers and describes each schedule that reads recovery differently", () => {
    const { dialog } = renderDialog({
      bulk: true,
      count: 3,
      schedules: [overdue("daily", "LAST"), overdue("hourly", null, { count: 5, more: false })],
    });
    expect(within(dialog).getByRole("heading", { name: "Enable 3 triggers" })).toBeInTheDocument();
    const recover = within(dialog).getByRole("radio", { name: "Recover missed runs (8 missed)" });
    expect(recover).toHaveAccessibleDescription(
      "daily runs only the latest missed tick. hourly runs every missed tick, one per scheduler pass.",
    );
  });
});
