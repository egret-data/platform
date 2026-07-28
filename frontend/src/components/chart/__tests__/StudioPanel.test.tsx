import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StudioPanel } from "../StudioPanel";
import type { VizDescriptor } from "../viz/types";

const desc = {
  key: "bar",
  meta: { key: "bar", name: "Bar", icon: () => null, description: "" },
  prefix: "cartesian.",
  defaultConfig: { yMetrics: [] },
  fromParams: () => ({ yMetrics: [] }),
  toSettings: () => ({}),
  buildSpec: () => ({}),
  requiredFields: () => [],
  controls: [
    { id: "x", section: "data", render: () => <div>X-axis control</div> },
    {
      id: "d",
      section: "display",
      render: ({ onChange }: { onChange: (p: Record<string, unknown>) => void }) => (
        <button onClick={() => onChange({ stacked: true })}>Stack</button>
      ),
    },
  ],
} as unknown as VizDescriptor<{ yMetrics: string[] }>;

const props = {
  descriptor: desc,
  config: { yMetrics: [] },
  onConfigChange: vi.fn(),
  columns: [], metrics: [], resultColumns: [], rows: [],
  unitRow: () => null,
  tab: "data" as const,
};

describe("StudioPanel", () => {
  it("renders the data-section controls on the Data tab", () => {
    render(<StudioPanel {...props} />);
    expect(screen.getByText("X-axis control")).toBeInTheDocument();
    expect(screen.queryByText("Stack")).not.toBeInTheDocument();
  });

  it("renders the display-section controls on the Display tab", () => {
    render(<StudioPanel {...props} tab="display" />);
    expect(screen.getByText("Stack")).toBeInTheDocument();
    expect(screen.queryByText("X-axis control")).not.toBeInTheDocument();
  });

  it("passes a control's patch up to onConfigChange", async () => {
    const onConfigChange = vi.fn();
    render(<StudioPanel {...props} tab="display" onConfigChange={onConfigChange} />);
    await userEvent.click(screen.getByRole("button", { name: "Stack" }));
    expect(onConfigChange).toHaveBeenCalledWith({ stacked: true });
  });

  it("says so when a viz has no controls for the tab", () => {
    const bare = { ...desc, controls: [] } as unknown as VizDescriptor<{ yMetrics: string[] }>;
    render(<StudioPanel {...props} descriptor={bare} tab="display" />);
    expect(screen.getByText(/no display options/i)).toBeInTheDocument();
  });

  it("renders controls under their group heading, in first-appearance order", () => {
    const grouped = {
      ...desc,
      controls: [
        { id: "a", section: "display", group: "Series", render: () => <div>A</div> },
        { id: "b", section: "display", group: "Axes", render: () => <div>B</div> },
        { id: "c", section: "display", group: "Series", render: () => <div>C</div> },
      ],
    } as unknown as VizDescriptor<{ yMetrics: string[] }>;
    render(<StudioPanel {...props} descriptor={grouped} tab="display" />);
    const headings = screen.getAllByRole("heading").map((h) => h.textContent);
    expect(headings).toEqual(["Series", "Axes"]);
  });

  it("puts advanced controls behind a disclosure that starts closed", async () => {
    const withAdvanced = {
      ...desc,
      controls: [
        { id: "a", section: "display", group: "Axes", render: () => <div>Always</div> },
        { id: "b", section: "display", group: "Axes", advanced: true, render: () => <div>Rare</div> },
      ],
    } as unknown as VizDescriptor<{ yMetrics: string[] }>;
    render(<StudioPanel {...props} descriptor={withAdvanced} tab="display" />);
    expect(screen.getByText("Always")).toBeInTheDocument();
    expect(screen.queryByText("Rare")).not.toBeInTheDocument();
    const trigger = screen.getByRole("button", { name: /more options/i });
    expect(trigger).toHaveAttribute("aria-expanded", "false");
    await userEvent.click(trigger);
    expect(screen.getByText("Rare")).toBeInTheDocument();
    expect(trigger).toHaveAttribute("aria-expanded", "true");
  });

  it("resets a group's disclosure when the active viz changes", async () => {
    const descA = {
      ...desc,
      key: "bar",
      controls: [
        { id: "a", section: "display", group: "Axes", render: () => <div>Always</div> },
        { id: "b", section: "display", group: "Axes", advanced: true, render: () => <div>RareA</div> },
      ],
    } as unknown as VizDescriptor<{ yMetrics: string[] }>;
    const descB = {
      ...desc,
      key: "line",
      controls: [
        { id: "a", section: "display", group: "Axes", render: () => <div>Always</div> },
        { id: "b", section: "display", group: "Axes", advanced: true, render: () => <div>RareB</div> },
      ],
    } as unknown as VizDescriptor<{ yMetrics: string[] }>;
    const { rerender } = render(<StudioPanel {...props} descriptor={descA} tab="display" />);
    await userEvent.click(screen.getByRole("button", { name: /more options/i }));
    expect(screen.getByText("RareA")).toBeInTheDocument();
    rerender(<StudioPanel {...props} descriptor={descB} tab="display" />);
    expect(screen.queryByText("RareB")).not.toBeInTheDocument();
  });

  it("still renders ungrouped controls, with no heading", () => {
    render(<StudioPanel {...props} tab="data" />);
    expect(screen.getByText("X-axis control")).toBeInTheDocument();
    expect(screen.queryByRole("heading")).not.toBeInTheDocument();
  });
});
