import { act, renderHook } from "@testing-library/react";
import { expect, test } from "vitest";
import { useBulkSelect } from "@/hooks/useBulkSelect";

const rows = [{ id: "a" }, { id: "b" }, { id: "c" }];

test("toggle, select-all, clear", () => {
  const { result } = renderHook(() => useBulkSelect(rows));
  expect(result.current.bulkMode).toBe(false);
  act(() => result.current.setBulkMode(true));
  act(() => result.current.toggle("a"));
  expect(result.current.selected.has("a")).toBe(true);
  expect(result.current.someSelected).toBe(true);
  expect(result.current.allSelected).toBe(false);
  act(() => result.current.toggleAll());
  expect(result.current.allSelected).toBe(true);
  act(() => result.current.toggleAll());
  expect(result.current.selected.size).toBe(0);
  act(() => { result.current.toggle("b"); result.current.clear(); });
  expect(result.current.selected.size).toBe(0);
});
