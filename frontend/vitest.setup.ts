import "@testing-library/jest-dom/vitest";
import { beforeEach } from "vitest";

// jsdom lacks ResizeObserver; Radix Switch (via @radix-ui/react-use-size) needs it.
if (typeof globalThis.ResizeObserver === "undefined") {
  globalThis.ResizeObserver = class ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
}

// jsdom lacks matchMedia; ThemeProvider (system theme) and Header read it, so any
// test that renders those — or the whole App — needs it. Defaults to "no match"
// (light theme, no reduced-motion).
if (typeof window !== "undefined" && !window.matchMedia) {
  window.matchMedia = (query: string) =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addListener() {},
      removeListener() {},
      addEventListener() {},
      removeEventListener() {},
      dispatchEvent() {
        return false;
      },
    }) as MediaQueryList;
}

// jsdom has no <canvas> 2D context (no `canvas` npm package installed) — calling
// getContext("2d") logs a noisy "Not implemented" error to the console on every call before
// falling through to `null`. sliceLabel.ts's `measureText` already treats a `null` 2D context
// as "no real rasteriser available" and falls back to an average-character-width
// approximation (see that file), so the noisy log serves no purpose in tests — this stubs
// getContext to return `null` directly and silently for "2d", the same way every other jsdom
// gap in this file is patched.
if (typeof HTMLCanvasElement !== "undefined") {
  const originalGetContext = HTMLCanvasElement.prototype.getContext;
  HTMLCanvasElement.prototype.getContext = function (contextId: string, ...args: unknown[]) {
    if (contextId === "2d") return null;
    return originalGetContext.apply(this, [contextId, ...args] as never);
  } as typeof HTMLCanvasElement.prototype.getContext;
}

// jsdom lacks these; Radix DropdownMenu/Select need them to open via pointer events.
beforeEach(() => {
  // `// @vitest-environment node` files (the eslint-rules tests) have no DOM.
  if (typeof HTMLElement === "undefined") return;
  if (!HTMLElement.prototype.hasPointerCapture) HTMLElement.prototype.hasPointerCapture = () => false;
  if (!HTMLElement.prototype.setPointerCapture) HTMLElement.prototype.setPointerCapture = () => undefined;
  if (!HTMLElement.prototype.releasePointerCapture) HTMLElement.prototype.releasePointerCapture = () => undefined;
  if (!HTMLElement.prototype.scrollIntoView) HTMLElement.prototype.scrollIntoView = () => undefined;
});
