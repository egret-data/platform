import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  REFRESH_OPTIONS,
  RefreshMenu,
  formatRemaining,
  ringPercent,
  secondsRemaining,
} from "../RefreshMenu";

describe("countdown maths", () => {
  it("counts whole seconds down from the refresh origin and clamps at both ends", () => {
    // refreshedAt is an epoch stamp, `now` is another; the countdown is the difference. Clamping
    // matters because both ends are reachable in the wild: a tab that slept past the interval
    // wakes up with a negative remainder, and a clock that jumped backwards produces one larger
    // than the period.
    expect(secondsRemaining(300, 1_000_000, 1_000_000)).toBe(300);
    expect(secondsRemaining(300, 1_000_000, 1_010_000)).toBe(290);
    expect(secondsRemaining(300, 1_000_000, 1_999_999)).toBe(0);
    expect(secondsRemaining(300, 1_000_000, 900_000)).toBe(300);
  });

  it("formats as m:ss with the second zero-padded", () => {
    // Metabase's RefreshWidgetTarget.tsx:44-50. "4:9" would be read as four minutes nine seconds
    // by nobody.
    expect(formatRemaining(300)).toBe("5:00");
    expect(formatRemaining(249)).toBe("4:09");
    expect(formatRemaining(59)).toBe("0:59");
    expect(formatRemaining(0)).toBe("0:00");
  });

  it("caps the ring at 0.95 so a full period still reads as a countdown", () => {
    // RefreshWidgetTarget.tsx:53. A 100% ring and a 0% ring are the same circle, so without the
    // cap a period that has only just started looks like one that has just expired.
    expect(ringPercent(300, 300)).toBe(0.95);
    expect(ringPercent(150, 300)).toBe(0.5);
    expect(ringPercent(0, 300)).toBe(0);
  });
});

describe("RefreshMenu", () => {
  it("lists Metabase's seven periods in order with the active one checked", async () => {
    // The VALUES are not a divergence — design §9 records that both sides have shipped
    // Off/1/5/10/15/30/60 all along. This asserts the CONTROL: a checked row, by role and name.
    const user = userEvent.setup();
    render(<RefreshMenu refreshSeconds={300} onChangeRefreshSeconds={vi.fn()} refreshedAt={0} onRefreshNow={vi.fn()} />);

    await user.click(screen.getByRole("button", { name: "Auto Refresh" }));
    const rows = await screen.findAllByRole("menuitemradio");
    expect(rows.map((r) => r.textContent)).toEqual([
      "Off", "1 minute", "5 minutes", "10 minutes", "15 minutes", "30 minutes", "60 minutes",
    ]);
    expect(screen.getByRole("menuitemradio", { name: "5 minutes" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("menuitemradio", { name: "Off" })).toHaveAttribute("aria-checked", "false");
    expect(REFRESH_OPTIONS.map((o) => o.seconds)).toEqual([null, 60, 300, 600, 900, 1800, 3600]);
  });

  it("reports the picked period in seconds, and null for Off", async () => {
    // Both directions, because a handler that always reports the row's label or always reports
    // null passes one of these and fails the other.
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<RefreshMenu refreshSeconds={null} onChangeRefreshSeconds={onChange} refreshedAt={0} onRefreshNow={vi.fn()} />);

    await user.click(screen.getByRole("button", { name: "Auto Refresh" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "10 minutes" }));
    expect(onChange).toHaveBeenCalledWith(600);

    await user.click(screen.getByRole("button", { name: "Auto Refresh" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "Off" }));
    expect(onChange).toHaveBeenLastCalledWith(null);
  });

  it("refreshes now without touching the period", async () => {
    // The manual path is the thing that did not exist: nothing but the interval ever bumped the
    // nonce (useDashboardViewControls.ts:27). Asking for one refresh must not schedule any.
    const user = userEvent.setup();
    const onRefreshNow = vi.fn();
    const onChange = vi.fn();
    render(<RefreshMenu refreshSeconds={null} onChangeRefreshSeconds={onChange} refreshedAt={0} onRefreshNow={onRefreshNow} />);

    await user.click(screen.getByRole("button", { name: "Auto Refresh" }));
    await user.click(await screen.findByRole("button", { name: "Refresh now" }));
    expect(onRefreshNow).toHaveBeenCalledTimes(1);
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe("RefreshMenu countdown", () => {
  beforeEach(() => vi.useFakeTimers({ shouldAdvanceTime: true }));
  afterEach(() => vi.useRealTimers());

  it("shows the time to the next refresh and ticks it down", () => {
    vi.setSystemTime(1_000_000);
    render(<RefreshMenu refreshSeconds={300} onChangeRefreshSeconds={vi.fn()} refreshedAt={1_000_000} onRefreshNow={vi.fn()} />);
    const trigger = screen.getByRole("button", { name: "Auto Refresh" });
    expect(trigger).toHaveAttribute("title", "Refreshing in 5:00");

    act(() => { vi.advanceTimersByTime(11_000); });
    expect(trigger).toHaveAttribute("title", "Refreshing in 4:49");
  });

  it("shows no countdown when auto-refresh is off, and starts no interval", () => {
    // The accessible NAME stays "Auto Refresh" whatever the clock says, so a test — and a screen
    // reader — can find the control without knowing the time.
    vi.setSystemTime(1_000_000);
    render(<RefreshMenu refreshSeconds={null} onChangeRefreshSeconds={vi.fn()} refreshedAt={1_000_000} onRefreshNow={vi.fn()} />);
    const trigger = screen.getByRole("button", { name: "Auto Refresh" });
    expect(trigger).toHaveAttribute("title", "Auto-refresh");
    expect(vi.getTimerCount()).toBe(0);
  });
});
