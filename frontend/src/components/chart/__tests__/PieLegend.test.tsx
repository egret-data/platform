// src/components/chart/__tests__/PieLegend.test.tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { PieLegend } from "../PieLegend";
import { PIE_LEGEND_COLUMN_WIDTH_PX, type PieLegendEntry } from "@/components/chart/echarts/options/pie";

const entries: PieLegendEntry[] = [
  { name: "Doohickey", color: "#509EE3", percent: "23.88%", value: 681, sliceDataIndex: 1 },
  { name: "Gadget", color: "#88BF4D", percent: "76.12%", value: 2171, sliceDataIndex: 5 },
];

const renderLegend = (props: Partial<Parameters<typeof PieLegend>[0]> = {}) =>
  render(
    <PieLegend
      entries={entries}
      placement="bottom"
      toggledOff={[]}
      onToggle={() => {}}
      {...props}
    />,
  );

describe("PieLegend", () => {
  // The whole reason the legend left the canvas: SVG <text> is not a list, and a screen reader
  // reading an ECharts legend gets a pile of unrelated text nodes. Real markup is the win.
  it("is a real list of buttons, one per entry", () => {
    renderLegend();
    const items = screen.getAllByRole("listitem");
    expect(items).toHaveLength(2);
    expect(items[0].closest("ul")).not.toBeNull();
    expect(screen.getAllByRole("button")).toHaveLength(2);
  });

  it("shows each entry's swatch colour, name and percentage", () => {
    renderLegend();
    expect(screen.getByText("Doohickey")).toBeInTheDocument();
    expect(screen.getByText("23.88%")).toBeInTheDocument();
    const swatch = screen.getByText("Doohickey").parentElement!.querySelector("[data-pie-legend-swatch]");
    expect(swatch).toHaveStyle({ background: "#509EE3" });
  });

  it("omits the percentage column entirely when the entries carry none", () => {
    renderLegend({ entries: [{ name: "Doohickey", color: "#509EE3", value: 681 }] });
    expect(screen.queryByText(/%$/)).toBeNull();
  });

  // Metabase's column is name-left, percentage-right, and a long name must not push the number
  // out of the column — it truncates with an ellipsis instead. jsdom does no layout, so what is
  // pinned here is the arrangement that produces it: a shrinkable, truncating name cell beside a
  // percentage cell that never shrinks.
  it("truncates a long name rather than pushing the percentage out of the column", () => {
    renderLegend({
      entries: [{ name: "A very long category name indeed", color: "#509EE3", percent: "23.88%", value: 681 }],
    });
    const name = screen.getByText("A very long category name indeed");
    expect(name.className).toContain("truncate");
    expect(name.className).toContain("min-w-0");
    expect(screen.getByText("23.88%").className).toContain("shrink-0");
    // …and the full text is still reachable, since the visible one may be cut.
    expect(name).toHaveAttribute("title", "A very long category name indeed");
  });

  it("occupies the reserved column when it sits beside the chart", () => {
    const { container } = renderLegend({ placement: "side" });
    const list = container.querySelector("ul")!;
    expect(list).toHaveAttribute("data-pie-legend", "side");
    expect(list).toHaveStyle({ width: `${PIE_LEGEND_COLUMN_WIDTH_PX}px` });
  });

  it("takes the full width and a bounded height when it sits below the chart", () => {
    const { container } = renderLegend({ placement: "bottom" });
    const list = container.querySelector("ul")!;
    expect(list).toHaveAttribute("data-pie-legend", "bottom");
    expect(list.getAttribute("style") ?? "").not.toContain("width");
    expect(list.className).toContain("overflow-y-auto");
  });

  // The ECharts legend this replaces toggled its slice on click (`legend.selectedMode` defaults
  // to `true`) and greyed the entry out; both are the component's job now.
  it("reports a click as a toggle of that entry", () => {
    const onToggle = vi.fn();
    renderLegend({ onToggle });
    fireEvent.click(screen.getByRole("button", { name: /Doohickey/ }));
    expect(onToggle).toHaveBeenCalledWith("Doohickey");
  });

  it("greys a toggled-off entry and says so to a screen reader", () => {
    renderLegend({ toggledOff: ["Doohickey"] });
    const off = screen.getByRole("button", { name: /Doohickey/ });
    const on = screen.getByRole("button", { name: /Gadget/ });
    expect(off).toHaveAttribute("aria-pressed", "false");
    expect(on).toHaveAttribute("aria-pressed", "true");
    expect(off.querySelector("[data-pie-legend-swatch]")).not.toHaveStyle({ background: "#509EE3" });
  });

  // Metabase greys every legend entry except the one naming the hovered slice's ring-0 ancestor —
  // which on a sunburst is the only thing connecting a slice four rings out to the entry that
  // names its colour family. The chart drives this one; the legend only renders it.
  describe("following the chart's hover", () => {
    it("fades every entry except the highlighted one", () => {
      renderLegend({ highlighted: "Gadget" });
      const faded = screen.getByRole("button", { name: /Doohickey/ });
      const lit = screen.getByRole("button", { name: /Gadget/ });
      expect(faded).toHaveAttribute("data-pie-legend-faded", "true");
      expect(faded.className).toContain("opacity-40");
      expect(lit).not.toHaveAttribute("data-pie-legend-faded");
      expect(lit.className).not.toContain("opacity-40");
    });

    it("fades nothing when nothing is highlighted", () => {
      const { container } = renderLegend({ highlighted: null });
      expect(container.querySelectorAll("[data-pie-legend-faded]")).toHaveLength(0);
    });

    // Fading is a momentary de-emphasis; toggled-off is a state the reader chose, and it stays
    // legible (and still says so to a screen reader) rather than being reduced to a ghost.
    it("is not the same treatment as a toggled-off entry", () => {
      renderLegend({ toggledOff: ["Doohickey"] });
      const off = screen.getByRole("button", { name: /Doohickey/ });
      expect(off).not.toHaveAttribute("data-pie-legend-faded");
      expect(off.className).not.toContain("opacity-40");
    });
  });

  // …and the other direction: pointing at an entry highlights its slice and swaps the chart's
  // centre readout to it. FOCUS counts as well as hover, because that readout is a value that
  // appears nowhere else — making it pointer-only would put it out of reach of a keyboard.
  describe("reporting its own hover", () => {
    it("reports the entry entered and cleared when it is left", () => {
      const onHover = vi.fn();
      renderLegend({ onHover });
      const entry = screen.getByRole("button", { name: /Doohickey/ });
      fireEvent.mouseEnter(entry);
      expect(onHover).toHaveBeenLastCalledWith("Doohickey");
      fireEvent.mouseLeave(entry);
      expect(onHover).toHaveBeenLastCalledWith(null);
    });

    it("reports a keyboard focus the same way", () => {
      const onHover = vi.fn();
      renderLegend({ onHover });
      const entry = screen.getByRole("button", { name: /Gadget/ });
      fireEvent.focus(entry);
      expect(onHover).toHaveBeenLastCalledWith("Gadget");
      fireEvent.blur(entry);
      expect(onHover).toHaveBeenLastCalledWith(null);
    });

    it("still toggles on click, hover handlers or not", () => {
      const onToggle = vi.fn();
      renderLegend({ onToggle, onHover: vi.fn() });
      fireEvent.click(screen.getByRole("button", { name: /Gadget/ }));
      expect(onToggle).toHaveBeenCalledWith("Gadget");
    });
  });

  it("renders nothing at all when there are no entries", () => {
    const { container } = renderLegend({ entries: [] });
    expect(container).toBeEmptyDOMElement();
  });
});
