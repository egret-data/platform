import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SeriesLineStyleSection } from "../SeriesLineStyleSection";

describe("SeriesLineStyleSection", () => {
  it("renders a style and size control per series name", () => {
    render(<SeriesLineStyleSection names={["a", "b"]} onChange={() => {}} />);
    expect(screen.getByLabelText("style a")).toBeInTheDocument();
    expect(screen.getByLabelText("size a")).toBeInTheDocument();
    expect(screen.getByLabelText("style b")).toBeInTheDocument();
    expect(screen.getByLabelText("size b")).toBeInTheDocument();
  });

  it("choosing a style emits it under that series' key", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<SeriesLineStyleSection names={["a"]} onChange={onChange} />);
    await user.click(screen.getByLabelText("style a"));
    await user.click(await screen.findByRole("option", { name: "Dashed" }));
    expect(onChange).toHaveBeenCalledWith({ seriesLine: { a: { style: "dashed" } } });
  });

  it("choosing a size emits it under that series' key", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<SeriesLineStyleSection names={["a"]} onChange={onChange} />);
    await user.click(screen.getByLabelText("size a"));
    await user.click(await screen.findByRole("option", { name: "L" }));
    expect(onChange).toHaveBeenCalledWith({ seriesLine: { a: { size: "L" } } });
  });

  // Pins the bug the shape invites: seriesLine is a map, so setting one series' field must
  // MERGE, not replace — get the outer spread wrong and it wipes every other series; get the
  // inner spread wrong and it wipes this series' other field.
  it("setting one series' style preserves the other series' entry and this series' size", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <SeriesLineStyleSection
        names={["a", "b"]}
        seriesLine={{ a: { style: "dashed" }, b: { size: "L" } }}
        onChange={onChange}
      />,
    );
    await user.click(screen.getByLabelText("style b"));
    await user.click(await screen.findByRole("option", { name: "Dotted" }));
    expect(onChange).toHaveBeenCalledWith({
      seriesLine: { a: { style: "dashed" }, b: { size: "L", style: "dotted" } },
    });
  });

  it("choosing Auto for style clears it back to undefined", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<SeriesLineStyleSection names={["a"]} seriesLine={{ a: { style: "dashed" } }} onChange={onChange} />);
    await user.click(screen.getByLabelText("style a"));
    await user.click(await screen.findByRole("option", { name: "Auto" }));
    expect(onChange).toHaveBeenCalledWith({ seriesLine: { a: { style: undefined } } });
  });
});
