import { describe, it, expect } from "vitest";
import { cleanup, render } from "@testing-library/react";
import { createElement } from "react";
import { SIMPLE_DESCRIPTORS } from "../simple";
import type { VizControlProps } from "../types";

/**
 * KI-053: one spelling of "color" across the chart editor. The Progress, Big number and Trend
 * Display tabs said "Colour", "Conditional colours" and "Switch positive / negative colours" while
 * every other panel says "color". Rendered through each descriptor's own Display controls, so a
 * label added to any of them later is held to the same spelling.
 */
describe("Display tab copy spells color one way", () => {
  for (const key of ["progress", "scalar", "smartscalar", "pie"]) {
    it(`${key}: no "colour" in the Display tab`, () => {
      const d = SIMPLE_DESCRIPTORS.find((x) => x.key === key)!;
      for (const control of d.controls.filter((c) => c.section === "display")) {
        const { container } = render(createElement(control.render, {
          config: {}, onChange: () => {}, columns: [], metrics: [],
          resultColumns: [{ name: "c", type: "varchar" }, { name: "m", type: "double" }],
          rows: [["a", 1]], temporalUnits: {}, columnSettings: {}, unitRow: () => null,
        } as unknown as VizControlProps<Record<string, unknown>>));
        const text = `${container.textContent ?? ""} ${[...container.querySelectorAll("[aria-label]")].map((e) => e.getAttribute("aria-label")).join(" ")}`;
        expect(text).not.toMatch(/colour/i);
        cleanup();
      }
    });
  }

  it("names the three controls the row lists with the US spelling", () => {
    const shown: string[] = [];
    for (const key of ["progress", "scalar", "smartscalar"]) {
      const d = SIMPLE_DESCRIPTORS.find((x) => x.key === key)!;
      for (const control of d.controls.filter((c) => c.section === "display")) {
        const { container } = render(createElement(control.render, {
          config: {}, onChange: () => {}, columns: [], metrics: [], resultColumns: [], rows: [],
          temporalUnits: {}, columnSettings: {}, unitRow: () => null,
        } as unknown as VizControlProps<Record<string, unknown>>));
        shown.push(container.textContent ?? "");
        cleanup();
      }
    }
    const all = shown.join(" ");
    expect(all).toContain("Color");
    expect(all).toContain("Conditional colors");
    expect(all).toContain("Switch positive / negative colors");
  });
});
