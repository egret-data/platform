import { afterEach, describe, expect, it, vi } from "vitest";
import { focusHeadingOnArrival } from "@/lib/focusArrival";

afterEach(() => {
  vi.useRealTimers();
  document.body.innerHTML = "";
});

const h1 = (text: string) => {
  const h = document.createElement("h1");
  h.textContent = text;
  document.body.appendChild(h);
  return h;
};

describe("focusHeadingOnArrival", () => {
  it("focuses the new page's heading, not the old one, and follows a heading replaced while it held focus", () => {
    vi.useFakeTimers();
    const old = h1("Old page");
    focusHeadingOnArrival();
    vi.advanceTimersByTime(100);
    expect(document.activeElement).toBe(document.body);
    old.remove();
    const flash = h1("Rule");
    vi.advanceTimersByTime(100);
    expect(document.activeElement).toBe(flash);
    // That page unmounts; the real destination arrives.
    flash.remove();
    const dest = h1("PII");
    vi.advanceTimersByTime(100);
    expect(document.activeElement).toBe(dest);
    expect(dest.tabIndex).toBe(-1);
  });

  it("never takes focus the user moved elsewhere", () => {
    vi.useFakeTimers();
    h1("Old page").remove();
    focusHeadingOnArrival();
    const button = document.createElement("button");
    document.body.appendChild(button);
    button.focus();
    h1("Destination");
    vi.advanceTimersByTime(500);
    expect(document.activeElement).toBe(button);
  });
});
