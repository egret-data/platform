import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { BigNumber } from "../BigNumber";

const COLS = [{ name: "rev", type: "double" }];

describe("BigNumber", () => {
  it("renders the formatted value", () => {
    render(<BigNumber columns={COLS} rows={[[1234.5]]} metric="rev" format={{ prefix: "$", decimals: 0 }} />);
    expect(screen.getByText("$1,235")).toBeInTheDocument();
  });

  it("does NOT print the metric name under the value", () => {
    // Metabase does not print it either: the card header carries the chart's name, so the old
    // always-on caption said it twice on every dashboard. The name is still spoken (below).
    render(<BigNumber columns={COLS} rows={[[1234.5]]} metric="rev" />);
    expect(screen.queryByText("rev")).not.toBeInTheDocument();
  });

  it("shows an em dash when there is no row", () => {
    render(<BigNumber columns={COLS} rows={[]} metric="rev" />);
    expect(screen.getByText("—")).toBeInTheDocument();
  });

  it("names the metric and value as one utterance when it is not interactive", () => {
    render(<BigNumber columns={COLS} rows={[[1234.5]]} metric="rev" format={{ decimals: 0 }} />);
    expect(screen.getByRole("img", { name: "rev: 1,235" })).toBeInTheDocument();
  });

  describe("captions", () => {
    it("renders label and sublabel only when set", () => {
      const { rerender } = render(<BigNumber columns={COLS} rows={[[1]]} metric="rev" />);
      expect(screen.queryByText("Revenue")).not.toBeInTheDocument();

      rerender(<BigNumber columns={COLS} rows={[[1]]} metric="rev" label="Revenue" sublabel="this quarter" />);
      expect(screen.getByText("Revenue")).toBeInTheDocument();
      expect(screen.getByText("this quarter")).toBeInTheDocument();
    });
  });

  it("keeps the captions in the same centred group as the number", () => {
    // The live check found them stranded at the BOTTOM of the card, ~300px under the number,
    // because the value's box had `flex-1 basis-0` and swallowed the spare height. jsdom runs no
    // layout so it cannot see the gap — what it CAN see is the structure that caused it: the
    // measuring element must be absolutely positioned (out of flow), not a flex child that grows.
    const { container } = render(
      <BigNumber columns={COLS} rows={[[1234]]} metric="rev" label="Revenue" />,
    );
    const sizer = container.querySelector("[aria-hidden]");
    expect(sizer, "the measuring element should exist").not.toBeNull();
    expect(sizer!.className).toContain("absolute");
    expect(sizer!.className).not.toContain("flex-1");
  });

  describe("conditional colours", () => {
    const bands = [
      { min: 0, max: 1500, color: "#e75454", label: "Low" },
      { min: 1701, max: null, color: "#689735", label: "High" },
    ];

    it("colours the value with the matching band", () => {
      render(<BigNumber columns={COLS} rows={[[1000]]} metric="rev" format={{ decimals: 0 }} segments={bands} />);
      expect(screen.getByText("1,000")).toHaveStyle({ color: "#e75454" });
    });

    it("matches an open-ended band", () => {
      render(<BigNumber columns={COLS} rows={[[9000]]} metric="rev" format={{ decimals: 0 }} segments={bands} />);
      expect(screen.getByText("9,000")).toHaveStyle({ color: "#689735" });
    });

    it("prints the band's name as TEXT, not colour alone", () => {
      // `color-not-only`, High severity. Metabase hides these names in a hover tooltip, which
      // leaves a red and a green number indistinguishable to a colourblind reader.
      render(<BigNumber columns={COLS} rows={[[1000]]} metric="rev" segments={bands} />);
      expect(screen.getByText("Low")).toBeInTheDocument();
    });

    it("speaks the band name alongside the value", () => {
      render(<BigNumber columns={COLS} rows={[[1000]]} metric="rev" format={{ decimals: 0 }} segments={bands} />);
      expect(screen.getByRole("img", { name: "rev: 1,000, Low" })).toBeInTheDocument();
    });

    it("leaves the value on the default token when no band matches", () => {
      render(<BigNumber columns={COLS} rows={[[1600]]} metric="rev" format={{ decimals: 0 }} segments={bands} />);
      const el = screen.getByText("1,600");
      expect(el).not.toHaveStyle({ color: "#e75454" });
      expect(el.className).toContain("text-foreground");
    });
  });

  describe("drill", () => {
    it("is a button that reports a click when a handler is supplied", async () => {
      const onValueClick = vi.fn();
      render(<BigNumber columns={COLS} rows={[[1234]]} metric="rev" onValueClick={onValueClick} />);

      const button = screen.getByRole("button");
      await userEvent.click(button);

      expect(onValueClick).toHaveBeenCalledTimes(1);
      // A scalar has no dimensions, so it drills on "the records behind this number" — an empty
      // dim list, which `drillFilters` turns into no filters at all.
      expect(onValueClick.mock.calls[0][0]).toEqual({ dims: [] });
    });

    it("names the button with the value and the action", () => {
      render(<BigNumber columns={COLS} rows={[[1234]]} metric="rev" format={{ decimals: 0 }} onValueClick={vi.fn()} />);
      expect(screen.getByRole("button", { name: "rev: 1,234. Show records" })).toBeInTheDocument();
    });

    it("is not a button, and not an interactive role, without a handler", () => {
      render(<BigNumber columns={COLS} rows={[[1234]]} metric="rev" />);
      expect(screen.queryByRole("button")).not.toBeInTheDocument();
    });

    it("stays inert when there is no value to drill into", () => {
      render(<BigNumber columns={COLS} rows={[]} metric="rev" onValueClick={vi.fn()} />);
      expect(screen.queryByRole("button")).not.toBeInTheDocument();
    });
  });
});
