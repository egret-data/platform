import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ChartPickerList } from "../ChartPickerList";

const chart = (id: string, name: string, viz = "bar") => ({ id, name, viz_type: viz, dataset_id: "d", description: "", created_by: null, updated_at: "", params: { spec: {} } }) as never;

describe("ChartPickerList", () => {
  it("filters rows by the search query", async () => {
    const user = userEvent.setup();
    render(<ChartPickerList charts={[chart("1", "Revenue bar"), chart("2", "Orders table", "table")]} onPick={() => {}} />);
    expect(screen.getByRole("button", { name: "Revenue bar" })).toBeTruthy();
    await user.type(screen.getByRole("textbox", { name: "search charts" }), "orders");
    expect(screen.queryByRole("button", { name: "Revenue bar" })).toBeNull();
    expect(screen.getByRole("button", { name: "Orders table" })).toBeTruthy();
  });
  it("calls onPick with the chart when a row is clicked", async () => {
    const user = userEvent.setup();
    const onPick = vi.fn();
    const c = chart("1", "Revenue bar");
    render(<ChartPickerList charts={[c]} onPick={onPick} />);
    await user.click(screen.getByRole("button", { name: "Revenue bar" }));
    expect(onPick).toHaveBeenCalledWith(c);
  });
  it("shows an empty state when no chart matches", async () => {
    const user = userEvent.setup();
    render(<ChartPickerList charts={[chart("1", "Revenue bar")]} onPick={() => {}} />);
    await user.type(screen.getByRole("textbox", { name: "search charts" }), "zzz");
    expect(screen.getByText("No charts")).toBeTruthy();
  });
});
