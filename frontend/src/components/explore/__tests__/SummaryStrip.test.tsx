import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SummaryStrip } from "@/components/explore/SummaryStrip";

describe("SummaryStrip", () => {
  it("renders value then label", () => {
    render(<SummaryStrip items={[{ label: "charts", value: 4 }]} />);
    expect(screen.getByText("4")).toBeInTheDocument();
    expect(screen.getByText("charts")).toBeInTheDocument();
  });

  it("swaps in `singular` at exactly one, and back out at two", () => {
    // The whole point of Item 1: seven pages read "1 charts" today.
    const { rerender } = render(
      <SummaryStrip items={[{ label: "charts", singular: "chart", value: 1 }]} />,
    );
    expect(screen.getByText("chart")).toBeInTheDocument();
    expect(screen.queryByText("charts")).not.toBeInTheDocument();

    rerender(<SummaryStrip items={[{ label: "charts", singular: "chart", value: 2 }]} />);
    expect(screen.getByText("charts")).toBeInTheDocument();
    expect(screen.queryByText("chart")).not.toBeInTheDocument();
  });

  it("uses the plural label at zero", () => {
    // "0 chart" is the mirror-image defect of "1 charts", and a `value <= 1`
    // implementation would ship it.
    render(<SummaryStrip items={[{ label: "charts", singular: "chart", value: 0 }]} />);
    expect(screen.getByText("charts")).toBeInTheDocument();
    expect(screen.queryByText("chart")).not.toBeInTheDocument();
  });

  it("leaves a label with no `singular` alone at one", () => {
    // "live", "draft", "disabled", "active", "paused", "failing" are
    // ADJECTIVES. Nothing here derives a string from the label, so there is no
    // mechanism that could give one of them an `s` or take one off — the
    // opt-in field is what makes that unrepresentable rather than remembered.
    render(
      <SummaryStrip
        items={[
          { label: "live", value: 1 },
          { label: "draft", value: 1 },
          { label: "in Trash", value: 1 },
        ]}
      />,
    );
    expect(screen.getByText("live")).toBeInTheDocument();
    expect(screen.getByText("draft")).toBeInTheDocument();
    expect(screen.getByText("in Trash")).toBeInTheDocument();
  });

  it("does not treat a formatted string value as a count", () => {
    // `EngineDetail.tsx` passes `${readyPods}/${pods.length}` and a bare phase
    // name through the same prop. The singular rule is numeric identity, so a
    // value that merely LOOKS like one cannot trip it.
    render(<SummaryStrip items={[{ label: "pods", singular: "pod", value: "1" }]} />);
    expect(screen.getByText("pods")).toBeInTheDocument();
    expect(screen.queryByText("pod")).not.toBeInTheDocument();
  });

  it("keeps the same DOM node across a 1 -> 2 transition", () => {
    // Guards the `key`. `SummaryStripItem.label` is the count-INDEPENDENT half
    // of the pair by construction, so the key does not change when the
    // rendered word does; a count-aware `label` formatter would have remounted
    // the item at every 1<->2 crossing, losing focus on the toggle variants.
    const { rerender } = render(
      <SummaryStrip items={[{ label: "charts", singular: "chart", value: 1, onClick: vi.fn() }]} />,
    );
    const before = screen.getByRole("button");
    rerender(
      <SummaryStrip items={[{ label: "charts", singular: "chart", value: 2, onClick: vi.fn() }]} />,
    );
    expect(screen.getByRole("button")).toBe(before);
  });

  it("applies the singular inside the clickable toggle variant too", async () => {
    const onClick = vi.fn();
    render(
      <SummaryStrip
        items={[{ label: "folders", singular: "folder", value: 1, onClick, active: true }]}
      />,
    );
    // Real accessible-name computation, not a `textContent` read — and it is
    // what turned up the note below.
    //
    // The name is "1folder", with NO space: the number lives in its own
    // `<span>` and the separation on screen comes from the flex `gap-1`, which
    // the accessible-name computation does not see. That is pre-existing and
    // true of every toggle chip on this strip (Endpoints' live/draft/disabled,
    // Alerts' failing), not something the singular rule introduced. It is
    // pinned here rather than papered over with a hand-written `aria-label`,
    // and it is not fixed here because inserting a text node into an
    // `inline-flex` row adds a flex ITEM, whose spacing no jsdom test can
    // check.
    const btn = screen.getByRole("button", { name: "1folder" });
    expect(btn).toHaveAttribute("aria-pressed", "true");
    await userEvent.click(btn);
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
