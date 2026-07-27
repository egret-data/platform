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
});
