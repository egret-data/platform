/**
 * What xyflow needs from jsdom to measure, fit and draw nodes (as xyflow's testing guide stubs
 * it). Call from a test file's `beforeAll`; it changes globals for that file only.
 *
 * - jsdom lays nothing out: every element is given a size, so xyflow can fit the graph.
 * - xyflow measures nodes when a ResizeObserver reports them, and fits only measured nodes; the
 *   setup file's observer never reports, so this one reports every element once, as a browser
 *   does. Batched per observer and microtask, as a browser delivers them: one entry per callback
 *   would make xyflow update its store once per node (5,000 renders of 5,000 nodes for a large graph).
 * - jsdom has no `DOMMatrixReadOnly`; xyflow reads the zoom from the viewport's transform with it
 *   when a node mounts after the first render.
 */
const size = { width: 800, height: 600 };

/** The size every element reports from now on (800 × 600 unless set): a narrow canvas, for one test. */
export function setXyflowSize(width: number, height: number): void {
  size.width = width;
  size.height = height;
}

export function installXyflowJsdom(): void {
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => size.width });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => size.height });
  globalThis.ResizeObserver = class {
    private pending: Element[] = [];
    constructor(private readonly cb: ResizeObserverCallback) {}
    observe(target: Element) {
      if (this.pending.push(target) > 1) return;
      queueMicrotask(() => {
        const entries = this.pending.map((t) => ({ target: t, contentRect: { width: size.width, height: size.height }, borderBoxSize: [{ inlineSize: size.width, blockSize: size.height }] }));
        this.pending = [];
        this.cb(entries as unknown as ResizeObserverEntry[], this as unknown as ResizeObserver);
      });
    }
    unobserve() {}
    disconnect() {}
  };
  if (typeof window.DOMMatrixReadOnly === "function") return;
  class DOMMatrixReadOnly {
    m22: number;
    constructor(transform?: string) {
      const scale = transform ? /scale\(([\d.]+)\)/.exec(transform)?.[1] : undefined;
      this.m22 = scale ? Number(scale) : 1;
    }
  }
  Object.defineProperty(window, "DOMMatrixReadOnly", { value: DOMMatrixReadOnly, configurable: true, writable: true });
}
