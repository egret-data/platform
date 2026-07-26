import { render, screen } from "@testing-library/react";
import { toast } from "sonner";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import App from "@/App";

// Regression guard for a bug that shipped silently: the `<Toaster/>` host is
// defined in components/ui/sonner.tsx but was never mounted, so every `toast()`
// call across the app (32 call sites) was a no-op and nothing caught it. This
// asserts App actually mounts a working toast host — fire a toast, see it.
//
// `useMe()` swallows fetch errors and returns null, so a rejecting fetch lets
// App render the login route without network; the Toaster is a sibling of the
// router, so it mounts regardless of route.

beforeEach(() => {
  vi.stubGlobal("fetch", vi.fn(() => Promise.reject(new Error("no network in test"))));
});

afterEach(() => {
  vi.unstubAllGlobals();
});

test("App mounts a global toast host so toast() is not a no-op", async () => {
  render(<App />);

  toast.success("toaster-mounted-probe");

  expect(await screen.findByText("toaster-mounted-probe")).toBeInTheDocument();
});
