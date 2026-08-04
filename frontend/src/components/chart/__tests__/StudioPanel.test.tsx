import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StudioPanel, hasAxesControls } from "../StudioPanel";
import { tableDescriptor } from "../viz/table";
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
  temporalUnits: {},
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

  it("stays silent instead of saying 'no options' when the caller has more content below", () => {
    // Every real Display-tab caller (ChartExplore, CardVizPanel) always renders
    // SharedDisplaySections right after this panel, so a bare descriptor must not print
    // a sentence that a Formatting/Columns section directly contradicts once data arrives.
    // No viz in the app is genuinely empty on the Display tab today — both callers always
    // compose SharedDisplaySections after it — so this synthetic bare descriptor is the
    // only way to exercise the branch; the case above pins that the underlying "say so"
    // behavior still exists for a caller that has nothing else to show.
    const bare = { ...desc, controls: [] } as unknown as VizDescriptor<{ yMetrics: string[] }>;
    render(<StudioPanel {...props} descriptor={bare} tab="display" hasMoreBelow />);
    expect(screen.queryByText(/no display options/i)).not.toBeInTheDocument();
  });

  it("a table's Display tab never claims there are no display options", () => {
    // Regression pin for the live-check finding: table declares zero display controls by
    // design (its Display tab is entirely shell-owned Columns/Formatting), but the shell
    // always renders those sections right after — so with hasMoreBelow set, the panel
    // must render nothing here rather than a sentence the sections below contradict.
    render(
      <StudioPanel
        descriptor={tableDescriptor()}
        config={{ columns: [], metrics: [] }}
        onConfigChange={vi.fn()}
        columns={[]} metrics={[]} resultColumns={[]} rows={[]}
        temporalUnits={{}}
        unitRow={() => null}
        tab="display"
        hasMoreBelow
      />,
    );
    expect(screen.queryByText(/no display options/i)).not.toBeInTheDocument();
  });

  it("renders controls under their group heading, in first-appearance order", () => {
    const grouped = {
      ...desc,
      controls: [
        { id: "a", section: "display", group: "Series", render: () => <div>A</div> },
        { id: "b", section: "display", group: "Format", render: () => <div>B</div> },
        { id: "c", section: "display", group: "Series", render: () => <div>C</div> },
      ],
    } as unknown as VizDescriptor<{ yMetrics: string[] }>;
    render(<StudioPanel {...props} descriptor={grouped} tab="display" />);
    const headings = screen.getAllByRole("heading").map((h) => h.textContent);
    expect(headings).toEqual(["Series", "Format"]);
  });

  it("puts advanced controls behind a disclosure that starts closed", async () => {
    const withAdvanced = {
      ...desc,
      controls: [
        { id: "a", section: "display", group: "Format", render: () => <div>Always</div> },
        { id: "b", section: "display", group: "Format", advanced: true, render: () => <div>Rare</div> },
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
        { id: "a", section: "display", group: "Format", render: () => <div>Always</div> },
        { id: "b", section: "display", group: "Format", advanced: true, render: () => <div>RareA</div> },
      ],
    } as unknown as VizDescriptor<{ yMetrics: string[] }>;
    const descB = {
      ...desc,
      key: "line",
      controls: [
        { id: "a", section: "display", group: "Format", render: () => <div>Always</div> },
        { id: "b", section: "display", group: "Format", advanced: true, render: () => <div>RareB</div> },
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

describe("StudioPanel — the Axes tab", () => {
  const desc = {
    key: "bar",
    controls: [
      { id: "s", section: "display", group: "Series", render: () => <div>SeriesControl</div> },
      { id: "x", section: "display", group: "Axes", render: () => <div>XScale</div> },
      { id: "y", section: "display", group: "Axes", advanced: true, render: () => <div>YMin</div> },
    ],
  } as never;

  // `group: "Axes"` is declared once by the viz; the shell decides whether it is a heading or a
  // tab. Leaving it in both places would have printed every axis control twice.
  it("moves the Axes group out of the Display tab", () => {
    render(<StudioPanel {...props} descriptor={desc} tab="display" />);
    expect(screen.getByText("SeriesControl")).toBeInTheDocument();
    expect(screen.queryByText("XScale")).toBeNull();
    expect(screen.queryByText("Axes")).toBeNull();
  });

  // Flat: the group heading would only repeat the tab's own name, and an "advanced" control has
  // no group left to hide inside, so it must not silently disappear.
  it("renders the Axes group flat in its own tab, advanced controls included", () => {
    render(<StudioPanel {...props} descriptor={desc} tab="axes" />);
    expect(screen.getByText("XScale")).toBeInTheDocument();
    expect(screen.getByText("YMin")).toBeInTheDocument();
    expect(screen.queryByText("SeriesControl")).toBeNull();
    expect(screen.queryByRole("button", { name: /more options/i })).toBeNull();
  });

  it("reports which vizzes have an Axes tab at all", () => {
    expect(hasAxesControls(desc)).toBe(true);
    expect(hasAxesControls({ key: "pie", controls: [{ id: "d", section: "display", render: () => null }] } as never)).toBe(false);
    expect(hasAxesControls(undefined)).toBe(false);
  });
});
