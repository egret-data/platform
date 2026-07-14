import { renderHook, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import type { ReactNode } from "react";

import { useEngineConfigForm } from "../useEngineConfigForm";

// A minimal engine whose spec mirrors what the 5s status poll returns. Each call
// builds FRESH object/array references with identical content — exactly what the
// polled `getEngine` query produces every tick.
function makeEngine(logLevel = "INFO") {
  return {
    id: "e1",
    name: "eng",
    type: "trino",
    namespace: "ns",
    warehouseId: "w1",
    status: "running",
    spec: {
      // A real engine's stored trinoConfig always carries coordinator/worker
      // (it was created through the same omit-empty build), so an unedited load
      // round-trips to an empty diff.
      trinoConfig: {
        coordinator: {},
        worker: {},
        logLevel,
        configProperties: { "query.max-memory": "1GB" },
        env: { FOO: "bar" },
      },
    },
  } as unknown as Parameters<typeof useEngineConfigForm>[0];
}

function wrapper({ children }: { children: ReactNode }) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

describe("useEngineConfigForm dirty tracking", () => {
  it("an identical 5s status poll does NOT mark the form dirty", () => {
    const { result, rerender } = renderHook(
      ({ e }) => useEngineConfigForm(e),
      { initialProps: { e: makeEngine() }, wrapper },
    );
    expect(result.current.dirty).toBe(false);

    // Simulate several polls: new object references, identical content.
    for (let i = 0; i < 3; i++) {
      rerender({ e: makeEngine() });
    }
    expect(result.current.dirty).toBe(false);
  });

  it("becomes dirty on a real user edit, and clean again when reverted", () => {
    const { result } = renderHook(({ e }) => useEngineConfigForm(e), {
      initialProps: { e: makeEngine() },
      wrapper,
    });
    expect(result.current.dirty).toBe(false);

    act(() => result.current.setLogLevel("DEBUG"));
    expect(result.current.dirty).toBe(true);

    act(() => result.current.setLogLevel("INFO"));
    expect(result.current.dirty).toBe(false);
  });

  it("re-prefills (still clean) when the spec content actually changes", () => {
    const { result, rerender } = renderHook(
      ({ e }) => useEngineConfigForm(e),
      { initialProps: { e: makeEngine("INFO") }, wrapper },
    );
    rerender({ e: makeEngine("DEBUG") });
    expect(result.current.dirty).toBe(false);
    expect(result.current.logLevel).toBe("DEBUG");
  });
});
