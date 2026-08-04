import { useState } from "react";
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { GaugeSegment } from "@/api/charts";
import { GaugeSegmentsSection } from "../GaugeSegmentsSection";
import { defaultGaugeSegments } from "@/components/chart/echarts/options/gauge";

function Stateful({ initial, value }: { initial?: GaugeSegment[]; value: number }) {
  const [segments, setSegments] = useState<GaugeSegment[] | undefined>(initial);
  return <GaugeSegmentsSection segments={segments} value={value} onChange={setSegments} />;
}

describe("GaugeSegmentsSection", () => {
  it("with nothing stored, shows today's three default zones — what the renderer would draw", () => {
    // Reuses the renderer's own default-zone logic (task 3's `defaultGaugeSegments`) rather
    // than re-deriving the thirds-of-[0, value*2] numbers here.
    const expected = defaultGaugeSegments(60);
    render(<GaugeSegmentsSection segments={undefined} value={60} onChange={() => {}} />);
    expect(screen.getAllByLabelText(/^range \d+ min$/)).toHaveLength(3);
    // Strings, not numbers: the bounds moved to `DecimalInput`, which is a TEXT input on purpose
    // (a `type="number"` box reports itself empty for as long as a decimal point is the last
    // character typed, so it cannot hold a half-written bound). `toHaveValue` returns the raw
    // string for a text input and a coerced number for a number input, so this is the same
    // assertion said in the new field's own terms — it is still the displayed bound.
    expected.forEach((seg, i) => {
      expect(screen.getByLabelText(`range ${i + 1} min`)).toHaveValue(String(seg.min));
      expect(screen.getByLabelText(`range ${i + 1} max`)).toHaveValue(String(seg.max));
    });
  });

  it("editing a bound emits the whole segments array with only that value changed", () => {
    const segments: GaugeSegment[] = [
      { min: 0, max: 30, color: "#111111" },
      { min: 30, max: 70, color: "#222222" },
      { min: 70, max: 100, color: "#333333" },
    ];
    const onChange = vi.fn();
    render(<GaugeSegmentsSection segments={segments} value={0} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("range 1 min"), { target: { value: "5" } });
    expect(onChange).toHaveBeenCalledWith([
      { min: 5, max: 30, color: "#111111" },
      { min: 30, max: 70, color: "#222222" },
      { min: 70, max: 100, color: "#333333" },
    ]);
  });

  it("editing a colour emits it", () => {
    const segments: GaugeSegment[] = [
      { min: 0, max: 50, color: "#111111" },
      { min: 50, max: 100, color: "#222222" },
    ];
    const onChange = vi.fn();
    render(<GaugeSegmentsSection segments={segments} value={0} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("range 2 color"), { target: { value: "#00ff00" } });
    expect(onChange).toHaveBeenCalledWith([
      { min: 0, max: 50, color: "#111111" },
      { min: 50, max: 100, color: "#00ff00" },
    ]);
  });

  it("naming a range emits label; clearing it emits undefined, not an empty string", () => {
    const onChangeA = vi.fn();
    const { rerender } = render(
      <GaugeSegmentsSection segments={[{ min: 0, max: 100, color: "#111111" }]} value={0} onChange={onChangeA} />,
    );
    fireEvent.change(screen.getByLabelText("range 1 name"), { target: { value: "Danger" } });
    expect(onChangeA).toHaveBeenCalledWith([{ min: 0, max: 100, color: "#111111", label: "Danger" }]);

    const onChangeB = vi.fn();
    rerender(
      <GaugeSegmentsSection
        segments={[{ min: 0, max: 100, color: "#111111", label: "Danger" }]}
        value={0}
        onChange={onChangeB}
      />,
    );
    fireEvent.change(screen.getByLabelText("range 1 name"), { target: { value: "" } });
    expect(onChangeB).toHaveBeenCalledWith([{ min: 0, max: 100, color: "#111111", label: undefined }]);
  });

  // The contiguity rule this unit exists for: ECharts' colour stops are cumulative upper
  // bounds, so a gap or overlap between neighbouring ranges silently swallows one of them.
  // Editing a max is the one edit that can create that, so it pushes the next range's min
  // to match rather than leaving a gap for a value to fall through unclassified.
  it("editing a range's max keeps the next range's min equal to it", () => {
    const segments: GaugeSegment[] = [
      { min: 0, max: 50, color: "#111111" },
      { min: 50, max: 100, color: "#222222" },
    ];
    const onChange = vi.fn();
    render(<GaugeSegmentsSection segments={segments} value={0} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("range 1 max"), { target: { value: "45" } });
    expect(onChange).toHaveBeenCalledWith([
      { min: 0, max: 45, color: "#111111" },
      { min: 45, max: 100, color: "#222222" },
    ]);
  });

  it("Add range appends one range, continuing from the last one's max", () => {
    const segments: GaugeSegment[] = [
      { min: 0, max: 50, color: "#111111" },
      { min: 50, max: 100, color: "#222222" },
    ];
    const onChange = vi.fn();
    render(<GaugeSegmentsSection segments={segments} value={0} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /add range/i }));
    expect(onChange).toHaveBeenCalledTimes(1);
    const [emitted] = onChange.mock.calls[0] as [GaugeSegment[]];
    expect(emitted).toHaveLength(3);
    expect(emitted[0]).toEqual(segments[0]);
    expect(emitted[1]).toEqual(segments[1]);
    expect(emitted[2].min).toBe(100);
  });

  it("removing goes down to one range and no further", () => {
    render(
      <Stateful
        initial={[
          { min: 0, max: 50, color: "#111111" },
          { min: 50, max: 100, color: "#222222" },
        ]}
        value={0}
      />,
    );
    fireEvent.click(screen.getByLabelText("remove range 2"));
    expect(screen.getAllByLabelText(/^range \d+ min$/)).toHaveLength(1);
    const removeBtn = screen.getByLabelText("remove range 1");
    expect(removeBtn).toBeDisabled();
    fireEvent.click(removeBtn);
    expect(screen.getAllByLabelText(/^range \d+ min$/)).toHaveLength(1);
  });

  // Sorting on every keystroke would make rows jump under the cursor mid-edit — sorting
  // belongs only in the option builder (task 3), never here.
  it("keeps rows in their stored order while a bound is being edited — no reordering under the cursor", async () => {
    const user = userEvent.setup();
    render(
      <Stateful
        initial={[
          { min: 0, max: 20, color: "#aaaaaa", label: "A" },
          { min: 20, max: 50, color: "#bbbbbb", label: "B" },
          { min: 50, max: 100, color: "#cccccc", label: "C" },
        ]}
        value={0}
      />,
    );
    const maxInput = screen.getByLabelText("range 2 max");
    await user.clear(maxInput);
    await user.type(maxInput, "1");
    // Still the second row, holding "B" — a numerically low intermediate value ("1" while
    // typing "15") never causes it to jump ahead of row 1 or behind row 3.
    expect(screen.getByLabelText("range 1 name")).toHaveValue("A");
    expect(screen.getByLabelText("range 2 name")).toHaveValue("B");
    expect(screen.getByLabelText("range 3 name")).toHaveValue("C");
  });
});

describe("GaugeSegmentsSection — the column header", () => {
  // Four controls sit on a range row and only the colour swatch reads as itself. Which box was
  // min, which was max and what the trailing text box was for could only be found by typing into
  // them; Metabase labels its own gauge ranges Label | Min | Max.
  it("names the columns above the rows", () => {
    render(<GaugeSegmentsSection value={100} onChange={() => {}} />);
    for (const name of ["Min", "Max", "Label"]) {
      expect(screen.getByText(name)).toBeInTheDocument();
    }
  });

  it("shows the value on the middle boundary, not a fraction", () => {
    render(<GaugeSegmentsSection value={18760} onChange={() => {}} />);
    expect((screen.getByLabelText("range 2 max") as HTMLInputElement).value).toBe("18760");
    expect((screen.getByLabelText("range 1 max") as HTMLInputElement).value).toBe("9380");
  });
});
