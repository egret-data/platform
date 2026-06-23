import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { useRef } from "react";

const downloadCsv = vi.fn();
const downloadPng = vi.fn();
vi.mock("../export-chart", () => ({ downloadCsv: (...a: unknown[]) => downloadCsv(...a), downloadPng: (...a: unknown[]) => downloadPng(...a) }));

import { ChartExportMenu } from "../ChartExportMenu";

function Harness({ withSvg, disabled }: { withSvg: boolean; disabled?: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  return (
    <div>
      <div ref={ref} data-testid="container">{withSvg ? <svg data-testid="thesvg" /> : <span>table</span>}</div>
      <ChartExportMenu title="My Chart" columns={[{ name: "a", type: "x" }]} rows={[["1"]]} containerRef={ref} disabled={disabled} />
    </div>
  );
}

describe("ChartExportMenu", () => {
  beforeEach(() => { downloadCsv.mockClear(); downloadPng.mockClear(); });

  it("shows CSV + PNG when the container has an <svg>", async () => {
    render(<Harness withSvg />);
    fireEvent.click(screen.getByRole("button", { name: "export chart" }));
    expect(await screen.findByText("Download CSV")).toBeTruthy();
    expect(screen.getByText("Download PNG")).toBeTruthy();
  });
  it("shows only CSV when there is no <svg> (table/pivot)", async () => {
    render(<Harness withSvg={false} />);
    fireEvent.click(screen.getByRole("button", { name: "export chart" }));
    expect(await screen.findByText("Download CSV")).toBeTruthy();
    expect(screen.queryByText("Download PNG")).toBeNull();
  });
  it("Download CSV calls downloadCsv with the title/columns/rows", async () => {
    render(<Harness withSvg />);
    fireEvent.click(screen.getByRole("button", { name: "export chart" }));
    fireEvent.click(await screen.findByText("Download CSV"));
    expect(downloadCsv).toHaveBeenCalledWith("My Chart", [{ name: "a", type: "x" }], [["1"]]);
  });
  it("disabled prop disables the trigger", () => {
    render(<Harness withSvg disabled />);
    expect((screen.getByRole("button", { name: "export chart" }) as HTMLButtonElement).disabled).toBe(true);
  });
});
