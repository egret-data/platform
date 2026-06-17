import { apiFetch, UnauthorizedError, readCookie } from "@/api/client";
import { afterEach, expect, test, vi } from "vitest";

afterEach(() => vi.restoreAllMocks());

test("attaches X-Egret-CSRF on mutating requests from the cookie", async () => {
  document.cookie = "egret_csrf=tok123";
  const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("{}", { status: 200 }));
  await apiFetch("/x", { method: "POST" });
  const headers = (spy.mock.calls[0][1] as RequestInit).headers as Headers;
  expect(headers.get("X-Egret-CSRF")).toBe("tok123");
});

test("does not attach CSRF on GET", async () => {
  document.cookie = "egret_csrf=tok123";
  const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("{}", { status: 200 }));
  await apiFetch("/x");
  const headers = (spy.mock.calls[0][1] as RequestInit).headers as Headers;
  expect(headers.get("X-Egret-CSRF")).toBeNull();
});

test("throws UnauthorizedError on 401", async () => {
  vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 401 }));
  await expect(apiFetch("/x")).rejects.toBeInstanceOf(UnauthorizedError);
});

test("readCookie parses a value", () => {
  document.cookie = "egret_csrf=abc";
  expect(readCookie("egret_csrf")).toBe("abc");
});
