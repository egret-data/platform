import { describe, it, expect, beforeEach } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { useCollapsed } from "../folders";

beforeEach(() => localStorage.clear());

describe("useCollapsed.expand", () => {
  it("un-collapses a collapsed key and persists that", () => {
    localStorage.setItem("k", JSON.stringify(["f1", "f2"]));
    const { result } = renderHook(() => useCollapsed("k"));
    expect(result.current.isCollapsed("f1")).toBe(true);
    act(() => result.current.expand("f1"));
    expect(result.current.isCollapsed("f1")).toBe(false);
    expect(result.current.isCollapsed("f2")).toBe(true);
    // Persisted, not just in state: the deep link is followed once, and the section must still be
    // open after the reload that a "Saved in" click is often followed by.
    expect(JSON.parse(localStorage.getItem("k")!)).toEqual(["f2"]);
  });

  it("is a genuine no-op on a key that is already expanded, keeping state identity", () => {
    // Asserting on localStorage here would assert NOTHING: rewriting ["f2"] over ["f2"] is
    // byte-identical, so a version without the early return passes that check. The property that
    // actually matters is state IDENTITY — Dashboards.tsx runs `expand(focusFolder)` from an
    // effect that depends on the hook's callbacks, so an expand that minted a fresh Set on every
    // call would change `isCollapsed`'s identity, re-fire the effect, and loop forever.
    localStorage.setItem("k", JSON.stringify(["f2"]));
    const { result } = renderHook(() => useCollapsed("k"));
    const before = result.current.isCollapsed;
    act(() => result.current.expand("f1"));
    expect(result.current.isCollapsed("f1")).toBe(false);
    expect(JSON.parse(localStorage.getItem("k")!)).toEqual(["f2"]);
    expect(result.current.isCollapsed).toBe(before);
  });

  it("does change state identity when the key really was collapsed", () => {
    // The other half of the pair: without it, `expand` could satisfy the identity assertion above
    // by never updating state at all.
    localStorage.setItem("k", JSON.stringify(["f1"]));
    const { result } = renderHook(() => useCollapsed("k"));
    const before = result.current.isCollapsed;
    act(() => result.current.expand("f1"));
    expect(result.current.isCollapsed).not.toBe(before);
  });
});
