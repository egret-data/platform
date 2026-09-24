import { renderHook, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import type { ReactNode } from "react";

import { useEngineConfigForm } from "../useEngineConfigForm";

// A minimal engine whose spec mirrors what the 5s status poll returns. Each call
// builds FRESH object/array references with identical content — exactly what the
// polled `getEngine` query produces every tick.
function makeEngine(serviceAccount?: string) {
  return {
    id: "e1",
    name: "eng",
    type: "trino",
    namespace: "ns",
    status: "running",
    spec: {
      // A real engine's stored trinoConfig always carries coordinator/worker
      // (it was created through the same omit-empty build), so an unedited load
      // round-trips to an empty diff.
      trinoConfig: {
        coordinator: {},
        worker: {},
        ...(serviceAccount ? { serviceAccount } : {}),
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

    act(() => result.current.setServiceAccount("sa-x"));
    expect(result.current.dirty).toBe(true);

    act(() => result.current.setServiceAccount(""));
    expect(result.current.dirty).toBe(false);
  });

  it("re-prefills (still clean) when the spec content actually changes", () => {
    const { result, rerender } = renderHook(
      ({ e }) => useEngineConfigForm(e),
      { initialProps: { e: makeEngine("sa-a") }, wrapper },
    );
    rerender({ e: makeEngine("sa-b") });
    expect(result.current.dirty).toBe(false);
    expect(result.current.serviceAccount).toBe("sa-b");
  });
});

describe("useEngineConfigForm editedSinceLoad (beforeunload guard)", () => {
  // A provisioning / connect-error engine can arrive with an absent (or sparse)
  // stored trinoConfig. The built payload then carries defaults the stored spec
  // lacks, so `dirty` (built-vs-stored) reads true on load — but the user hasn't
  // touched anything, so the browser unsaved-changes guard must stay disarmed.
  function makeSparseEngine() {
    return {
      id: "e1",
      name: "eng",
      type: "trino",
      namespace: "ns",
      status: "provisioning",
      spec: { trinoConfig: undefined },
    } as unknown as Parameters<typeof useEngineConfigForm>[0];
  }

  it("is false on load even when `dirty` is true for an unedited sparse spec", () => {
    const { result } = renderHook(({ e }) => useEngineConfigForm(e), {
      initialProps: { e: makeSparseEngine() },
      wrapper,
    });
    // The whole point of the guard: `dirty` may be noisy on load, but the user
    // has made no edit, so we must NOT prompt on navigate/close.
    expect(result.current.dirty).toBe(true);
    expect(result.current.editedSinceLoad).toBe(false);
  });

  it("arms on a real edit and disarms again on revert", () => {
    const { result } = renderHook(({ e }) => useEngineConfigForm(e), {
      initialProps: { e: makeSparseEngine() },
      wrapper,
    });
    expect(result.current.editedSinceLoad).toBe(false);

    act(() => result.current.setServiceAccount("sa-x"));
    expect(result.current.editedSinceLoad).toBe(true);

    act(() => result.current.setServiceAccount(""));
    expect(result.current.editedSinceLoad).toBe(false);
  });

  it("an identical 5s status poll keeps it false", () => {
    const { result, rerender } = renderHook(
      ({ e }) => useEngineConfigForm(e),
      { initialProps: { e: makeSparseEngine() }, wrapper },
    );
    for (let i = 0; i < 3; i++) rerender({ e: makeSparseEngine() });
    expect(result.current.editedSinceLoad).toBe(false);
  });
});

describe("useEngineConfigForm delegation mode", () => {
  function withDelegation(mode?: "impersonate" | "jwt") {
    const e = makeEngine();
    (
      e.spec as unknown as { trinoConfig: Record<string, unknown> }
    ).trinoConfig.delegationMode = mode;
    return e;
  }

  it("defaults to Auto and omits the key from the built payload", () => {
    const { result } = renderHook(({ e }) => useEngineConfigForm(e), {
      initialProps: { e: withDelegation(undefined) },
      wrapper,
    });
    // An engine with no stored delegationMode seeds "" (Auto); the backend then
    // resolves the default (jwt→impersonate). Auto must OMIT the key so the
    // backend default applies rather than pinning an explicit mode.
    expect(result.current.delegationMode).toBe("");
    expect(result.current.builtTrinoConfig.delegationMode).toBeUndefined();
  });

  it("loads a jwt engine and PRESERVES the mode on round-trip (the save-drop fix)", () => {
    const { result } = renderHook(({ e }) => useEngineConfigForm(e), {
      initialProps: { e: withDelegation("jwt") },
      wrapper,
    });
    expect(result.current.delegationMode).toBe("jwt");
    // The built payload keeps delegationMode:"jwt" — without this the UI save
    // would silently strip it and revert the engine to impersonate.
    expect(result.current.builtTrinoConfig.delegationMode).toBe("jwt");
    expect(result.current.dirty).toBe(false);
  });

  it("switching Auto → jwt marks dirty and emits the key", () => {
    const { result } = renderHook(({ e }) => useEngineConfigForm(e), {
      initialProps: { e: withDelegation(undefined) },
      wrapper,
    });
    act(() => result.current.setDelegationMode("jwt"));
    expect(result.current.dirty).toBe(true);
    expect(result.current.builtTrinoConfig.delegationMode).toBe("jwt");
  });
});
