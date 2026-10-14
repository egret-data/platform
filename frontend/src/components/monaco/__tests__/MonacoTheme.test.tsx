import { afterEach, describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";

/**
 * Monaco's theme is global: the last editor to set one decides for every
 * editor on the page, and `@monaco-editor/react` sets "light" when a caller
 * names no theme. So the wrapper must default the theme from the app's mode,
 * or one theme-less editor turns all the others light in dark mode (W7d).
 *
 * The stub stands in for the lazy chunk and shows the theme it was given.
 */
vi.mock("@/lib/monaco", () => {
  const Stub = ({ theme }: { theme?: string }) => <div data-testid="editor" data-theme={theme} />;
  return { default: Stub, DiffEditor: ({ theme }: { theme?: string }) => <div data-testid="diff" data-theme={theme} /> };
});

afterEach(() => {
  document.documentElement.classList.remove("dark");
});

describe("MonacoEditor theme", () => {
  it("defaults to vs-dark when the app is dark and follows a class flip", async () => {
    document.documentElement.classList.add("dark");
    const { default: MonacoEditor } = await import("../MonacoEditor");

    render(<MonacoEditor value="x" />);

    expect(await screen.findByTestId("editor")).toHaveAttribute("data-theme", "vs-dark");
    await act(async () => {
      document.documentElement.classList.remove("dark");
    });
    expect(screen.getByTestId("editor")).toHaveAttribute("data-theme", "vs");
  });

  it("defaults the diff editor from the app mode too", async () => {
    document.documentElement.classList.add("dark");
    const { MonacoDiffEditor } = await import("../MonacoEditor");

    render(<MonacoDiffEditor original="a" modified="b" />);

    expect(await screen.findByTestId("diff")).toHaveAttribute("data-theme", "vs-dark");
  });

  it("lets a caller's own theme win", async () => {
    document.documentElement.classList.add("dark");
    const { default: MonacoEditor } = await import("../MonacoEditor");

    render(<MonacoEditor value="x" theme="egret-dark" />);

    expect(await screen.findByTestId("editor")).toHaveAttribute("data-theme", "egret-dark");
  });
});
