import { describe, it, expect, vi, beforeEach } from "vitest";
import { render } from "@testing-library/react";
import type { EChartsCoreOption } from "echarts/core";

const instance = vi.hoisted(() => ({
  setOption: vi.fn(),
  resize: vi.fn(),
  dispose: vi.fn(),
  isDisposed: vi.fn(() => false),
}));
const init = vi.hoisted(() => vi.fn(() => instance));

vi.mock("echarts/core", () => ({ init, use: vi.fn() }));
vi.mock("../register", () => ({ registerEChartsModules: vi.fn() }));

// ResizeObserver is not in jsdom
beforeEach(() => {
  instance.setOption.mockClear(); instance.resize.mockClear(); instance.dispose.mockClear(); init.mockClear();
  globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
});

import { EChartsRenderer } from "../EChartsRenderer";

const opt = { series: [] } as EChartsCoreOption;

describe("EChartsRenderer", () => {
  it("inits and sets the option on mount", () => {
    render(<EChartsRenderer option={opt} />);
    expect(init).toHaveBeenCalledTimes(1);
    expect(instance.setOption).toHaveBeenCalledWith(opt, true);
  });
  it("disposes on unmount", () => {
    const { unmount } = render(<EChartsRenderer option={opt} />);
    unmount();
    expect(instance.dispose).toHaveBeenCalledTimes(1);
  });
});
