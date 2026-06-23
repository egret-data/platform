import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useRef } from "react";

const downloadCsv = vi.fn();
const downloadPng = vi.fn((..._a: unknown[]) => Promise.resolve());
vi.mock("../export-chart", () => ({
  downloadCsv: (...a: unknown[]) => downloadCsv(...a),
  downloadPng: (...a: unknown[]) => downloadPng(...a),
}));

import { ChartExportMenu } from "../ChartExportMenu";

function Harness({ withSvg, disabled }: { withSvg: boolean; disabled?: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  return (
    <div>
      <div ref={ref} data-testid="container">
        {withSvg ? <svg data-testid="thesvg" /> : <span>table</span>}
      </div>
      <ChartExportMenu title="My Chart" columns={[{ name: "a", type: "x" }]} rows={[["1"]]} containerRef={ref} disabled={disabled} />
    </div>
  );
}

describe("ChartExportMenu", () => {
  beforeEach(() => { downloadCsv.mockClear(); downloadPng.mockClear(); });

  it("shows CSV + PNG when the container has an <svg>", async () => {
    const user = userEvent.setup();
    render(<Harness withSvg />);
    await user.click(screen.getByRole("button", { name: /export chart/i }));
    expect(await screen.findByText("Download CSV")).toBeTruthy();
    expect(screen.getByText("Download PNG")).toBeTruthy();
  });
  it("shows only CSV when there is no <svg> (table/pivot)", async () => {
    const user = userEvent.setup();
    render(<Harness withSvg={false} />);
    await user.click(screen.getByRole("button", { name: /export chart/i }));
    expect(await screen.findByText("Download CSV")).toBeTruthy();
    expect(screen.queryByText("Download PNG")).toBeNull();
  });
  it("Download CSV calls downloadCsv with the title/columns/rows", async () => {
    const user = userEvent.setup();
    render(<Harness withSvg />);
    await user.click(screen.getByRole("button", { name: /export chart/i }));
    await user.click(await screen.findByText("Download CSV"));
    expect(downloadCsv).toHaveBeenCalledWith("My Chart", [{ name: "a", type: "x" }], [["1"]]);
  });
  it("Download PNG calls downloadPng with the title and the svg element", async () => {
    const user = userEvent.setup();
    render(<Harness withSvg />);
    await user.click(screen.getByRole("button", { name: /export chart/i }));
    await user.click(await screen.findByText("Download PNG"));
    expect(downloadPng).toHaveBeenCalledWith("My Chart", expect.anything());
  });
  it("disabled prop disables the trigger", () => {
    render(<Harness withSvg disabled />);
    expect((screen.getByRole("button", { name: /export chart/i }) as HTMLButtonElement).disabled).toBe(true);
  });
});
