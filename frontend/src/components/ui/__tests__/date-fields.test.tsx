import { beforeAll, describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DateField, LocalDateTimeField, MonthField, TimeField } from "@/components/ui/date-time-picker";

// The pickers load their month grid lazily; load it once so no test pays for it.
beforeAll(async () => {
  await import("@/components/ui/calendar");
}, 30_000);

/** Holds the string value a consumer would hold and reports every change. */
function Host({ initial, children, spy }: { initial: string; spy: (v: string) => void; children: (p: { value: string; onChange: (v: string) => void }) => React.ReactNode }) {
  const [value, setValue] = useState(initial);
  return <>{children({ value, onChange: (v) => { spy(v); setValue(v); } })}</>;
}

const pad = (n: number) => String(n).padStart(2, "0");
const now = new Date();
const ym = `${now.getFullYear()}-${pad(now.getMonth() + 1)}`;

describe("string-valued date fields (KI-276)", () => {
  it("DateField emits a YYYY-MM-DD string and shows it", async () => {
    const spy = vi.fn();
    const user = userEvent.setup();
    render(<Host initial="" spy={spy}>{(p) => <DateField aria-label="Day" {...p} />}</Host>);
    await user.click(screen.getByRole("button", { name: "Day" }));
    await user.click(within(await screen.findByRole("grid")).getByRole("button", { name: /\b15th\b/ }));
    expect(spy).toHaveBeenLastCalledWith(`${ym}-15`);
    expect(screen.getByRole("button", { name: "Day" })).toHaveTextContent(`${ym}-15`);
  }, 15_000);

  it("DateField refuses days before min", async () => {
    const user = userEvent.setup();
    render(<DateField aria-label="Day" value="" onChange={() => {}} min={`${ym}-20`} />);
    await user.click(screen.getByRole("button", { name: "Day" }));
    const grid = await screen.findByRole("grid");
    expect(within(grid).getByRole("button", { name: /\b15th\b/ })).toBeDisabled();
    expect(within(grid).getByRole("button", { name: /\b21st\b/ })).toBeEnabled();
  }, 15_000);

  it("MonthField emits YYYY-MM", async () => {
    const spy = vi.fn();
    const user = userEvent.setup();
    render(<Host initial="2026-03" spy={spy}>{(p) => <MonthField aria-label="Month" {...p} />}</Host>);
    await user.click(screen.getByRole("button", { name: "Month" }));
    await user.click(screen.getByRole("button", { name: "Next year" }));
    await user.click(screen.getByRole("button", { name: "Jun" }));
    expect(spy).toHaveBeenLastCalledWith("2027-06");
  });

  it("LocalDateTimeField keeps seconds and returns a datetime-local string", async () => {
    const spy = vi.fn();
    const user = userEvent.setup();
    render(<Host initial={`${ym}-10T08:30:15`} spy={spy}>{(p) => <LocalDateTimeField aria-label="At" precision="second" {...p} />}</Host>);
    const trigger = screen.getByRole("button", { name: "At" });
    expect(trigger).toHaveTextContent(`${ym}-10 08:30:15`);
    await user.click(trigger);
    await user.click(within(await screen.findByRole("grid")).getByRole("button", { name: /\b15th\b/ }));
    expect(spy).toHaveBeenLastCalledWith(`${ym}-15T08:30:15`);
    const time = screen.getByLabelText("Time");
    await user.clear(time);
    await user.type(time, "23:59:58{Enter}");
    expect(spy).toHaveBeenLastCalledWith(`${ym}-15T23:59:58`);
  }, 15_000);

  it("LocalDateTimeField keeps milliseconds", async () => {
    render(<LocalDateTimeField aria-label="At" precision="millisecond" value={`${ym}-10T08:30:15.123`} onChange={() => {}} />);
    expect(screen.getByRole("button", { name: "At" })).toHaveTextContent(`${ym}-10 08:30:15.123`);
  });

  it("TimeField passes the typed HH:mm:ss through", async () => {
    const spy = vi.fn();
    render(<Host initial="" spy={spy}>{(p) => <TimeField aria-label="T" seconds {...p} />}</Host>);
    await userEvent.type(screen.getByLabelText("T"), "09:05:00");
    expect(spy).toHaveBeenLastCalledWith("09:05:00");
  });
});
