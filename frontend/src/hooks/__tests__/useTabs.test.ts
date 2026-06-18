import { renderHook, act } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";
import { useTabs } from "@/hooks/useTabs";

afterEach(() => localStorage.clear());

test("seeds one tab, adds, renames, switches, closes, persists", () => {
  const { result, rerender } = renderHook(() => useTabs("eng"));
  expect(result.current.tabs).toHaveLength(1);
  const first = result.current.tabs[0].id;
  act(() => result.current.addTab());
  expect(result.current.tabs).toHaveLength(2);
  const second = result.current.activeId;
  act(() => result.current.renameTab(second!, "My Tab"));
  expect(result.current.tabs.find((t) => t.id === second)!.title).toBe("My Tab");
  act(() => result.current.patchTab(second!, { sql: "SELECT 2", dbId: "db1", limit: 100 }));
  const stored = JSON.parse(localStorage.getItem("sqllab:tabs:eng")!);
  expect(stored.find((t: { id: string }) => t.id === second).sql).toBe("SELECT 2");
  act(() => result.current.closeTab(second!));
  expect(result.current.tabs).toHaveLength(1);
  expect(result.current.activeId).toBe(first);
  act(() => result.current.closeTab(first));
  expect(result.current.tabs).toHaveLength(1); // never below one
  rerender();
});
