// plotly.js-dist-min ships no TypeScript types. Minimal declaration covering
// just the surface PlotlyOutput uses (react/purge/Plots.resize).
declare module "plotly.js-dist-min" {
  interface PlotlyStatic {
    react(
      el: HTMLElement,
      data: unknown[],
      layout?: Record<string, unknown>,
      config?: Record<string, unknown>,
    ): Promise<HTMLElement>;
    purge(el: HTMLElement): void;
    Plots: { resize(el: HTMLElement): void };
  }
  const Plotly: PlotlyStatic;
  export default Plotly;
}
