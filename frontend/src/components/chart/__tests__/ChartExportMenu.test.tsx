import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useRef } from "react";

const downloadCsv = vi.fn();
const downloadJson = vi.fn();
const downloadPng = vi.fn((..._a: unknown[]) => Promise.resolve());
const downloadXlsx = vi.fn((..._a: unknown[]) => Promise.resolve());
vi.mock("../export-chart", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../export-chart")>()),
  downloadCsv: (...a: unknown[]) => downloadCsv(...a),
  downloadJson: (...a: unknown[]) => downloadJson(...a),
  downloadPng: (...a: unknown[]) => downloadPng(...a),
}));
vi.mock("../export-xlsx", () => ({ downloadXlsx: (...a: unknown[]) => downloadXlsx(...a) }));

import { ChartExportMenu } from "../ChartExportMenu";
import { MAX_CHART_ROWS } from "@/api/charts";

/** The note's text, derived from the one constant the chart query is capped by (KI-049, KI-072). */
const LIMIT_NOTE = `The maximum download size is ${MAX_CHART_ROWS.toLocaleString()} rows.`;

const columns = [{ name: "a", type: "x" }];
const rows = [["1"]];

function Harness({ withSvg, withIcon, disabled, columnSettings }: { withSvg: boolean; withIcon?: boolean; disabled?: boolean; columnSettings?: Record<string, { column_title?: string }> }) {
  const ref = useRef<HTMLDivElement>(null);
  return (
    <div>
      <div ref={ref} data-testid="container">
        {/* An icon <svg> first, as the Object detail's pager arrows are: it must never be what
            `.png` finds. */}
        {withIcon && <svg data-testid="icon" className="lucide" />}
        {withSvg ? <div data-chart-image=""><svg data-testid="thesvg" /></div> : <span>table</span>}
      </div>
      <ChartExportMenu title="My Chart" columns={columns} rows={rows} containerRef={ref} disabled={disabled} columnSettings={columnSettings} />
    </div>
  );
}

async function open(withSvg: boolean, columnSettings?: Record<string, { column_title?: string }>) {
  const user = userEvent.setup();
  render(<Harness withSvg={withSvg} columnSettings={columnSettings} />);
  await user.click(screen.getByRole("button", { name: /export chart/i }));
  await screen.findByText("Download data");
  return user;
}

describe("ChartExportMenu", () => {
  beforeEach(() => {
    downloadCsv.mockClear(); downloadJson.mockClear(); downloadPng.mockClear(); downloadXlsx.mockClear();
  });

  it("offers Metabase's three table formats, in its order", async () => {
    await open(false);
    // Asserted by ROLE, not by text: these are radios in a radiogroup, which is what makes the
    // control a segmented control rather than three buttons that happen to look like one.
    const radios = screen.getAllByRole("radio");
    expect(radios.map((r) => r.textContent)).toEqual([".csv", ".xlsx", ".json"]);
  });

  it("adds .png only when the container rendered an <svg>", async () => {
    await open(true);
    expect(screen.getAllByRole("radio").map((r) => r.textContent)).toEqual([".csv", ".xlsx", ".json", ".png"]);
  });

  // KI-043: Object detail renders no chart, only pager-arrow icons, and `.png` used to export one.
  it("does not offer .png when the only <svg> in the card is an icon", async () => {
    const user = userEvent.setup();
    render(<Harness withSvg={false} withIcon />);
    await user.click(screen.getByRole("button", { name: /export chart/i }));
    await screen.findByText("Download data");
    expect(screen.getAllByRole("radio").map((r) => r.textContent)).toEqual([".csv", ".xlsx", ".json"]);
  });

  it("carries Metabase's formatting checkbox, default on, and the chart's real row limit", async () => {
    // KI-049/KI-072: the file is built from the rows the card loaded, which a chart query caps at
    // MAX_CHART_ROWS — the popover used to promise Metabase's 1 million.
    await open(false);
    expect(screen.getByLabelText("keep the data formatted")).toBeChecked();
    expect(screen.getByText(LIMIT_NOTE)).toBeInTheDocument();
    expect(LIMIT_NOTE).toBe("The maximum download size is 10,000 rows.");
  });

  it("hides the formatting checkbox and the row-limit note for .png", async () => {
    // Metabase's canConfigureFormatting: an image has no cells to format.
    const user = await open(true);
    await user.click(screen.getByRole("radio", { name: ".png" }));
    expect(screen.queryByLabelText("keep the data formatted")).toBeNull();
    expect(screen.queryByText(LIMIT_NOTE)).toBeNull();
  });

  it("downloads CSV with the formatting flag on by default", async () => {
    const user = await open(false, { a: { column_title: "Column A" } });
    await user.click(screen.getByRole("button", { name: "Download" }));
    expect(downloadCsv).toHaveBeenCalledWith("My Chart", columns, rows, { a: { column_title: "Column A" } }, true);
  });

  it("passes the formatting flag off once the checkbox is cleared", async () => {
    const user = await open(false);
    await user.click(screen.getByLabelText("keep the data formatted"));
    await user.click(screen.getByRole("button", { name: "Download" }));
    expect(downloadCsv).toHaveBeenCalledWith("My Chart", columns, rows, undefined, false);
  });

  it("downloads .xlsx through the lazy writer when that format is picked", async () => {
    const user = await open(false);
    await user.click(screen.getByRole("radio", { name: ".xlsx" }));
    await user.click(screen.getByRole("button", { name: "Download" }));
    expect(downloadXlsx).toHaveBeenCalledWith("My Chart", columns, rows, undefined, true);
    expect(downloadCsv).not.toHaveBeenCalled();
  });

  it("downloads .json", async () => {
    const user = await open(false);
    await user.click(screen.getByRole("radio", { name: ".json" }));
    await user.click(screen.getByRole("button", { name: "Download" }));
    expect(downloadJson).toHaveBeenCalledWith("My Chart", columns, rows, undefined, true);
  });

  it("downloads PNG from the chart image, not from an icon before it", async () => {
    const user = userEvent.setup();
    render(<Harness withSvg withIcon />);
    await user.click(screen.getByRole("button", { name: /export chart/i }));
    await screen.findByText("Download data");
    await user.click(screen.getByRole("radio", { name: ".png" }));
    await user.click(screen.getByRole("button", { name: "Download" }));
    expect(downloadPng).toHaveBeenCalledWith("My Chart", screen.getByTestId("thesvg"));
  });

  it("falls back to .csv when a card that had an svg no longer does", async () => {
    // hasSvg is read on open. A card that re-renders as a table between two opens would otherwise
    // leave .png selected and the Download button would silently do nothing.
    const user = userEvent.setup();
    const { rerender } = render(<Harness withSvg />);
    await user.click(screen.getByRole("button", { name: /export chart/i }));
    await user.click(await screen.findByRole("radio", { name: ".png" }));
    await user.keyboard("{Escape}");
    rerender(<Harness withSvg={false} />);
    await user.click(screen.getByRole("button", { name: /export chart/i }));
    await screen.findByText("Download data");
    expect(screen.getByRole("radio", { name: ".csv" })).toBeChecked();
  });

  it("disabled prop disables the trigger", () => {
    render(<Harness withSvg disabled />);
    expect((screen.getByRole("button", { name: /export chart/i }) as HTMLButtonElement).disabled).toBe(true);
  });
});
