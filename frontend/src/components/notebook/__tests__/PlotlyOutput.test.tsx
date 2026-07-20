import { render, cleanup } from "@testing-library/react";
import { vi, describe, it, expect, afterEach } from "vitest";

const react = vi.fn();
const purge = vi.fn();
const resize = vi.fn();
vi.mock("plotly.js-dist-min", () => ({
  default: { react, purge, Plots: { resize } },
}));

import PlotlyOutput from "../PlotlyOutput";

afterEach(() => {
  cleanup();
  react.mockClear();
  purge.mockClear();
});

describe("PlotlyOutput", () => {
  it("renders the figure via Plotly.react and purges on unmount", async () => {
    const figure = { data: [{ type: "bar", x: ["a"], y: [1] }], layout: { title: "t" } };
    const { unmount } = render(<PlotlyOutput figure={figure} />);
    await vi.waitFor(() => expect(react).toHaveBeenCalled());
    const [, data, layout] = react.mock.calls[0];
    expect(data).toEqual(figure.data);
    expect(layout).toMatchObject({ title: "t" });
    unmount();
    expect(purge).toHaveBeenCalled();
  });
});
