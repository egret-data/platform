import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { createElement } from "react";
import { SIMPLE_DESCRIPTORS } from "../simple";
import type { VizControlProps } from "../types";

/**
 * KI-056: before the first run the funnel's Steps list had nothing to list and rendered nothing,
 * so nothing said that the list will appear, nor that until it (or a sort) orders the stages they
 * come in the order the database returns them.
 */
const funnel = SIMPLE_DESCRIPTORS.find((d) => d.key === "funnel")!;
const steps = funnel.controls.find((c) => c.id === "steps")!;
const props = (rows: unknown[][], config: Record<string, unknown> = { dimension: "stage", metric: "visitors" }) => ({
  config, onChange: () => {}, columns: [], metrics: [],
  resultColumns: rows.length ? [{ name: "stage", type: "varchar" }, { name: "visitors", type: "bigint" }] : [],
  rows, temporalUnits: {}, columnSettings: {}, unitRow: () => null,
} as unknown as VizControlProps<Record<string, unknown>>);

describe("the funnel's Steps before and after the first run", () => {
  it("says where the steps will appear and what orders them until then", () => {
    render(createElement(steps.render, props([])));
    expect(screen.getByText("Steps")).toBeInTheDocument();
    expect(screen.getByText(/appear here after the first run/)).toBeInTheDocument();
    expect(screen.getByText(/in the order the database returns them/)).toBeInTheDocument();
  });

  it("lists the steps once there is a result", () => {
    render(createElement(steps.render, props([["Visited", 100], ["Paid", 10]])));
    expect(screen.getByText("Visited")).toBeInTheDocument();
    expect(screen.queryByText(/appear here after the first run/)).toBeNull();
  });

  it("says nothing while no step column is chosen", () => {
    const { container } = render(createElement(steps.render, props([], {})));
    expect(container).toBeEmptyDOMElement();
  });
});
